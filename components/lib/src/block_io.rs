//! One synchronous block client over the IO0 ring and `block/v2` (B83).
//!
//! Every migrated storage client needs the same nine steps: create the ring
//! buffer and the payload buffer, map both, format the ring, loan both halves
//! to the driver, wait for its capacity announcement, submit a request, signal,
//! wait, and decode the completion. Each of the eight storage-family probes
//! doing that by hand is eight chances to get the cursor discipline, the lease
//! bookkeeping, or the direction wrong -- and a client that gets the direction
//! wrong programs a device to write into a buffer the device was meant to read.
//!
//! # Why synchronous
//!
//! The retired root `BlockTransact` was a call: the caller blocked until the
//! sector moved. The clients migrated here were written against that shape, and
//! their gates assert an ordering the asynchronous ring does not itself impose.
//! This adapter restores the call shape over the ring rather than rewriting
//! eight probes and their assertions into continuations. Clients wanting real
//! concurrency use [`slime_proto::io_queue_ring`] directly, as `io-block-probe`
//! does; nothing here forecloses that.
//!
//! # What this does not decide
//!
//! Not authority. This is the client half, and a client cannot grant itself
//! block rights: the ring's rights live in the generation's
//! `block-ring-authority` table, which only the driver reads. A write this
//! adapter submits on a read-only ring is refused by the driver with
//! `STATUS_BAD_RIGHTS`, and [`BlockError::Refused`] is how the caller learns
//! that. Adding a client-side rights check here would be worse than useless:
//! it would look like enforcement while being trivially removable.

use boot_contracts::generation::{RIGHT_BUFFER_MAP, RIGHT_BUFFER_WRITE};
use slime_proto::block_v2::{self, WireBlockReply, WireBlockRequest};
use slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN;
use slime_proto::io_queue::{self, COMPLETION_PAYLOAD_BYTES, WireBufferSlice};
use slime_proto::io_queue_ring::{Outstanding, Queue, QueueError, format};
use slime_rt::{
    CapabilityDisposition, ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, capability_delegate,
    notification_signal, notification_wait, shared_buffer_create, shared_buffer_loan,
    shared_buffer_map, yield_now,
};

/// Ring depth. One outstanding request at a time is all a synchronous caller
/// can have, but the ring is the mechanism's minimum admissible power of two
/// and a deeper ring costs nothing here: the depth bounds concurrency, and this
/// adapter's concurrency is one by construction.
const SLOTS: usize = 8;
/// Bytes of the sector payload buffer, and so the largest transfer.
const DATA_PAGES: usize = 8;
const PAGE: u64 = 4096;
const DATA_BYTES: u64 = DATA_PAGES as u64 * PAGE;
/// Yields given up before concluding the driver will never answer. A bound
/// rather than a timeout: this has no clock, and an unbounded spin against a
/// dead driver would hang a plane instead of failing it.
const ANSWER_YIELDS: u32 = 2_000_000;

/// Why a block operation could not complete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockError {
    /// Setup failed: a buffer could not be created, mapped, loaned, or handed
    /// to the driver.
    Setup,
    /// The driver refused the request. `status` is the IO0 queue status, so
    /// [`io_queue::STATUS_BAD_RIGHTS`] here is a ring whose declared authority
    /// excludes the operation -- the replacement for the root's rights refusal.
    Refused { status: u32, device_status: u32 },
    /// The driver produced nothing within [`ANSWER_YIELDS`], or died.
    Lost,
    /// The ring, the completion, or the reply did not decode.
    Malformed,
    /// The request itself is inadmissible: a count past the payload buffer, or
    /// an operation this adapter does not carry.
    BadRequest,
}

/// A block reply, with the queue and device statuses kept separate.
///
/// Both matter and they answer different questions: the queue status says
/// whether the request was admitted and authorized, the device status whether
/// the medium honoured it. Collapsing them would make "refused for want of
/// authority" indistinguishable from "the disk failed".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockReply {
    pub sectors_done: u32,
    pub device_status: u32,
    pub detail: u64,
    pub transferred: u64,
}

