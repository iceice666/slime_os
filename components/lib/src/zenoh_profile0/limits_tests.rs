//! Host tests for the link configuration: the three pinned configurations and the boundaries
//! `contracts/zenoh-link-limits/v1` states, each on both sides.

use super::*;

fn with(base: Limits, change: impl FnOnce(&mut Limits)) -> Limits {
    let mut limits = base;
    change(&mut limits);
    limits.stack_bytes = limits.required_stack();
    if limits.mode == Mode::Heap {
        limits.heap_bytes = limits.heap_needed();
    }
    limits
}

#[test]
fn the_pinned_configurations_are_accepted() {
    for limits in [DEMO, LARGE, HEAP] {
        assert_eq!(limits.check(), Ok(()), "{}", limits.name);
    }
}

#[test]
fn the_pinned_figures_are_the_arithmetic() {
    assert_eq!(HEAP.message_bytes(), 16_556);
    assert_eq!(HEAP.heap_needed(), 149_004);
    assert_eq!(DEMO.required_stack(), 5442);
    assert_eq!(LARGE.required_stack(), 26_696);
    assert_eq!(HEAP.required_stack(), 18_504);
    assert!(DEMO.required_stack() <= DEMO.stack_bytes);
    assert!(LARGE.required_stack() <= LARGE.stack_bytes);
    assert!(HEAP.required_stack() <= HEAP.stack_bytes);
}

#[test]
fn a_batch_holds_the_handshake_and_one_operation() {
    let smallest = HANDSHAKE_BYTES + STREAM_LENGTH_BYTES;
    let heap = |batch: usize| {
        with(HEAP, |l| {
            l.batch_bytes = batch;
            l.key_bytes = 1;
        })
    };
    assert_eq!(heap(smallest - 1).check(), Err(LimitsError::Batch));
    assert_eq!(heap(smallest).check(), Ok(()));
    assert_eq!(heap(DATA_BYTES).check(), Ok(()));
    assert_eq!(heap(DATA_BYTES + 1).check(), Err(LimitsError::Batch));
    assert_eq!(heap(0).check(), Err(LimitsError::Batch));
}

#[test]
fn a_declaration_fits_the_batch_to_the_byte() {
    let key = |batch: usize, key: usize| {
        with(HEAP, |l| {
            l.batch_bytes = batch;
            l.key_bytes = key;
        })
    };
    // 19 bytes around a key below 126, 20 from 126, 21 from 16382; the batch holds all of it.
    assert_eq!(key(144, 125).declaration_bytes(), 144);
    assert_eq!(key(144, 125).check(), Ok(()));
    assert_eq!(key(143, 125).check(), Err(LimitsError::Batch));
    assert_eq!(key(146, 126).declaration_bytes(), 146);
    assert_eq!(key(146, 126).check(), Ok(()));
    assert_eq!(key(145, 126).check(), Err(LimitsError::Batch));
    assert_eq!(key(4096, 4076).check(), Ok(()));
    assert_eq!(key(4096, 4077).check(), Err(LimitsError::Batch));
    assert_eq!(key(2048, 2028).check(), Ok(()));
    assert_eq!(key(2048, 2029).check(), Err(LimitsError::Batch));
    assert_eq!(key(89, 70).check(), Ok(()));
    assert_eq!(key(89, 71).check(), Err(LimitsError::Batch));
}

#[test]
fn a_fixed_message_and_its_frame_fill_the_batch_exactly() {
    let payload = |payload: u64| with(LARGE, |l| l.payload_bytes = payload);
    assert_eq!(payload(1869).fixed_need(), 2048);
    assert_eq!(payload(1869).check(), Ok(()));
    assert_eq!(payload(1870).check(), Err(LimitsError::Fixed));
    // The step in the width of the payload length, at 127 and 128 bytes.
    let narrow = with(LARGE, |l| {
        l.payload_bytes = 127;
        l.batch_bytes = 89 + 400;
    });
    assert_eq!(narrow.check(), Ok(()));
    let batch = |payload: u64, batch: usize| {
        with(LARGE, |l| {
            l.payload_bytes = payload;
            l.batch_bytes = batch;
        })
    };
    assert_eq!(
        batch(127, 127 + 129 + 33 + 5 + 2 + 1 + 6 + 2).check(),
        Ok(())
    );
    assert_eq!(
        batch(127, 127 + 129 + 33 + 5 + 2 + 1 + 6 + 2 - 1).check(),
        Err(LimitsError::Fixed)
    );
    assert_eq!(
        batch(128, 128 + 129 + 33 + 5 + 2 + 2 + 6 + 2).check(),
        Ok(())
    );
    assert_eq!(
        batch(128, 128 + 129 + 33 + 5 + 2 + 2 + 6 + 2 - 1).check(),
        Err(LimitsError::Fixed)
    );
}

