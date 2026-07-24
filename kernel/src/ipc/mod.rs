use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::AtomicBool;
use spin::Mutex;

use crate::task::{self, TaskId};

use crate::capability::{Capability, CapabilityTable};

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 4;
pub const CHANNEL_QUEUE: usize = 16;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

#[derive(Clone)]
pub struct Message {
    pub bytes: [u8; MAX_MSG],
    pub len: usize,
    pub caps: [Option<Capability>; MAX_CAPS_PER_MSG],
}

/// A directed message queue plus the id of the single task (if any) parked in
/// `SYS_WAIT` on receiving from it. SPSC 1:1 registration is sufficient for the
/// current channel model; the waiter is consumed on the first wake.
pub struct Channel {
    pub queue: VecDeque<Message>,
    pub recv_waiter: Option<TaskId>,
}

impl Channel {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            recv_waiter: None,
        }
    }
}

pub struct EndpointInner {
    pub channel: Arc<Mutex<Channel>>,
    pub peer_channel: Weak<Mutex<Channel>>,
    pub owner_alive: Arc<AtomicBool>,
    pub peer_owner_alive: Weak<AtomicBool>,
}

pub struct Endpoint {
    inner: Arc<EndpointInner>,
    owner_alive: Arc<AtomicBool>,
}

impl Endpoint {
    pub fn inner(&self) -> &EndpointInner {
        &self.inner
    }

    /// Reports whether this endpoint's own receive queue holds a message.
    pub fn has_pending(&self) -> bool {
        !self.inner.channel.lock().queue.is_empty()
    }

    /// Reports whether the peer owner has been dropped, so a `recv` here would
    /// return `ERR_PEER_DEAD` rather than block forever.
    pub fn peer_dead(&self) -> bool {
        self.inner.peer_owner_alive.upgrade().is_none()
    }

    /// Records `id` as the task waiting to receive on this endpoint. A later
    /// peer `send` (or peer death) wakes it. Single-slot (SPSC): each channel
    /// has exactly one receiving owner, so a second distinct waiter would drop
    /// a wake. Assert the invariant in debug builds to catch a future fan-in
    /// channel rather than hang silently.
    pub fn register_recv_waiter(&self, id: TaskId) {
        let mut channel = self.inner.channel.lock();
        debug_assert!(
            channel.recv_waiter.is_none() || channel.recv_waiter == Some(id),
            "second concurrent recv waiter would drop a wake"
        );
        channel.recv_waiter = Some(id);
    }
}

impl Clone for Endpoint {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            owner_alive: self.owner_alive.clone(),
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if Arc::strong_count(&self.owner_alive) == 2 {
            self.owner_alive
                .store(false, core::sync::atomic::Ordering::Release);
            // Wake a peer parked in `recv`/`SYS_WAIT`: it will now observe
            // `ERR_PEER_DEAD` instead of blocking forever. Take the waiter
            // under the peer channel lock, release it, then enqueue the wake
            // (lock order: Channel -> PENDING_WAKES).
            if let Some(peer_channel) = self.inner.peer_channel.upgrade() {
                let waiter = peer_channel.lock().recv_waiter.take();
                if let Some(waiter) = waiter {
                    task::wake(waiter);
                }
            }
        }
    }
}

pub fn channel() -> (Endpoint, Endpoint) {
    let a_alive = Arc::new(AtomicBool::new(true));
    let b_alive = Arc::new(AtomicBool::new(true));
    let a_channel = Arc::new(Mutex::new(Channel::new()));
    let b_channel = Arc::new(Mutex::new(Channel::new()));

    let a = Endpoint {
        inner: Arc::new(EndpointInner {
            channel: a_channel.clone(),
            peer_channel: Arc::downgrade(&b_channel),
            owner_alive: a_alive.clone(),
            peer_owner_alive: Arc::downgrade(&b_alive),
        }),
        owner_alive: a_alive.clone(),
    };
    let b = Endpoint {
        inner: Arc::new(EndpointInner {
            channel: b_channel,
            peer_channel: Arc::downgrade(&a_channel),
            owner_alive: b_alive.clone(),
            peer_owner_alive: Arc::downgrade(&a_alive),
        }),
        owner_alive: b_alive.clone(),
    };
    (a, b)
}

pub fn send(ep: &Endpoint, bytes: &[u8], caps: &mut [Option<Capability>; MAX_CAPS_PER_MSG]) -> i64 {
    let ep = ep.inner();
    if ep.peer_owner_alive.upgrade().is_none() {
        return ERR_PEER_DEAD;
    }
    let Some(peer_channel) = ep.peer_channel.upgrade() else {
        return ERR_PEER_DEAD;
    };

    let waiter = {
        let mut channel = peer_channel.lock();
        if channel.queue.len() >= CHANNEL_QUEUE {
            return ERR_WOULDBLOCK;
        }

        let len = bytes.len().min(MAX_MSG);
        let mut msg = Message {
            bytes: [0; MAX_MSG],
            len,
            caps: core::array::from_fn(|_| None),
        };
        msg.bytes[..len].copy_from_slice(&bytes[..len]);
        for (dst, src) in msg.caps.iter_mut().zip(caps.iter_mut()) {
            *dst = src.take();
        }
        channel.queue.push_back(msg);
        // Take the peer's receive waiter while holding the channel lock, then
        // release it before waking (lock order: Channel -> PENDING_WAKES).
        channel.recv_waiter.take()
    };
    if let Some(waiter) = waiter {
        task::wake(waiter);
    }
    ERR_SUCCESS
}

pub fn recv(
    ep: &Endpoint,
    buf: &mut [u8],
    cap_out: &mut [u64; MAX_CAPS_PER_MSG],
    caps: &mut CapabilityTable,
) -> i64 {
    let ep = ep.inner();
    let mut channel = ep.channel.lock();
    if let Some(msg) = channel.queue.front() {
        let cap_count = msg.caps.iter().filter(|cap| cap.is_some()).count();
        if caps.available_slots() < cap_count {
            return ERR_OUT_OF_MEMORY;
        }

        let mut msg = channel
            .queue
            .pop_front()
            .expect("front message disappeared");
        let len = msg.len.min(buf.len());
        buf[..len].copy_from_slice(&msg.bytes[..len]);
        for (i, cap) in msg.caps.iter_mut().enumerate() {
            cap_out[i] = 0;
            if let Some(cap) = cap.take() {
                cap_out[i] = caps
                    .insert(cap)
                    .expect("cap-table capacity changed after preflight")
                    as u64;
            }
        }
        return len as i64;
    }
    drop(channel);

    if ep.peer_owner_alive.upgrade().is_none() {
        ERR_PEER_DEAD
    } else {
        ERR_WOULDBLOCK
    }
}