/// A synchronous block client bound to one ring and one payload buffer.
pub struct BlockIo<'a> {
    queue: Queue<'a>,
    outstanding: Outstanding<SLOTS>,
    data: &'a mut [u8],
    data_buffer: u64,
    data_lease: u64,
    peer: u32,
    request_ready: u32,
    completion_ready: u32,
    next_id: u64,
    capacity: u64,
}

impl<'a> BlockIo<'a> {
    /// Create both buffers, hand the driver its loans, and learn the device's
    /// capacity.
    ///
    /// `ring_base` and `data_base` are where this component maps its own
    /// halves; `peer` is the endpoint slot the driver receives loans on. The
    /// two loans are delegated ring-first, because the driver's own receive
    /// order is positional and reversing them would silently exchange the
    /// ring for the payload buffer.
    ///
    /// # Safety
    ///
    /// `ring_base` and `data_base` must be page-aligned addresses this
    /// component's VSpace has free for `PAGE` and [`DATA_BYTES`] bytes, and
    /// must not alias each other or anything else this component maps. The
    /// returned `BlockIo` borrows both regions for `'a`.
    pub unsafe fn attach(
        factory_slot: u32,
        peer_slot: u32,
        request_ready: u32,
        completion_ready: u32,
        ring_base: u64,
        data_base: u64,
    ) -> Result<Self, BlockError> {
        let ring = shared_buffer_create(factory_slot, 1, true).map_err(|_| BlockError::Setup)?;
        let data =
            shared_buffer_create(factory_slot, DATA_PAGES, true).map_err(|_| BlockError::Setup)?;
        if shared_buffer_map(ring.slot, ring_base, 0, PAGE, true) != ERR_SUCCESS
            || shared_buffer_map(data.slot, data_base, 0, DATA_BYTES, true) != ERR_SUCCESS
        {
            return Err(BlockError::Setup);
        }
        // SAFETY: the caller's contract -- both regions are free, page-aligned,
        // non-aliasing, and were just mapped writable at these addresses.
        let ring_bytes =
            unsafe { core::slice::from_raw_parts_mut(ring_base as *mut u8, PAGE as usize) };
        // SAFETY: as above, for the payload region.
        let data_bytes =
            unsafe { core::slice::from_raw_parts_mut(data_base as *mut u8, DATA_BYTES as usize) };
        format(ring_bytes, SLOTS, 1).map_err(|_| BlockError::Setup)?;
        let ring_loan = delegate(ring.slot, peer_slot, PAGE)?;
        let data_loan = delegate(data.slot, peer_slot, DATA_BYTES)?;
        let _ = ring_loan;
        let queue = Queue::attach(ring_bytes, SLOTS).map_err(|_| BlockError::Setup)?;
        let outstanding = Outstanding::<SLOTS>::new(queue.epoch());
        let capacity = await_capacity(peer_slot)?;
        Ok(Self {
            queue,
            outstanding,
            data: data_bytes,
            data_buffer: data.id,
            data_lease: data_loan.lease,
            peer: peer_slot,
            request_ready,
            completion_ready,
            next_id: 1,
            capacity,
        })
    }

    /// Sectors the device reports. The driver's announcement, not a request.
    pub const fn capacity(&self) -> u64 {
        self.capacity
    }

    /// Read `sector.len() / SECTOR_BYTES` sectors from `lba`.
    pub fn read(&mut self, lba: u64, sector: &mut [u8]) -> Result<BlockReply, BlockError> {
        let count = sector_count(sector.len())?;
        let reply = self.transact(
            block_v2::OP_READ,
            lba,
            count,
            io_queue::DIRECTION_DEVICE_WRITE,
            sector.len() as u64,
        )?;
        sector.copy_from_slice(&self.data[..sector.len()]);
        Ok(reply)
    }

    /// Write `sector` at `lba`.
    pub fn write(&mut self, lba: u64, sector: &[u8]) -> Result<BlockReply, BlockError> {
        let count = sector_count(sector.len())?;
        self.data[..sector.len()].copy_from_slice(sector);
        self.transact(
            block_v2::OP_WRITE,
            lba,
            count,
            io_queue::DIRECTION_DEVICE_READ,
            sector.len() as u64,
        )
    }

