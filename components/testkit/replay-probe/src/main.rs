#![no_std]
#![no_main]

//! C9.5's plane: a recorded run, a deterministic replay of it, and the two
//! refusals that make the determinism claim mean something.
//!
//! Five instances of this binary, told apart by what the generation declares
//! about each — the same shape `clock-authority-probe` and `wait-set-probe` use,
//! and for the same reason: what a component may do is authenticated generation
//! data, so the role is discovered rather than compiled in. Here the discovery is
//! exact rather than inferred, because C9.5 added an operation that answers it:
//! `recording_participation()` reports this instance's own role, its declared
//! record capacity, and whether the generation claims it deterministic.
//!
//! - `replay-recorder` records its own clock reads, timer expiry, and lifecycle
//!   transitions, produces typed outputs from them, and streams the recording to
//!   its peer. It is *not* declared deterministic: capturing inputs faithfully is
//!   not the same as being reproducible, and the generation says so.
//! - `replay-replayer` is declared deterministic. It receives the recording,
//!   validates it whole, replays every input, recomputes the same outputs from
//!   them, and compares field by field against the outputs the recording carries.
//!   Two boots of this plane must produce byte-identical `[replay]` output lines.
//! - `replay-unrecorded` holds a `blockRead` grant, which
//!   `contracts/generation/v5` classifies as an unrecorded nondeterminism source.
//!   The generation therefore *cannot* declare it deterministic — admission
//!   refuses that — so it reads back `deterministic=0` and reports its own
//!   inadmissibility.
//! - `replay-observer` is the unrecorded stream's declared replayer, present so
//!   that stream is paired: a stream with one end would not decode.
//! - `replay-unnamed` is named by no recording entry. It reads role `none`, which
//!   is the deny-by-default answer rather than a refusal, and is what lets one
//!   component image run in a generation that records it and one that does not.
//!
//! # Why the stream crosses as messages rather than a shared buffer
//!
//! One record is exactly `MAX_MSG` bytes, so a record is a message. A shared
//! buffer would add a mapping, a seal, and a loan to a plane whose subject is the
//! recording's *content*, and the C7 plane already proves that machinery. What
//! matters here is that the replayer validates the bytes it was handed before
//! consuming any of them, and that is independent of how they arrived.

use boot_contracts::generation::RIGHT_RECV;
use slime_components::block_io::BlockIo;
use slime_proto::block_v2;
use slime_proto::recording_stream::{
    MAX_STREAM_BYTES, RECORD_BYTES, Recorder, RecordingError, Replay,
};
use slime_rt::{
    ERR_SUCCESS, MAX_CAPS_PER_MSG, MAX_MSG, RecordingRole, debug_write, exit, monotonic_read,
    notification_wait, recording_participation, simulated_time_advance, simulated_time_read,
    timer_arm, yield_now,
};

slime_rt::entry!(main);

/// The declared endpoint between the recorder and the replayer.
const PEER_SLOT: u32 = 0;
/// The unrecorded holder's peer endpoint and shared-buffer factory for its IO0 ring.
const BLOCK_PEER_SLOT: u32 = 8;
const BLOCK_FACTORY_SLOT: u32 = 3;
const BLOCK_RING_BASE: u64 = 0x0000_001f_0000_0000;
const BLOCK_DATA_BASE: u64 = 0x0000_001f_0001_0000;
/// The recorder's declared wait binding on `replay-recorder-tick`, which carries
/// its C9.1 timer expiry.
const TICK_SLOT: u32 = 0;
/// The declared timer badge from the generation's clock authority.
const TIMER_BADGE: u64 = 1 << 9;
/// Spins the replayer allows the recorder before concluding a missing stream.
const SETTLE_SPINS: usize = 2_000_000;
/// The relative timer delay the recorder arms. Short enough to expire inside the
/// gate's watchdog, long enough that the arm and the expiry are distinct events.
const TIMER_DELAY: u64 = 1_000_000;
/// The declared simulated-time step the recorder advances by.
///
/// The simulated clock moves only when its declared advancer moves it, so this
/// number — not the hardware — is what makes the recorded reads, and therefore
/// every derived output, identical across two boots of one composition.
const SIMULATED_STEP: u64 = 4_096;

/// The recorder's declared output channels. Two, because a single channel could
/// not catch a replay that produced the right values in the wrong places.
const CHANNEL_ELAPSED: u32 = 1;
const CHANNEL_STATE: u32 = 2;

