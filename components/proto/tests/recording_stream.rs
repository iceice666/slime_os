//! C9.5's recording and replay state machine.
//!
//! Every test here defends one of the milestone's required checks against a
//! plausible bug, and two of them are the *reason* the machine exists: a
//! truncated or reordered stream must be refused rather than partially replayed,
//! and the record capacity must be refused structurally.

use slime_proto::fabric_trace::{
    CLOCK_MONOTONIC, CLOCK_SIMULATED, FLAG_TERMINAL, FORMAT_VERSION, KIND_CLOCK_READ, KIND_OUTPUT,
    MAX_LIFECYCLE_STATE, MAX_OUTPUT_CHANNEL, TRACE_MAGIC, WireTraceRecord,
};
use slime_proto::recording_stream::{
    MAX_RECORD_CAPACITY, MAX_STREAM_BYTES, RECORD_BYTES, Recorder, RecordingError, Replay,
};

/// The recording contract's own ceilings must equal the ones this module sizes
/// its arrays by. Two numbers would let a stream be declared at one length and
/// read at another, and the failure would be a misparsed stream rather than a
/// build error.
///
/// Here rather than as a `const _: () = assert!(..)`: `boot-contracts` holds the
/// recording constants and `slime-proto` holds the trace's, and `boot-contracts`
/// does not depend on `slime-proto`, so no crate sees both at compile time.
/// `slime-proto`'s dev-dependency does, which makes this the only place the
/// agreement is checkable at all.
#[test]
fn the_contract_and_the_machine_agree_on_every_bound() {
    assert_eq!(
        MAX_RECORD_CAPACITY,
        boot_contracts::recording_policy::MAX_RECORD_CAPACITY
    );
    assert_eq!(RECORD_BYTES, boot_contracts::recording_policy::RECORD_BYTES);
    assert_eq!(
        MAX_STREAM_BYTES,
        boot_contracts::recording_policy::MAX_STREAM_BYTES
    );
    // A `lifecycle` record's `event` carries a `lifecycle-policy/v1` state id, so
    // the trace's ceiling must equal that policy's last declared state. Smaller
    // would silently refuse a transition the generation admits; larger would admit
    // a state id no policy can produce.
    assert_eq!(
        MAX_LIFECYCLE_STATE,
        boot_contracts::lifecycle_policy::STATE_ERROR
    );
}
/// A recorded run and its replay agree on every input, in order.
#[test]
fn a_recording_replays_every_input_in_order() {
    let mut recorder = Recorder::new(16);
    recorder.clock_read(CLOCK_MONOTONIC, 1_000).expect("clock");
    recorder.advance(10).expect("clock advances");
    recorder.clock_read(CLOCK_SIMULATED, 0).expect("simulated");
    recorder.timer_expiry(7).expect("timer");
    recorder.lifecycle(5).expect("lifecycle");
    recorder.output(1, 42).expect("output");
    recorder.terminal().expect("terminal");

    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    assert_eq!(written, recorder.len() * RECORD_BYTES);

    let mut replay = Replay::open(&bytes[..written], 16).expect("opens");
    assert_eq!(replay.len(), 6);
    assert_eq!(replay.clock_read(CLOCK_MONOTONIC).expect("clock"), 1_000);
    // A simulated clock's first read legitimately answers zero, and refusing it
    // would make the starting instant of a deterministic clock unrecordable.
    assert_eq!(replay.clock_read(CLOCK_SIMULATED).expect("simulated"), 0);
    assert_eq!(replay.timer_expiry().expect("timer"), 7);
    assert_eq!(replay.lifecycle().expect("lifecycle"), 5);
    assert_eq!(replay.output().expect("output"), (1, 42));
    replay.finish().expect("terminal consumed");
    assert_eq!(replay.remaining(), 0);
}

/// Serializing one recording twice produces identical bytes, which is what
/// "byte-identical typed outputs across two boots" rests on at this layer.
#[test]
fn one_recording_serializes_to_identical_bytes_every_time() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 5).expect("clock");
    recorder.output(2, 9).expect("output");
    recorder.terminal().expect("terminal");
    let mut first = [0u8; MAX_STREAM_BYTES];
    let mut second = [0u8; MAX_STREAM_BYTES];
    let a = recorder.serialize(&mut first).expect("first");
    let b = recorder.serialize(&mut second).expect("second");
    assert_eq!(a, b);
    assert_eq!(first[..a], second[..b]);
}