    /// Flush the device's write cache.
    ///
    /// Carries `DIRECTION_NONE` and a zero-length slice: a flush moves no
    /// bytes, so declaring a direction would name a transfer that does not
    /// happen.
    pub fn flush(&mut self) -> Result<BlockReply, BlockError> {
        self.transact(block_v2::OP_FLUSH, 0, 0, io_queue::DIRECTION_NONE, 0)
    }

    /// Ask the device for its capacity through the ring.
    ///
    /// Distinct from [`BlockIo::capacity`], which reports the driver's boot
    /// announcement. This is a request, so it is what an authority arm must
    /// use: a ring with no right is refused here and answered there.
    pub fn geometry(&mut self) -> Result<BlockReply, BlockError> {
        self.transact(block_v2::OP_GEOMETRY, 0, 0, io_queue::DIRECTION_NONE, 0)
    }

    /// Tell the driver this client is finished.
    ///
    /// A blocking send against a driver that polls its endpoint: the driver's
    /// non-blocking receive completes against a sender already parked here, so
    /// exactly one side blocks and the rendezvous always happens. Both sides
    /// polling would let each observe the other as absent forever.
    pub fn shutdown(&mut self) -> Result<(), BlockError> {
        if slime_rt::send(self.peer, &[1], &[]) == ERR_SUCCESS {
            Ok(())
        } else {
            Err(BlockError::Lost)
        }
    }

    /// Submit one raw request, for the arms that must send something the typed
    /// operations above cannot express -- a malformed magic, an unknown
    /// opcode, or a count past the device.
    pub fn transact_raw(
        &mut self,
        request: WireBlockRequest,
        direction: u32,
        length: u64,
    ) -> Result<BlockReply, BlockError> {
        self.submit_and_settle(request, direction, length)
    }

    fn transact(
        &mut self,
        op: u8,
        lba: u64,
        sector_count: u32,
        direction: u32,
        length: u64,
    ) -> Result<BlockReply, BlockError> {
        let request = WireBlockRequest {
            magic: block_v2::BLOCK_MAGIC,
            version: block_v2::FORMAT_VERSION,
            op,
            flags: 0,
            lba,
            sector_count,
            reserved: [0; 4],
            padding: [0; 32],
        };
        self.submit_and_settle(request, direction, length)
    }