/// The lifecycle state the recorder advances to, from
/// `contracts/lifecycle-policy/v1`'s frozen numbering. Recorded as an input
/// because a replayed component must see the same transition sequence.
const STATE_RUNNING: u32 = 5;

fn main(_startup_arg: u32) {
    let participation = match recording_participation() {
        Ok(participation) => participation,
        Err(error) => fail_with(b"recording participation", error),
    };
    match participation.role {
        Some(RecordingRole::Record) => {
            // Neither recorder is declared deterministic, and the two reach that
            // for different reasons: the stream recorder simply is not claimed,
            // while `replay-unrecorded` *cannot* be — it holds a `blockRead`
            // grant, and admission refuses a deterministic claim over an
            // unrecorded nondeterminism source. Asserted for both, because a
            // recorder reading back `deterministic=1` would mean that refusal did
            // not run.
            if participation.deterministic {
                fail(b"a recorder was admitted as deterministic")
            }
            // The two are told apart by authority rather than by name: the stream
            // recorder is the one instance the generation grants a timer, so a
            // timer it cannot arm means this is the unrecorded-source instance.
            // Discovering the role from the grant keeps the plane's roles
            // authenticated data rather than a compiled-in table (B70).
            match timer_arm(TIMER_DELAY) {
                Ok(timer) => run_recorder(participation.record_capacity, timer),
                Err(_) => run_unrecorded(participation.record_capacity),
            }
        }
        Some(RecordingRole::Replay) => {
            if participation.deterministic {
                run_replayer(participation.record_capacity)
            } else {
                // `replay-observer`: the unrecorded stream's declared peer,
                // present so that stream is paired. It asserts what it can — that
                // its own claim is absent — and exits without a stream, because
                // the recorder on its stream never sends one.
                report_undeclared(b"observer", participation.record_capacity)
            }
        }
        None => run_unnamed(),
    }
}

/// A recorder the generation cannot declare deterministic, because its IO0
/// ring carries block-read authority classified as an unrecorded
/// nondeterminism source.
///
/// This is C9.5's second required check observed from inside: the manifest could
/// ask for `deterministic = true` here and the build would refuse it, so what
/// runs reads back an absent claim. The instance proves the authority is real by
/// reading a sector through the userspace driver, so the refusal is about an
/// authority it holds rather than one nobody has.
fn run_unrecorded(capacity: u32) -> ! {
    let request_ready = binding(b"notification:io-block-request-ready+signal");
    let completion_ready = binding(b"notification:io-block-completion-ready+wait");
    // SAFETY: both bases are page-aligned addresses in this component's free
    // VSpace range, do not alias each other, and nothing else maps them.
    let mut io = unsafe {
        BlockIo::attach(
            BLOCK_FACTORY_SLOT,
            BLOCK_PEER_SLOT,
            request_ready,
            completion_ready,
            BLOCK_RING_BASE,
            BLOCK_DATA_BASE,
        )
    }
    .unwrap_or_else(|_| fail(b"unrecorded block attach"));
    let mut sector = [0u8; block_v2::SECTOR_BYTES];
    io.read(0, &mut sector)
        .unwrap_or_else(|_| fail(b"unrecorded block read"));
    write_pair(
        b"[replay:unrecorded] role=record capacity=",
        capacity as u64,
        b" claim=",
        0,
    );
    debug_write(b"[replay:unrecorded] unrecorded source held\n");
    io.shutdown()
        .unwrap_or_else(|_| fail(b"unrecorded driver shutdown"));
    exit(0)
}