/// A replay that asks for the wrong input kind is refused rather than allowed to
/// skip ahead to a matching record: skipping would reorder the inputs, which is
/// the divergence the ordering rules exist to prevent.
#[test]
fn a_replay_cannot_skip_an_input_to_find_a_matching_one() {
    let mut recorder = Recorder::new(8);
    recorder.timer_expiry(3).expect("timer");
    recorder.clock_read(CLOCK_MONOTONIC, 11).expect("clock");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let mut replay = Replay::open(&bytes[..written], 8).expect("opens");
    // The next record is the timer, so a clock read must be refused even though
    // one exists later in the stream.
    assert_eq!(
        replay.clock_read(CLOCK_MONOTONIC),
        Err(RecordingError::Exhausted)
    );
}

/// A recorded clock read of one source cannot be replayed as the other. The two
/// are different determinism claims: the simulated clock moves only when a
/// declared advancer moves it, while the monotonic counter is the hardware's.
#[test]
fn a_monotonic_read_cannot_be_replayed_as_a_simulated_one() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 11).expect("clock");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let mut replay = Replay::open(&bytes[..written], 8).expect("opens");
    assert_eq!(
        replay.clock_read(CLOCK_SIMULATED),
        Err(RecordingError::Exhausted)
    );
}

/// A truncated stream is refused whole, and nothing is exposed. This is C9.5's
/// third required check: every prefix of a valid recording must fail, because a
/// replayer that consumed part of one has produced output nobody can compare.
#[test]
fn every_truncation_is_refused_rather_than_partially_replayed() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.timer_expiry(2).expect("timer");
    recorder.lifecycle(4).expect("lifecycle");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    for length in 0..written {
        let error = Replay::open(&bytes[..length], 8).expect_err("refuses a prefix");
        assert!(
            matches!(error, RecordingError::BadLength | RecordingError::Truncated),
            "prefix of {length} bytes gave {error:?}"
        );
    }
    Replay::open(&bytes[..written], 8).expect("the whole stream opens");
}

/// A reordered stream is refused whole. Swapping two records that a valid
/// recording emitted in order must fail, or replay would answer inputs in an
/// order the recorded run never observed.
#[test]
fn a_reordered_stream_is_refused() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.advance(5).expect("clock advances");
    recorder.timer_expiry(2).expect("timer");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    Replay::open(&bytes[..written], 8).expect("in order, it opens");

    let mut swapped = bytes;
    let (first, rest) = swapped.split_at_mut(RECORD_BYTES);
    first.swap_with_slice(&mut rest[..RECORD_BYTES]);
    assert_eq!(
        Replay::open(&swapped[..written], 8).unwrap_err(),
        RecordingError::Reordered
    );
}

/// A stream with no terminal record is refused, so a complete recording and one
/// cut short cannot read alike.
#[test]
fn a_stream_without_a_terminal_is_refused() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    assert_eq!(
        Replay::open(&bytes[..written], 8).unwrap_err(),
        RecordingError::Truncated
    );
}

/// A record after the terminal is refused: such a stream was appended to after
/// being declared complete, and replaying the tail would replay inputs the
/// recorder never claimed to have finished capturing.
#[test]
fn a_record_after_the_terminal_is_refused() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    // Append a well-formed record after the terminal, stamped so the declared
    // order still holds — the stream is refused for the terminal's position
    // alone, not for being out of order.
    let extra = WireTraceRecord {
        magic: TRACE_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_OUTPUT,
        flags: 0,
        route_identity: 0,
        correlation: 8,
        sequence: 9,
        now_ns: 0,
        status: 0,
        event: 1,
        high_water: 0,
        order_class: 4,
        reserved: [0; 3],
    };
    bytes[written..written + RECORD_BYTES].copy_from_slice(&extra.encode());
    assert_eq!(
        Replay::open(&bytes[..written + RECORD_BYTES], 8).unwrap_err(),
        RecordingError::Malformed
    );
}

/// The declared capacity is refused structurally, at both ends. This is C9.5's
/// fourth required check.
#[test]
fn the_record_capacity_is_refused_structurally() {
    // A recorder stops at its declared capacity rather than growing, and counts
    // the refusal so a short recording is distinguishable from a complete one.
    let mut recorder = Recorder::new(2);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("first");
    recorder.timer_expiry(2).expect("second");
    assert_eq!(
        recorder.clock_read(CLOCK_MONOTONIC, 3),
        Err(RecordingError::Full)
    );
    assert_eq!(recorder.len(), 2);
    assert_eq!(recorder.refused(), 1);

    // A replayer refuses a stream longer than the generation declared, before
    // decoding a single input.
    let mut full = Recorder::new(4);
    full.clock_read(CLOCK_MONOTONIC, 1).expect("first");
    full.timer_expiry(2).expect("second");
    full.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = full.serialize(&mut bytes).expect("serialize");
    assert_eq!(
        Replay::open(&bytes[..written], 2).unwrap_err(),
        RecordingError::BadLength
    );
    Replay::open(&bytes[..written], 3).expect("at the declared capacity it opens");

    // A capacity above the format ceiling is refused rather than clamped on the
    // consuming side: the bound is authenticated, so a larger one means the
    // caller was answered a number no admitted resource could hold.
    assert_eq!(
        Replay::open(&bytes[..written], MAX_RECORD_CAPACITY + 1).unwrap_err(),
        RecordingError::BadLength
    );
}

