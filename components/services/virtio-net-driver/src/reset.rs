//! Terminal settlement for link requests still queued when reset stops admission.

use slime_proto::io_queue::{REQUEST_PAYLOAD_BYTES, STATUS_MALFORMED, STATUS_RESET};
use slime_proto::io_queue_ring::{Queue, QueueError};
use slime_proto::link_device::{self, WireLinkReply};

pub fn settle_queued<const N: usize>(
    queue: &mut Queue<'_>,
    admitted: &[Option<u64>; N],
    mapped_bytes: u64,
) -> Result<usize, QueueError> {
    if N != queue.slot_count() {
        return Err(QueueError::Malformed);
    }
    let mut body = [0; REQUEST_PAYLOAD_BYTES];
    let mut seen = [None; N];
    let mut count = 0;
    let reply = WireLinkReply {
        magic: link_device::LINK_MAGIC,
        version: link_device::FORMAT_VERSION,
        op: link_device::OP_RESET,
        link_state: link_device::LINK_UP,
        frame_len: 0,
        reserved: [0; 2],
        tx_frames: 0,
        rx_frames: 0,
        detail: 0,
    }
    .encode();
    // Reset closes submission admission. These entries own no DMA requests,
    // but their current-epoch identities still require exactly one terminal reply.
    for seen_slot in 0..N {
        let (id, status) = match queue.take_request(&mut body, mapped_bytes) {
            Ok(submission) => (submission.request_id, STATUS_RESET),
            Err(error) if error.error == QueueError::Empty => break,
            Err(error) if error.request_id != 0 && error.epoch == queue.epoch() => {
                (error.request_id, STATUS_MALFORMED)
            }
            Err(_) => continue,
        };
        if admitted.contains(&Some(id)) || seen.contains(&Some(id)) {
            continue;
        }
        seen[seen_slot] = Some(id);
        queue.complete(id, status, 0, &reply, true)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use slime_proto::io_queue::{
        self, COMPLETION_PAYLOAD_BYTES, QUEUE_HEADER_LEN, REQUEST_SLOT_LEN, WireBufferSlice,
    };
    use slime_proto::io_queue_ring::{Outstanding, format, mapping_bytes};
    use std::vec;

    const SLOTS: usize = 8;
    const PAGE: u64 = 4096;
    fn slice() -> WireBufferSlice {
        WireBufferSlice {
            buffer: 3,
            lease: 7,
            offset: 0,
            length: 1518,
            direction: io_queue::DIRECTION_DEVICE_WRITE,
            reserved: [0; 4],
        }
    }

    #[test]
    fn reset_settles_queued_receive_once_without_admission_or_stale_alias() {
        let mut bytes = vec![0; mapping_bytes(SLOTS).unwrap()];
        format(&mut bytes, SLOTS, 2).unwrap();
        let mut outstanding: Outstanding<SLOTS> = Outstanding::new(2);
        outstanding.admit(2, 7, 1518).unwrap();
        outstanding.admit(4, 7, 1518).unwrap();
        {
            let mut queue = Queue::attach(&mut bytes, SLOTS).unwrap();
            queue.submit(2, &slice(), &[], false, PAGE).unwrap();
            queue
                .take_request(&mut [0; REQUEST_PAYLOAD_BYTES], PAGE)
                .unwrap();
            queue.complete(2, STATUS_RESET, 0, &[], true).unwrap();
            for id in [4, 4, 2, 6] {
                queue.submit(id, &slice(), &[], false, PAGE).unwrap();
            }
            queue.begin_reset();
        }
        let stale_epoch = QUEUE_HEADER_LEN
            + slime_proto::queue_slot_index(5, SLOTS) * REQUEST_SLOT_LEN
            + io_queue::OFF_REQUEST_EPOCH;
        bytes[stale_epoch..stale_epoch + 8].copy_from_slice(&1u64.to_le_bytes());
        let mut queue = Queue::attach(&mut bytes, SLOTS).unwrap();
        let mut admitted = [None; SLOTS];
        admitted[0] = Some(2);
        assert_eq!(settle_queued(&mut queue, &admitted, PAGE), Ok(1));
        assert_eq!(queue.submitted(), 0);
        assert_eq!(queue.completions_pending(), 2);
        let mut body = [0; COMPLETION_PAYLOAD_BYTES];
        for id in [2, 4] {
            let completion = queue.take_completion(&outstanding, &mut body).unwrap();
            assert_eq!(completion.request_id, id);
            assert_eq!(completion.status, STATUS_RESET);
            assert_eq!(completion.transferred, 0);
            assert!(completion.epoch_ended);
            if id == 4 {
                assert_eq!(
                    WireLinkReply::decode(&body[..completion.payload_len])
                        .unwrap()
                        .op,
                    link_device::OP_RESET
                );
            }
            assert_eq!(outstanding.settle(id, STATUS_RESET).unwrap().lease, 7);
        }
        assert!(outstanding.is_empty());
        assert_eq!(
            queue.take_completion(&outstanding, &mut body),
            Err(QueueError::Empty)
        );
        assert_eq!(queue.advance_epoch(), Ok(3));
        outstanding.adopt_epoch(3).unwrap();
        assert_eq!(outstanding.epoch(), 3);
    }

    #[test]
    fn reset_completion_backpressure_is_bounded_and_does_not_overwrite() {
        let mut bytes = vec![0; mapping_bytes(SLOTS).unwrap()];
        format(&mut bytes, SLOTS, 1).unwrap();
        let mut queue = Queue::attach(&mut bytes, SLOTS).unwrap();
        let mut outstanding: Outstanding<SLOTS> = Outstanding::new(1);
        for id in 1..=SLOTS as u64 {
            outstanding.admit(id, 7, 1518).unwrap();
            queue.submit(id, &slice(), &[], false, PAGE).unwrap();
            queue
                .take_request(&mut [0; REQUEST_PAYLOAD_BYTES], PAGE)
                .unwrap();
            queue.complete(id, STATUS_RESET, 0, &[], true).unwrap();
        }
        queue.submit(99, &slice(), &[], false, PAGE).unwrap();
        queue.begin_reset();
        assert_eq!(
            settle_queued(&mut queue, &[None; SLOTS], PAGE),
            Err(QueueError::Full)
        );
        assert_eq!(queue.completions_pending(), SLOTS as u64);
        let mut body = [0; COMPLETION_PAYLOAD_BYTES];
        for id in 1..=SLOTS as u64 {
            assert_eq!(
                queue
                    .take_completion(&outstanding, &mut body)
                    .unwrap()
                    .request_id,
                id
            );
        }
    }
}