/// Record every input this component observes, derive outputs from them, and
/// stream the recording to the declared peer.
///
/// `timer` is the identity of the timer already armed by the role discovery in
/// [`main`]: arming one is how this instance proved it holds the clock authority
/// the stream recorder is granted, so arming a second would be a live timer with
/// no purpose against a declared quota.
fn run_recorder(capacity: u32, timer: u64) -> ! {
    let mut recorder = Recorder::new(capacity as usize);

    // A monotonic read. The value is whatever the hardware answered, which is
    // exactly why it must be recorded: the replay cannot obtain it again. It is
    // *not* an output, for the reason the outputs below explain.
    let observed = monotonic_read().unwrap_or_else(|error| fail_with(b"monotonic read", error));
    step(recorder.advance(observed), b"advance to the observed read");
    step(
        recorder.clock_read(slime_proto::fabric_trace::CLOCK_MONOTONIC, observed),
        b"record the monotonic read",
    );

    // The expiry of that timer, delivered on the declared badge. Recorded by the
    // identity the root assigned, so a replay resolves *which* timer fired rather
    // than assuming one outstanding.
    await_expiry();
    step(recorder.timer_expiry(timer), b"record timer expiry");

    // Two *simulated* reads around a declared advance, and these are what the
    // outputs are computed from.
    //
    // The distinction is the whole reason both clocks appear here. A monotonic
    // read is a hardware instant: two boots observe different values by
    // construction, so an output derived from one could not be compared byte for
    // byte across boots — the very property C9.5's first required check asks for.
    // The simulated clock only moves when its declared advancer moves it, so two
    // boots of one composition see the same values, and an output derived from
    // them is reproducible. Recording the hardware read anyway is deliberate: it
    // proves the recording carries an input the replay genuinely cannot obtain,
    // and the replayer checks that it *received* it without letting it reach an
    // output.
    let before = simulated_time_read().unwrap_or_else(|error| fail_with(b"simulated read", error));
    step(
        recorder.clock_read(slime_proto::fabric_trace::CLOCK_SIMULATED, before),
        b"record the first simulated read",
    );
    simulated_time_advance(SIMULATED_STEP)
        .unwrap_or_else(|error| fail_with(b"simulated advance", error));
    let after = simulated_time_read().unwrap_or_else(|error| fail_with(b"simulated read", error));
    if after <= before {
        fail(b"the simulated clock did not advance")
    }
    step(
        recorder.clock_read(slime_proto::fabric_trace::CLOCK_SIMULATED, after),
        b"record the second simulated read",
    );

    // A lifecycle transition the generation admits. Recorded so a replayed
    // component observes the same sequence of states.
    let state = slime_rt::lifecycle_state_advance(STATE_RUNNING)
        .unwrap_or_else(|error| fail_with(b"lifecycle advance", error));
    step(recorder.lifecycle(state), b"record lifecycle transition");

    // The outputs, derived from the recorded *simulated* inputs alone. That
    // derivation is the deterministic function under test: the replayer computes
    // the same one from the same recorded inputs, and two boots must agree on
    // every field.
    let elapsed = derive_elapsed(before, after);
    let encoded_state = derive_state(state);
    step(
        recorder.output(CHANNEL_ELAPSED, elapsed),
        b"record elapsed output",
    );
    step(
        recorder.output(CHANNEL_STATE, encoded_state),
        b"record state output",
    );
    step(recorder.terminal(), b"record terminal");

    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let written = match recorder.serialize(&mut bytes) {
        Ok(written) => written,
        Err(error) => fail_recording(b"serialize", error),
    };
    write_pair(
        b"[replay:recorder] recorded records=",
        recorder.len() as u64,
        b" bytes=",
        written as u64,
    );
    write_pair(
        b"[replay:recorder] outputs elapsed=",
        elapsed,
        b" state=",
        encoded_state,
    );
    if recorder.refused() != 0 {
        fail(b"recorder refused a record inside its declared capacity")
    }

    // One record per message: a record is exactly `MAX_MSG` bytes, so the stream
    // needs no framing of its own. The peer counts records rather than trusting a
    // declared length, because a length it was told is not a length it verified.
    for index in 0..written / RECORD_BYTES {
        let start = index * RECORD_BYTES;
        let mut frame = [0u8; MAX_MSG];
        frame.copy_from_slice(&bytes[start..start + RECORD_BYTES]);
        if slime_rt::send(PEER_SLOT, &frame, &[]) != ERR_SUCCESS {
            fail(b"stream send")
        }
    }
    // C9.5's runtime half, observed rather than argued. Admission certifies the
    // replayer deterministic against the authority the *generation* declares, and
    // a transfer could otherwise widen it afterwards: the claim would stay
    // authenticated and stop being true. So this deliberately offers the peer a
    // capability carrying `recv` — an unrecorded source — and the root must refuse
    // the receiver's import of it.
    //
    // The recorder attempts the delegation and the replayer attempts the import;
    // neither can succeed, and each prints what it observed. Offering it from
    // here rather than asserting the rule in a comment is the difference between
    // a gate that checks the mechanism and one that checks the docstring.
    let mut descriptor = [0u8; 64];
    descriptor[0] = 1;
    let offered = slime_rt::capability_delegate(
        PEER_SLOT,
        PEER_SLOT,
        slime_rt::CapabilityDisposition::Retain,
        slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        RIGHT_RECV,
        &descriptor,
    );
    write_pair(
        b"[replay:recorder] offered unrecorded=",
        u64::from(offered == ERR_SUCCESS),
        b" status=",
        offered.unsigned_abs(),
    );
    debug_write(b"[replay:recorder] streamed\n");
    exit(0)
}