/// A byte length that is not a whole number of records is refused, so truncation
/// and completeness are never the same observation.
#[test]
fn a_partial_record_is_refused() {
    let mut recorder = Recorder::new(4);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    assert_eq!(
        Replay::open(&bytes[..written - 1], 4).unwrap_err(),
        RecordingError::BadLength
    );
}

/// A recorder's clock cannot retreat: the declared order would refuse the next
/// record anyway, and failing at the advance names the real defect.
#[test]
fn a_retreating_clock_is_refused_at_the_advance() {
    let mut recorder = Recorder::new(4);
    recorder.advance(10).expect("forward");
    assert_eq!(recorder.advance(9), Err(RecordingError::OutOfOrder));
    // The clock did not move, so records that follow still carry the instant the
    // recorder had actually reached.
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    recorder.terminal().expect("terminal");
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let replay = Replay::open(&bytes[..written], 4).expect("opens");
    assert_eq!(replay.len(), 2);
}

/// An out-of-vocabulary event is refused at emission rather than serialized: a
/// record whose event the format does not bound is not evidence a reader can
/// compare across runs.
#[test]
fn an_event_outside_the_declared_vocabulary_is_refused() {
    let mut recorder = Recorder::new(8);
    assert_eq!(
        recorder.lifecycle(MAX_LIFECYCLE_STATE + 1),
        Err(RecordingError::Malformed)
    );
    assert_eq!(
        recorder.output(MAX_OUTPUT_CHANNEL + 1, 1),
        Err(RecordingError::Malformed)
    );
    assert_eq!(recorder.lifecycle(0), Err(RecordingError::Malformed));
    assert_eq!(recorder.len(), 0);
}

/// A timer identity of zero is a real expiry, not a malformed record.
///
/// `docs/syscall-abi.md` documents `CLOCK TIMER ARM`'s primary result as "an
/// opaque timer id; zero is valid", and the root assigns exactly that to a
/// holder's first timer. An earlier revision of the validator required a nonzero
/// identity here on the theory that zero names no timer, and it refused the first
/// real expiry the plane's recorder observed — a booted `[replay] FAIL record
/// timer expiry: malformed`. This pins the corrected rule against that
/// regression.
#[test]
fn a_zero_timer_identity_is_a_real_expiry() {
    let mut recorder = Recorder::new(4);
    recorder.timer_expiry(0).expect("the first timer's id is 0");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let mut replay = Replay::open(&bytes[..written], 4).expect("opens");
    assert_eq!(replay.timer_expiry().expect("expiry"), 0);
}

/// A replay that stops before the terminal is refused: a prefix of a
/// deterministic run is not the run.
#[test]
fn finishing_early_is_refused() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.timer_expiry(2).expect("timer");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let mut replay = Replay::open(&bytes[..written], 8).expect("opens");
    assert_eq!(replay.clock_read(CLOCK_MONOTONIC).expect("clock"), 1);
    // The timer record is still unconsumed.
    assert_eq!(replay.finish(), Err(RecordingError::Exhausted));
}

/// A malformed record inside an otherwise valid stream is refused whole.
#[test]
fn a_malformed_record_refuses_the_whole_stream() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    recorder.timer_expiry(2).expect("timer");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    // Corrupt the second record's magic. The first is still valid, which is the
    // point: a replayer must not hand out the first input before discovering the
    // second is unreadable.
    bytes[RECORD_BYTES] ^= 0xff;
    assert_eq!(
        Replay::open(&bytes[..written], 8).unwrap_err(),
        RecordingError::Malformed
    );
}

