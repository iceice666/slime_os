#![no_std]
#![no_main]

//! Declared instance lifetime, observed from inside each instance.
//!
//! One image runs as every role in `sel4-lifetime` and `sel4-lifetime-exit`.
//! Each role is told apart by the authority its generation grants it, never by
//! its name. The owner holds its child's executable grant, and each resident
//! instance meant to exit is the target of a notification naming it. Every
//! other instance learns what it is by asking the root for its own lifetime.

use slime_rt::{Lifetime, Termination};

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if let Ok(executable) = slime_rt::resolve_binding(b"lifetime-quitter-executable") {
        run_owner(executable);
    }
    let lifetime = slime_rt::lifetime().unwrap_or_else(|_| fail(b"lifetime query"));
    match lifetime {
        Lifetime::Bounded => {
            slime_rt::debug_write(b"[lifetime-probe] role=worker lifetime=bounded\n");
            slime_rt::exit(0)
        }
        Lifetime::Resident => {
            if slime_rt::resolve_binding(b"notification:lifetime-quitter-exit+wait").is_ok() {
                slime_rt::debug_write(b"[lifetime-probe] role=quitter lifetime=resident\n");
                slime_rt::exit(0)
            }
            if slime_rt::resolve_binding(b"notification:lifetime-leaver-exit+wait").is_ok() {
                slime_rt::debug_write(b"[lifetime-probe] role=leaver lifetime=resident\n");
                slime_rt::exit(0)
            }
            slime_rt::debug_write(b"[lifetime-probe] role=holder lifetime=resident\n");
            loop {
                slime_rt::yield_now();
            }
        }
    }
}

/// Spawn the resident optional child, then report the outcome the root
/// recorded for it through the supervision handle.
fn run_owner(executable: u32) -> ! {
    let child = slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail(b"spawn quitter"));
    let outcome = loop {
        match slime_rt::supervision_status(child.supervision_slot) {
            Ok(Some(termination)) => break termination,
            Ok(None) => slime_rt::yield_now(),
            Err(_) => fail(b"quitter supervision"),
        }
    };
    slime_rt::debug_write(b"[lifetime-probe] role=owner quitter outcome=");
    slime_rt::debug_write(match outcome {
        Termination::Unhealthy => b"unhealthy\n",
        Termination::Exit(_) => b"exit\n",
        Termination::Fault(_) => b"fault\n",
        Termination::Timeout => b"timeout\n",
        Termination::PeerLoss => b"peer-loss\n",
    });
    slime_rt::exit(0)
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[lifetime-probe] FAIL ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