/// Receive a recording, validate it whole, replay every input, and compare the
/// outputs recomputed from those inputs against the ones the recording carries.
fn run_replayer(capacity: u32) -> ! {
    let mut bytes = [0u8; MAX_STREAM_BYTES];
    let bound = capacity as usize * RECORD_BYTES;
    if bound > bytes.len() {
        // The declared capacity is refused by the decoder and by admission before
        // this point, so reaching here means the root answered a capacity no
        // admitted resource could hold.
        fail(b"declared capacity exceeds the format ceiling")
    }
    let received = receive_stream(&mut bytes[..bound]);
    write_pair(
        b"[replay:replayer] received records=",
        (received / RECORD_BYTES) as u64,
        b" bound=",
        bound as u64,
    );

    // Two negative controls first, on the bytes actually received, because both
    // are properties of *this* stream rather than of a synthetic one. A truncated
    // stream and a reordered one must each be refused before a single input is
    // handed out, which is what "refused rather than partially replayed" means.
    if received > RECORD_BYTES {
        match Replay::open(&bytes[..received - RECORD_BYTES], capacity as usize) {
            Err(RecordingError::Truncated) => {
                debug_write(b"[replay:replayer] truncated refused\n");
            }
            Err(error) => fail_recording(b"truncation gave the wrong refusal", error),
            Ok(_) => fail(b"a truncated stream opened"),
        }
        let mut swapped = bytes;
        let (first, rest) = swapped.split_at_mut(RECORD_BYTES);
        first.swap_with_slice(&mut rest[..RECORD_BYTES]);
        match Replay::open(&swapped[..received], capacity as usize) {
            Err(RecordingError::Reordered) => {
                debug_write(b"[replay:replayer] reordered refused\n");
            }
            Err(error) => fail_recording(b"reordering gave the wrong refusal", error),
            Ok(_) => fail(b"a reordered stream opened"),
        }
    }

    // The declared capacity is refused structurally: a stream longer than the
    // generation admits is rejected before any record is decoded.
    let records = received / RECORD_BYTES;
    if records > 1 {
        match Replay::open(&bytes[..received], records - 1) {
            Err(RecordingError::BadLength) => {
                debug_write(b"[replay:replayer] over-capacity refused\n");
            }
            Err(error) => fail_recording(b"over-capacity gave the wrong refusal", error),
            Ok(_) => fail(b"a stream longer than its capacity opened"),
        }
    }

    let mut replay = match Replay::open(&bytes[..received], capacity as usize) {
        Ok(replay) => replay,
        Err(error) => fail_recording(b"open", error),
    };

    // Replay the inputs in order. Each is answered from the recording rather than
    // from the live source — this component holds no clock authority at all, so a
    // replay that reached for one would be refused rather than silently
    // succeeding with a different value.
    //
    // The monotonic read is consumed and checked but never reaches an output: it
    // is a hardware instant, so two boots recorded different values, and it is
    // here to prove the recording carries an input the replay could not have
    // obtained. The outputs come from the simulated pair, which two boots of one
    // composition observe identically.
    let observed = match replay.clock_read(slime_proto::fabric_trace::CLOCK_MONOTONIC) {
        Ok(value) => value,
        Err(error) => fail_recording(b"replay the monotonic read", error),
    };
    let timer = match replay.timer_expiry() {
        Ok(timer) => timer,
        Err(error) => fail_recording(b"replay timer expiry", error),
    };
    let before = match replay.clock_read(slime_proto::fabric_trace::CLOCK_SIMULATED) {
        Ok(value) => value,
        Err(error) => fail_recording(b"replay the first simulated read", error),
    };
    let after = match replay.clock_read(slime_proto::fabric_trace::CLOCK_SIMULATED) {
        Ok(value) => value,
        Err(error) => fail_recording(b"replay the second simulated read", error),
    };
    let state = match replay.lifecycle() {
        Ok(state) => state,
        Err(error) => fail_recording(b"replay lifecycle transition", error),
    };

    // This component holds no clock, so every live read must be refused. Without
    // this the replay could be reading the hardware and agreeing by luck.
    if monotonic_read().is_ok() || simulated_time_read().is_ok() {
        fail(b"the replayer holds live clock authority")
    }
    // The recorded hardware instant must be a real observation rather than a
    // zero the recorder never wrote, and the simulated pair must show the
    // declared advance.
    if observed == 0 {
        fail(b"the recorded monotonic read was never observed")
    }
    if after != before + SIMULATED_STEP {
        fail(b"the recorded simulated reads do not show the declared advance")
    }

    // Recompute the outputs from the replayed inputs with the same function the
    // recorder used, then compare against what the recording carries — field by
    // field, in order, following C8.15's semantic-comparison pattern.
    let recomputed_elapsed = derive_elapsed(before, after);
    let recomputed_state = derive_state(state);
    let (elapsed_channel, recorded_elapsed) = match replay.output() {
        Ok(output) => output,
        Err(error) => fail_recording(b"replay elapsed output", error),
    };
    let (state_channel, recorded_state) = match replay.output() {
        Ok(output) => output,
        Err(error) => fail_recording(b"replay state output", error),
    };
    if elapsed_channel != CHANNEL_ELAPSED || state_channel != CHANNEL_STATE {
        fail(b"replayed outputs arrived on the wrong channels")
    }
    if recomputed_elapsed != recorded_elapsed || recomputed_state != recorded_state {
        fail(b"replayed outputs diverged from the recording")
    }
    if let Err(error) = replay.finish() {
        fail_recording(b"terminal", error)
    }
    // The receiving half of C9.5's runtime gate. The recorder offered a
    // capability carrying `recv`, an unrecorded source, and this instance is the
    // one the generation claims deterministic — so the root must refuse the
    // import rather than widen the claim it already certified. `ERR_BAD_CAP` is
    // the refusal; a success here would mean the gate is absent.
    let imported = slime_rt::capability_import();
    match imported {
        Ok(slot) => {
            write_pair(
                b"[replay:replayer] FAIL imported unrecorded authority into slot=",
                u64::from(slot),
                b" claim=",
                1,
            );
            exit(1)
        }
        Err(status) => write_pair(
            b"[replay:replayer] unrecorded import refused status=",
            status.unsigned_abs(),
            b" expected=",
            slime_rt::ERR_BAD_CAP.unsigned_abs(),
        ),
    }

    // The comparison's inputs are printed as well as its verdict. A gate that saw
    // only "matched" could not tell a real agreement from two zeros.
    write_pair(
        b"[replay:replayer] inputs first=",
        before,
        b" second=",
        after,
    );
    write_pair(
        b"[replay:replayer] inputs timer=",
        timer,
        b" state=",
        u64::from(state),
    );
    write_pair(
        b"[replay:replayer] outputs elapsed=",
        recomputed_elapsed,
        b" state=",
        recomputed_state,
    );
    debug_write(b"[replay:replayer] matched\n");
    exit(0)
}