/// The kinds C9.5 added carry the fields its families declare, and the shared
/// validator enforces them. A clock read with a route identity, or an output with
/// a status, is not a record two runs could compare.
#[test]
fn the_recording_families_fix_every_field_they_do_not_use() {
    let base = WireTraceRecord {
        magic: TRACE_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_CLOCK_READ,
        flags: 0,
        route_identity: 0,
        correlation: 5,
        sequence: 0,
        now_ns: 0,
        status: 0,
        event: CLOCK_MONOTONIC,
        high_water: 0,
        order_class: 1,
        reserved: [0; 3],
    };
    assert!(slime_proto::valid_trace_record(&base));
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        route_identity: 1,
        ..base
    }));
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        status: -1,
        ..base
    }));
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        high_water: 1,
        ..base
    }));
    // An unnamed clock is not evidence: a reader could not tell which clock
    // answered, so two runs reading different clocks would compare as equal.
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        event: 0,
        ..base
    }));
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        kind: KIND_OUTPUT,
        event: MAX_OUTPUT_CHANNEL + 1,
        ..base
    }));
    // The terminal flag belongs to the resource family, so a recording family
    // carrying it would make the end of the stream ambiguous.
    assert!(!slime_proto::valid_trace_record(&WireTraceRecord {
        flags: FLAG_TERMINAL,
        ..base
    }));
}
/// A terminal-flagged record of the wrong shape is refused at `open`, before any
/// input is exposed.
///
/// `valid_trace_record` permits `FLAG_TERMINAL` on any `KIND_RESOURCE` counter,
/// so a stream ending in a terminal-flagged *frames* record used to open here and
/// hand out every preceding input before `finish` noticed (found by review). The
/// canonical terminal is one shape, not a family.
#[test]
fn a_noncanonical_terminal_is_refused_before_any_input_is_exposed() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 11).expect("clock");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    Replay::open(&bytes[..written], 8).expect("the canonical terminal opens");

    // Rewrite the terminal's counter to another declared resource code. The
    // record stays well formed and still carries the terminal flag.
    let terminal_start = written - RECORD_BYTES;
    let mut terminal = WireTraceRecord::decode(&bytes[terminal_start..written]).expect("decode");
    assert!(slime_proto::valid_trace_record(&terminal));
    terminal.event = slime_proto::fabric_trace::RESOURCE_FRAMES;
    assert!(
        slime_proto::valid_trace_record(&terminal),
        "the shared validator must still accept it, or this test proves nothing"
    );
    bytes[terminal_start..written].copy_from_slice(&terminal.encode());
    assert_eq!(
        Replay::open(&bytes[..written], 8).unwrap_err(),
        RecordingError::Malformed
    );
}

/// A refused replay step leaves the position untouched.
///
/// `clock_read` used to consume the record and then check its source, so a caller
/// asking for the wrong clock advanced the cursor anyway and any recovery
/// continued from a shifted stream (found by review).
#[test]
fn a_refused_clock_read_does_not_move_the_cursor() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 11).expect("clock");
    recorder.timer_expiry(4).expect("timer");
    recorder.terminal().expect("terminal");
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    let mut replay = Replay::open(&bytes[..written], 8).expect("opens");
    let before = replay.remaining();
    assert_eq!(
        replay.clock_read(CLOCK_SIMULATED),
        Err(RecordingError::Exhausted)
    );
    assert_eq!(
        replay.remaining(),
        before,
        "a refused step consumed a record"
    );
    // And the stream is still replayable from exactly where it was.
    assert_eq!(replay.clock_read(CLOCK_MONOTONIC).expect("clock"), 11);
    assert_eq!(replay.timer_expiry().expect("timer"), 4);
    replay.finish().expect("terminal");
}

/// A recorder is single-use: nothing may follow its terminal.
///
/// Without this the recorder could serialize a stream its own `Replay::open`
/// refuses for having records past its terminal — a defect discovered one process
/// away from where it was made (found by review).
#[test]
fn a_closed_recorder_refuses_every_further_record() {
    let mut recorder = Recorder::new(8);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    assert!(!recorder.is_closed());
    recorder.terminal().expect("terminal");
    assert!(recorder.is_closed());
    assert_eq!(recorder.advance(10), Err(RecordingError::Closed));
    assert_eq!(
        recorder.clock_read(CLOCK_MONOTONIC, 2),
        Err(RecordingError::Closed)
    );
    assert_eq!(recorder.timer_expiry(3), Err(RecordingError::Closed));
    assert_eq!(recorder.lifecycle(5), Err(RecordingError::Closed));
    assert_eq!(recorder.output(1, 9), Err(RecordingError::Closed));
    assert_eq!(recorder.terminal(), Err(RecordingError::Closed));
    // The stream is exactly what it was when it closed, and it still replays.
    assert_eq!(recorder.len(), 2);
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = recorder.serialize(&mut bytes).expect("serialize");
    Replay::open(&bytes[..written], 8).expect("a closed recorder's stream opens");
}

/// A recorder whose terminal was refused for want of capacity stays usable, so it
/// can report that rather than being locked out of its own recorder.
#[test]
fn a_refused_terminal_does_not_close_the_recorder() {
    let mut recorder = Recorder::new(1);
    recorder.clock_read(CLOCK_MONOTONIC, 1).expect("clock");
    assert_eq!(recorder.terminal(), Err(RecordingError::Full));
    assert!(!recorder.is_closed());
    assert_eq!(recorder.refused(), 1);
}
