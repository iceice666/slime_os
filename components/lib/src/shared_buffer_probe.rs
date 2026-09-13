//! Startup self-check proving a component's generation-declared shared-buffer
//! quota is live (C7.2/C7.3).
//!
//! A component reaches this code only if the generation both grants it a
//! `SharedBufferFactory` capability and lists it in the `shared-buffer-budget`
//! resource. Those are independent: the grant authorizes the operation, the
//! budget bounds it, and a holder missing either allocates nothing. Running the
//! full create/map/write/seal/release lifecycle at startup is what makes the
//! live boot path — not just the kernel test harness — evidence that the quota
//! was decoded from the generation and charged to this component.
//!
//! Deliberately bounded: one page, mapped once, released before returning. The
//! check must not consume quota a component later needs, and it must not depend
//! on any peer, so it is safe to run before a component enters its main loop.

use slime_rt::ERR_SUCCESS;

/// Result of the startup shared-buffer self-check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// The component allocated, mapped, wrote, sealed, and released one page.
    /// Its generation-declared quota is live.
    Ok,
    /// The factory capability is absent or carries no creation right.
    Denied,
    /// Creation was authorized but the quota rejected it — the component has no
    /// budget entry, or its ceiling is already reached.
    QuotaExceeded,
    /// A lifecycle step failed after a successful allocation.
    Failed,
}

/// Exercise one page of shared-buffer quota through `factory_slot`, mapping it
/// at `base`, and release it again.
///
/// `base` must be a free, page-aligned user address in the caller's address
/// space; the mapping is removed before this returns, so the address is only
/// borrowed for the duration of the check.
pub fn probe(factory_slot: u32, base: u64) -> ProbeOutcome {
    const PAGE: u64 = 4096;
    const MARKER: u8 = 0x5B;

    let buffer = match slime_rt::shared_buffer_create(factory_slot, 1, true) {
        Ok(buffer) => buffer,
        Err(slime_rt::ERR_BAD_CAP) => return ProbeOutcome::Denied,
        Err(slime_rt::ERR_OUT_OF_MEMORY) => return ProbeOutcome::QuotaExceeded,
        Err(_) => return ProbeOutcome::Failed,
    };
    if buffer.id == 0 {
        return ProbeOutcome::Failed;
    }

    let mut outcome = ProbeOutcome::Ok;
    if slime_rt::shared_buffer_map(buffer.slot, base, 0, PAGE, true) != ERR_SUCCESS {
        outcome = ProbeOutcome::Failed;
    } else {
        // SAFETY: the kernel installed a writable user mapping of exactly one
        // page at `base`, and it stays mapped until the unmap below.
        unsafe {
            let cell = base as *mut u8;
            cell.write_volatile(MARKER);
            if cell.read_volatile() != MARKER {
                outcome = ProbeOutcome::Failed;
            }
        }
        // Sealing must downgrade the live writable mapping rather than fail.
        if slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS {
            outcome = ProbeOutcome::Failed;
        }
        if slime_rt::shared_buffer_unmap(buffer.slot, base) != ERR_SUCCESS {
            outcome = ProbeOutcome::Failed;
        }
    }

    if slime_rt::shared_buffer_release(buffer.slot) != ERR_SUCCESS {
        outcome = ProbeOutcome::Failed;
    }
    outcome
}

/// Run [`probe`] and report the result on the debug console under `label`.
/// Returns `true` when the component's quota is live.
pub fn probe_and_report(label: &[u8], factory_slot: u32, base: u64) -> bool {
    let outcome = probe(factory_slot, base);
    slime_rt::debug_write(label);
    slime_rt::debug_write(match outcome {
        ProbeOutcome::Ok => b" shared-buffer quota live\n" as &[u8],
        ProbeOutcome::Denied => b" shared-buffer denied\n",
        ProbeOutcome::QuotaExceeded => b" shared-buffer quota exhausted\n",
        ProbeOutcome::Failed => b" shared-buffer lifecycle failed\n",
    });
    outcome == ProbeOutcome::Ok
}
