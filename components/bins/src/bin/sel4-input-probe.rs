#![no_std]
#![no_main]

//! The seL4 input plane's subject: `InputRead` mediation (P5.4.3).
//!
//! Small on purpose. M6.4's Dango session is the *consumer* of key events, and
//! it is a large composition with its own failure modes; this plane asserts the
//! mechanism underneath it, so a defect in the authority path is distinguishable
//! from a defect in the shell.
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
const EMPTY_SLOT: u32 = 0;

/// What the plane's script types, in order. Kept in step with
/// `slime-root/src/main.rs::input_script`.
const EXPECTED: &[u8] = b"ab c\n";

slime_rt::entry!(main);

fn main(startup_arg: u32) {
    if startup_arg == 0 {
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
