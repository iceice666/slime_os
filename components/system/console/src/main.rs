#![no_std]
#![no_main]

use boot_contracts::generation::BootAction;
use slime_components::generation_composition;
use slime_rt::{MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// A free page-aligned address, borrowed only for the startup self-check.
const QUOTA_PROBE_BASE: u64 = 0x0000_0006_0000_0000;

/// The shared-buffer factory this component is granted, when it is granted one.
///
/// Resolved through the root rather than compiled in (CP2). This was
/// `const SHARED_BUFFER_FACTORY_SLOT: u32 = 1`, reasoned out in a comment from
/// "slot 0 is the channel end and a root-launched component's runtime-numbered
/// slots start above its executables" — correct for the manifests that existed,
/// and exactly the coupling B70 names: a number that is a property of one
/// generation, restated in this component's own source.
///
/// `None` is a real answer, not a failure: under the channel plane this component
/// is granted no factory at all, and the caller already had to handle not being a
/// holder. Returning `None` rather than a plausible default is the point — a
/// default would resolve to *some* slot and hide a generation that stopped
/// granting the authority.
fn shared_buffer_factory_slot() -> Option<u32> {
    slime_rt::resolve_binding(b"console-shared-buffer-factory").ok()
}
const CLOSE: &[u8] = b"SLIME.CONSOLE.CLOSE";

fn main(_startup_arg: u32) {
    let mut buf = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    if generation_composition::is(BootAction::Channel) {
        slime_rt::debug_write(b"[console] unrelated progress while sender blocked\n");
    }
    // CP2's runtime binding resolution, proved on the planes this component
    // already boots rather than in a component written only to prove it. The
    // denial arm is asserted here; successful binding resolution is exercised
    // by `init` on channel planes and by this component's own buffer-factory
    // lookup on the loan plane.
    //
    // `init-shared-buffer-factory` is a real grant held by `init` in the planes
    // that declare it, so asking for it here is a component asking about
    // authority it was not granted — the case that must never resolve. A root
    // that answered from the shared boot layout instead of this instance's own
    // binding list would leak it, which is what an earlier root did.
    //
    // The exact status is asserted, not merely "an error". `is_err()` alone would
    // also accept `ERR_BAD_CAP` from the service-authority gate or a window
    // failure, so a regression that broke label 37's routing entirely would make
    // this component print `denied` and pass. `ERR_INVALID_ARG` is what the
    // not-bound answer returns specifically.
    match slime_rt::resolve_binding(b"init-shared-buffer-factory") {
        Err(slime_rt::ERR_INVALID_ARG) => {
            slime_rt::debug_write(b"[console] ungranted binding denied\n");
        }
        Err(_) => {
            slime_rt::debug_write(b"[console] ungranted binding refused for the wrong reason\n");
            slime_rt::exit(1);
        }
        Ok(_) => {
            slime_rt::debug_write(b"[console] ungranted binding leaked a slot\n");
            slime_rt::exit(1);
        }
    }
    loop {
        let n = slime_rt::recv_blocking(0, &mut buf, &mut caps);
        match n {
            n if n < 0 => slime_rt::exit(1),
            n => {
                if &buf[..n as usize] == CLOSE {
                    slime_rt::debug_write(b"[console] channel close received\n");
                    slime_rt::debug_write(b"[console] channel plane complete\n");
                    return;
                }
                slime_rt::debug_write(&buf[..n as usize]);
                // P5.3.2's unrelated holder. Under the loan-plane generation
                // this component is a declared shared-buffer holder that takes
                // no part in the loan, and the milestone requires the loan's
                // quota exhaustion to leave it undisturbed.
                //
                // Receiving and printing does not show that — it exercises the
                // channel plane, not the quota plane. So on the message init
                // sends *after* exhausting all four of its own ceilings, this
                // runs the same bounded create/map/write/seal/unmap/release the
                // startup probe performs, against its own declared quota. A
                // ceiling that leaked across holders fails here.
                //
                // The loan generation declares this holder's independent quota,
                // and CP2 resolves the slot it lands in at runtime. A generation
                // that declares the boot action but grants no factory is a
                // contradiction worth failing on rather than probing slot 1 and
                // hoping.
                if generation_composition::is(BootAction::Loan) {
                    let Some(factory) = shared_buffer_factory_slot() else {
                        slime_rt::debug_write(
                            b"[console] loan plane declares no shared-buffer factory binding\n",
                        );
                        slime_rt::exit(1);
                    };
                    if !slime_components::shared_buffer_probe::probe_and_report(
                        b"[console]",
                        factory,
                        QUOTA_PROBE_BASE,
                    ) {
                        slime_rt::exit(1);
                    }
                }
            }
        }
    }
}