#[test]
fn a_fixed_link_has_no_timeout_or_heap_and_a_heap_link_needs_both() {
    let mut fixed = LARGE;
    fixed.reassembly_timeout_ms = 1000;
    assert_eq!(fixed.check(), Err(LimitsError::Fixed));
    let mut fixed = LARGE;
    fixed.heap_bytes = 1;
    assert_eq!(fixed.check(), Err(LimitsError::Fixed));
    let mut heap = HEAP;
    heap.reassembly_timeout_ms = 0;
    assert_eq!(heap.check(), Err(LimitsError::Heap));
    let mut heap = HEAP;
    heap.heap_bytes = heap.heap_needed() - 1;
    assert_eq!(heap.check(), Err(LimitsError::Heap));
    let mut heap = HEAP;
    heap.heap_bytes = heap.heap_needed();
    assert_eq!(heap.check(), Ok(()));
    // One message of heap is not enough for four outstanding samples, and two are not for one.
    heap.heap_bytes = heap.message_bytes();
    assert_eq!(heap.check(), Err(LimitsError::Heap));
    let one = with(HEAP, |l| l.outstanding_samples = 1);
    assert_eq!(one.heap_bytes, 3 * one.message_bytes());
    let mut short = one;
    short.heap_bytes = 2 * one.message_bytes();
    assert_eq!(short.check(), Err(LimitsError::Heap));
}

#[test]
fn any_positive_timeout_is_accepted() {
    for timeout in [1u64, 250, 5000, 65_536, 1 << 32, (1 << 63) - 1] {
        let limits = with(HEAP, |l| l.reassembly_timeout_ms = timeout);
        assert_eq!(limits.check(), Ok(()), "{timeout}");
    }
}

#[test]
fn counts_are_bounded_at_what_the_wire_and_the_arithmetic_hold() {
    let counts = |keys: usize, subscribers: usize, samples: usize| {
        with(HEAP, |l| {
            l.max_keys = keys;
            l.max_subscribers = subscribers;
            l.outstanding_samples = samples;
            l.payload_bytes = 4;
        })
    };
    assert_eq!(counts(0, 0, 4).check(), Err(LimitsError::Count));
    assert_eq!(counts(4, 0, 4).check(), Err(LimitsError::Count));
    assert_eq!(counts(2, 3, 4).check(), Err(LimitsError::Count));
    assert_eq!(counts(4, 4, 0).check(), Err(LimitsError::Count));
    assert_eq!(counts(65_535, 65_535, 1).check(), Ok(()));
    assert_eq!(counts(65_536, 65_536, 1).check(), Err(LimitsError::Count));
    assert_eq!(counts(256, 256, 256).check(), Ok(()));
    assert_eq!(counts(1, 1, 65_536).check(), Ok(()));
    assert_eq!(counts(1, 1, MAX_OUTSTANDING_SAMPLES).check(), Ok(()));
    assert_eq!(
        counts(1, 1, MAX_OUTSTANDING_SAMPLES + 1).check(),
        Err(LimitsError::Count)
    );
}

#[test]
fn sizes_are_bounded_at_what_the_wire_carries() {
    let key = |key: usize| {
        with(HEAP, |l| {
            l.key_bytes = key;
            l.batch_bytes = DATA_BYTES;
        })
    };
    assert_eq!(key(0).check(), Err(LimitsError::Size));
    assert_eq!(key(1).check(), Ok(()));
    let mut attachment = HEAP;
    attachment.attachment_bytes = 32;
    assert_eq!(attachment.check(), Err(LimitsError::Size));
    attachment.attachment_bytes = 34;
    assert_eq!(attachment.check(), Err(LimitsError::Size));
    let payload = |payload: u64| with(HEAP, |l| l.payload_bytes = payload);
    assert_eq!(payload(0).check(), Err(LimitsError::Size));
    assert_eq!(payload(1).check(), Ok(()));
    assert_eq!(payload(MAX_PAYLOAD_LENGTH_BYTES).check(), Ok(()));
    assert_eq!(
        payload(MAX_PAYLOAD_LENGTH_BYTES + 1).check(),
        Err(LimitsError::Size)
    );
}

#[test]
fn the_stack_must_cover_the_arithmetic_to_the_byte() {
    let mut limits = LARGE;
    limits.stack_bytes = limits.required_stack();
    assert_eq!(limits.check(), Ok(()));
    limits.stack_bytes -= 1;
    assert_eq!(limits.check(), Err(LimitsError::Stack));
}

#[test]
fn a_name_is_one_to_sixteen_bytes() {
    let mut limits = LARGE;
    limits.name = "";
    assert_eq!(limits.check(), Err(LimitsError::Name));
    limits.name = "0123456789abcdef";
    assert_eq!(limits.check(), Ok(()));
    limits.name = "0123456789abcdefg";
    assert_eq!(limits.check(), Err(LimitsError::Name));
}

#[test]
fn a_variable_length_integer_widens_at_each_seven_bits() {
    for (value, width) in [
        (0u64, 1usize),
        (127, 1),
        (128, 2),
        (16_383, 2),
        (16_384, 3),
        (2_097_151, 3),
        (2_097_152, 4),
        (268_435_455, 4),
        (268_435_456, 5),
        (4_294_967_295, 5),
    ] {
        assert_eq!(leb_bytes(value), width, "{value}");
    }
}
