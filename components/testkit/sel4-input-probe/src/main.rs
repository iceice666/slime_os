#![no_std]
#![no_main]

//! The seL4 input plane's subject: `InputRead` mediation (P5.4.3).
//!
//! Small on purpose. This plane isolates the input mediation mechanism from any
//! interactive language, so authority-path defects remain distinguishable from
//! shell or evaluator defects.
//!
//! Three claims:
//!
//! * a granted capability yields the generation's scripted keys, in order and
//!   decoded — so the encoding the root writes is the one the runtime reads;
//! * an exhausted script terminates the reader rather than blocking it, which
//!   is what stops a spent session from spinning on an always-ready wait;
//! * a slot holding no input capability is refused, so reading keys is
//!   authority rather than ambient.

use slime_rt::InputKey;

/// The input capability the generation grants this component.
const INPUT_SLOT: u32 = 1;
/// A slot holding no input capability, for the refusal arm.
///
/// Deliberately not the run-token slot: that one holds a real declared endpoint
/// on the instance that runs, so an `input_read` against it would be refused for
/// carrying the wrong *kind* rather than for holding nothing. Slot 2 is inside
/// the table's bounds and this component is granted nothing there.
const EMPTY_SLOT: u32 = 2;
/// The run token: init's declared edge to the instance that runs the scenario.
///
/// This is also the discriminator. The plane declares this executable twice —
/// the instance init spawns, and a root-owned `idle` instance whose whole point
/// is that input authority with no session reads nothing — and only the first is
/// ever sent on. Naming the capability is what distinguishes them; the
/// `startup_arg` that used to is delivered as zero to every non-bootstrap
/// instance, spawned or autostarted alike, so it could not.
const RUN_TOKEN_SLOT: u32 = 0;
/// Yields given up before concluding no run token will arrive.
///
/// Both instances hold a real endpoint here — the idle one a loopback nothing
/// ever sends on — so the idle instance always exhausts this bound, which makes
/// the number a latency rather than a safety margin. Small enough that its
/// `idle without a run token` marker precedes the spawned instance's work, and
/// large enough for the spawned instance, whose token init sends immediately
/// after the spawn returns.
const RUN_TOKEN_YIELDS: usize = 64;

/// What the plane's script types, in order. Kept in step with
/// `slime-root/src/main.rs::input_script`.
const EXPECTED: &[u8] = b"ab c\n";

slime_rt::entry!(main);

/// The generation declares this instance `autostart = false`, so the only copy
/// that runs is the one init spawned. The `startup_arg == 0` park that used to
/// stand here could not tell the two apart: the root delivers a nonzero boot
/// action only to the bootstrap instance, so a spawned child reads zero too and
/// parked instead of running the scenario.
///
/// The plane still declares a second instance of this executable, root-owned
/// and holding its own input capability. That one is the `idle` instance, and it
/// remains `autostart = true` — its whole point is that a component holding
/// input authority with no session reads nothing.
fn main(_startup_arg: u32) {
    // The run token, waited for with a bounded non-blocking receive.
    //
    // The idle instance is granted no endpoint at this slot, so its very first
    // `recv` is refused by the runtime before any invocation reaches the
    // kernel: `native_endpoint` resolves slot 0 inside the declared region, and
    // the root installed nothing there, so the receive returns an error rather
    // than faulting. The spawned instance's token is sent by init immediately
    // after the spawn returns.
    //
    // Bounded rather than blocking, because a native Endpoint with no sender is
    // indistinguishable from one whose sender has not spoken yet — and a
    // blocking receive on the idle instance would hang a `required` graph.
    let mut token = [0u8; slime_rt::MAX_MSG];
    let mut no_caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    let mut granted = false;
    for _ in 0..RUN_TOKEN_YIELDS {
        match slime_rt::recv(RUN_TOKEN_SLOT, &mut token, &mut no_caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => break,
            _ => {
                granted = true;
                break;
            }
        }
    }
    if !granted {
        slime_rt::debug_write(b"[sel4-input-probe] idle without a run token\n");
        slime_rt::exit(0);
    }

    // A slot with no input capability. Checked first, so a mechanism that
    // ignored the capability entirely could not pass the arms below by
    // accident.
    if slime_rt::input_read(EMPTY_SLOT).is_ok() {
        fail(b"an ungranted slot answered");
    }
    slime_rt::debug_write(b"[sel4-input-probe] ungranted slot refused\n");

    // The script, in order and decoded. A `pressed` field that never set would
    // make every event a release; a code the runtime maps differently would
    // make every character a space. Both happened, so both are compared here
    // rather than assumed.
    for expected in EXPECTED {
        let event = match slime_rt::input_read(INPUT_SLOT) {
            Ok(Some(event)) => event,
            Ok(None) => fail(b"the script ran short"),
            Err(_) => fail(b"input read"),
        };
        if !event.pressed {
            fail(b"an event arrived as a release");
        }
        let matched = match (event.key, *expected) {
            (InputKey::Character(character), byte) => character as u32 == u32::from(byte),
            (InputKey::Space, b' ') => true,
            (InputKey::Enter, b'\n') => true,
            _ => false,
        };
        if !matched {
            fail(b"the decoded key is not the scripted byte");
        }
    }
    slime_rt::debug_write(b"[sel4-input-probe] script decoded in order\n");

    // Exhausted. The source answers Escape rather than blocking, which is what
    // keeps a reader looping on `WouldBlock` from spinning forever against a
    // wait this source always satisfies.
    for _ in 0..3 {
        match slime_rt::input_read(INPUT_SLOT) {
            Ok(Some(event)) if event.key == InputKey::Escape => {}
            Ok(Some(_)) => fail(b"a spent script yielded a key"),
            Ok(None) => fail(b"a spent script blocked its reader"),
            Err(_) => fail(b"input read after exhaustion"),
        }
    }
    slime_rt::debug_write(b"[sel4-input-probe] exhausted script ends the reader\n");

    slime_rt::debug_write(b"[sel4-input-probe] input plane complete\n");
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[sel4-input-probe] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