    fn submit_and_settle(
        &mut self,
        request: WireBlockRequest,
        direction: u32,
        length: u64,
    ) -> Result<BlockReply, BlockError> {
        if length > DATA_BYTES {
            return Err(BlockError::BadRequest);
        }
        // `DIRECTION_NONE` requires an entirely zero slice, not merely a
        // zero length: a slice naming a buffer and a lease it will not touch
        // would claim a transfer that does not happen, and `valid_buffer_slice`
        // refuses it. FLUSH and GEOMETRY move no bytes.
        let slice = if direction == io_queue::DIRECTION_NONE {
            WireBufferSlice {
                buffer: 0,
                lease: 0,
                offset: 0,
                length: 0,
                direction,
                reserved: [0; 4],
            }
        } else {
            WireBufferSlice {
                buffer: self.data_buffer,
                lease: self.data_lease,
                offset: 0,
                length,
                direction,
                reserved: [0; 4],
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.queue
            .submit(id, &slice, &request.encode(), false, DATA_BYTES)
            .map_err(|error| match error {
                QueueError::Closed => BlockError::Lost,
                QueueError::Malformed | QueueError::TooLarge => BlockError::BadRequest,
                _ => BlockError::Setup,
            })?;
        self.outstanding
            .admit(id, slice.lease, slice.length)
            .map_err(|_| BlockError::Setup)?;
        if notification_signal(self.request_ready) != ERR_SUCCESS {
            return Err(BlockError::Lost);
        }
        let mut body = [0u8; COMPLETION_PAYLOAD_BYTES];
        // Retry, not a single wait. `completion_ready` is a *latched*
        // notification, so a completion produced while this client was between
        // its signal and its first ring check leaves a wake latched with no
        // entry behind it. The next request's wait then returns immediately,
        // finds the ring still empty, and a single-wait loop would report the
        // live request lost -- without settling it, desynchronising every later
        // completion.
        //
        // Liveness comes from the ring's `driver_state`, not from a wake count.
        // `notification_wait` is `seL4_Wait`: it blocks rather than yielding, so
        // a counter over waits never advances against a driver that stopped
        // signalling, and the bound would be unreachable exactly when it was
        // needed. `driver_state` is the driver's own single-writer field and a
        // supervised death marks it `DRIVER_DEAD`, which is a fact rather than a
        // guess about elapsed time.
        //
        // Checked *before* each wait, so a driver that died between the ring
        // check and the wait is seen on the next iteration rather than parked on.
        let completion = loop {
            match self.queue.take_completion(&self.outstanding, &mut body) {
                Ok(completion) if completion.request_id == id => break completion,
                // A completion for another identity cannot occur with one
                // request in flight, and is not silently dropped: the ring has
                // already consumed it, so the only honest answer is that this
                // ring is not behaving as its contract says.
                Ok(_) => return Err(BlockError::Malformed),
                Err(QueueError::Empty) => {
                    // A dead or resetting driver will produce no further
                    // completion, so waiting for one would park forever. The
                    // request stays admitted; settling it is the caller's
                    // decision once it knows the epoch ended.
                    if self.queue.driver_state() == io_queue::DRIVER_DEAD {
                        return Err(BlockError::Lost);
                    }
                    if notification_wait(self.completion_ready).is_err() {
                        return Err(BlockError::Lost);
                    }
                }
                Err(_) => return Err(BlockError::Malformed),
            }
        };
        // Settle before judging status: the lease must be released exactly once
        // whether the driver honoured the request or refused it, and an early
        // return on refusal would leak it.
        let settled = self
            .outstanding
            .settle(completion.request_id, completion.status)
            .map_err(|_| BlockError::Malformed)?;
        if settled.lease != slice.lease {
            return Err(BlockError::Malformed);
        }
        let reply = WireBlockReply::decode(&body[..completion.payload_len])
            .filter(|reply| {
                reply.magic == block_v2::BLOCK_MAGIC && reply.version == block_v2::FORMAT_VERSION
            })
            .ok_or(BlockError::Malformed)?;
        if completion.status != io_queue::STATUS_OK {
            return Err(BlockError::Refused {
                status: completion.status,
                device_status: reply.device_status,
            });
        }
        Ok(BlockReply {
            sectors_done: reply.sectors_done,
            device_status: reply.device_status,
            detail: reply.detail,
            transferred: completion.transferred,
        })
    }
}

struct Loan {
    lease: u64,
}

fn delegate(buffer_slot: u32, peer: u32, length: u64) -> Result<Loan, BlockError> {
    let loan =
        shared_buffer_loan(buffer_slot, peer, 0, length, true).map_err(|_| BlockError::Setup)?;
    let mut descriptor = [0u8; MAX_MSG];
    descriptor[..8].copy_from_slice(&loan.id.to_le_bytes());
    if capability_delegate(
        peer,
        loan.slot,
        CapabilityDisposition::Move,
        OBJECT_KIND_SHARED_BUFFER_LOAN,
        RIGHT_BUFFER_MAP | RIGHT_BUFFER_WRITE,
        &descriptor,
    ) != ERR_SUCCESS
    {
        return Err(BlockError::Setup);
    }
    Ok(Loan { lease: loan.id })
}

/// Wait for the driver's capacity announcement on the peer endpoint.
///
/// Bounded rather than blocking: a driver that never announces must fail the
/// plane rather than park it, and this component has no clock to time out on.
fn await_capacity(peer: u32) -> Result<u64, BlockError> {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    for _ in 0..ANSWER_YIELDS {
        match slime_rt::recv(peer, &mut bytes, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => return Err(BlockError::Lost),
            result if result as usize >= 8 => {
                return Ok(u64::from_le_bytes(bytes[..8].try_into().unwrap()));
            }
            _ => return Err(BlockError::Malformed),
        }
    }
    Err(BlockError::Lost)
}

fn sector_count(bytes: usize) -> Result<u32, BlockError> {
    if bytes == 0
        || !bytes.is_multiple_of(block_v2::SECTOR_BYTES)
        || bytes > DATA_BYTES as usize
        || bytes / block_v2::SECTOR_BYTES > block_v2::MAX_SECTORS_PER_REQUEST as usize
    {
        return Err(BlockError::BadRequest);
    }
    Ok((bytes / block_v2::SECTOR_BYTES) as u32)
}