/// An instance the recording resource does not name.
fn run_unnamed() -> ! {
    // Deny by default, and the answer is a role rather than an error: being
    // absent from the resource is a fact about this instance, not a refusal.
    debug_write(b"[replay:unnamed] role=none capacity=0 deterministic=0\n");
    exit(0)
}

/// An instance the resource names without a determinism claim.
///
/// Both `replay-observer` and — were the generation to try it — a component
/// holding an unrecorded source land here. The distinction the marker carries is
/// that the claim is *absent*, which for `replay-unrecorded` is admission's
/// verdict rather than the manifest's preference.
fn report_undeclared(role: &[u8], capacity: u32) -> ! {
    debug_write(b"[replay:");
    debug_write(role);
    write_pair(b"] role=replay capacity=", capacity as u64, b" claim=", 0);
    exit(0)
}

/// The elapsed-time output: a pure function of two recorded clock reads.
///
/// Deliberately a *difference* rather than either raw value. A replay that
/// answered a constant for both reads would produce the same difference as one
/// that answered the recorded values, so the recorded values are printed too and
/// the gate compares those across boots as well.
const fn derive_elapsed(first: u64, second: u64) -> u64 {
    second - first
}

/// The state output: a pure function of the recorded lifecycle transition.
const fn derive_state(state: u32) -> u64 {
    state as u64 * 7 + 1
}

/// Block until the declared timer badge arrives.
fn await_expiry() {
    for _ in 0..SETTLE_SPINS {
        match notification_wait(TICK_SLOT) {
            Ok(badge) if badge & TIMER_BADGE != 0 => return,
            Ok(_) => {}
            Err(error) => fail_with(b"tick wait", error),
        }
    }
    fail(b"declared timer never expired")
}

/// Collect the stream one record per message, stopping when the peer stops.
///
/// Returns the byte count received. A short stream is not diagnosed here: it is
/// the replayer's job to refuse an incomplete recording, and diagnosing it at the
/// transport would make the refusal a property of the transport rather than of
/// the recording.
///
/// An *over-long* stream is diagnosed here, and that asymmetry is the point.
/// Filling the declared bound and returning the prefix would hand
/// `Replay::open` a stream that looks complete — a valid terminal inside the
/// bound, with the sender's surplus records never observed — so the structural
/// ceiling would be satisfied by truncation rather than enforced (found by
/// review). Once the bound is full the receiver therefore polls once more: a
/// record still waiting means the sender exceeded what the generation declared.
fn receive_stream(out: &mut [u8]) -> usize {
    let mut received = 0;
    let mut idle = 0;
    while received + RECORD_BYTES <= out.len() {
        match receive_record(out, received) {
            Some(next) => {
                received = next;
                idle = 0;
            }
            None => {
                idle += 1;
                if idle >= SETTLE_SPINS {
                    break;
                }
                yield_now();
            }
        }
    }
    if received == 0 {
        fail(b"no stream arrived")
    }
    // The bound is full, so anything still queued is past the declared capacity.
    // Non-blocking: `recv` answers `ERR_WOULDBLOCK` when the peer has finished.
    if received + RECORD_BYTES > out.len() {
        let mut frame = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        if slime_rt::recv(PEER_SLOT, &mut frame, &mut caps) > 0 {
            fail(b"the sender streamed past the declared capacity")
        }
    }
    received
}

/// Receive one record into `out` at `offset`, or `None` when nothing arrived.
fn receive_record(out: &mut [u8], offset: usize) -> Option<usize> {
    let mut frame = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let result = slime_rt::recv(PEER_SLOT, &mut frame, &mut caps);
    if result == RECORD_BYTES as i64 {
        out[offset..offset + RECORD_BYTES].copy_from_slice(&frame[..RECORD_BYTES]);
        return Some(offset + RECORD_BYTES);
    }
    if result > 0 {
        fail(b"stream frame was not one record")
    }
    None
}

fn binding(name: &[u8]) -> u32 {
    slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"notification binding"))
}

fn step(result: Result<(), RecordingError>, reason: &[u8]) {
    if let Err(error) = result {
        fail_recording(reason, error)
    }
}

fn write_pair(prefix: &[u8], first: u64, middle: &[u8], second: u64) {
    let mut digits = [0u8; 20];
    debug_write(prefix);
    debug_write(decimal(first, &mut digits));
    debug_write(middle);
    debug_write(decimal(second, &mut digits));
    debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        digits[0] = b'0';
        return &digits[..1];
    }
    let mut index = digits.len();
    let mut remaining = value;
    while remaining > 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail_recording(reason: &[u8], error: RecordingError) -> ! {
    debug_write(b"[replay] FAIL ");
    debug_write(reason);
    debug_write(b": ");
    debug_write(match error {
        RecordingError::Full => b"full",
        RecordingError::Malformed => b"malformed",
        RecordingError::OutOfOrder => b"out-of-order",
        RecordingError::BadLength => b"bad-length",
        RecordingError::Truncated => b"truncated",
        RecordingError::Reordered => b"reordered",
        RecordingError::Exhausted => b"exhausted",
        RecordingError::Closed => b"closed",
    });
    debug_write(b"\n");
    exit(1)
}

fn fail_with(reason: &[u8], error: i64) -> ! {
    let mut digits = [0u8; 20];
    debug_write(b"[replay] FAIL ");
    debug_write(reason);
    debug_write(b": ");
    debug_write(decimal(error.unsigned_abs(), &mut digits));
    debug_write(b"\n");
    exit(1)
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[replay] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
