//! Native seL4 child fixture proving the `slime-root` mechanism cutover.
//!
//! The root task builds this image into its own CSpace/VSpace/TCB and grants it
//! exactly one authority it can invoke: the badged root service endpoint in
//! CSpace slot 1. The fixture sends one root service request, then follows the
//! directive the root returns — exit cleanly, or fault deliberately so the root
//! observes a fault on its supervision path.
//!
//! The clean-exit fixture additionally runs the shared-buffer phase before it
//! exits. That phase is the observable half of `slime-root/src/buffer_adapter.rs`:
//! it reads bytes the root wrote through a real shared frame, writes a reply
//! into the same frame, and then deliberately violates the two protections the
//! root's mapping rights are supposed to enforce by mechanism —
//!
//! 1. storing to a read-only mapping, and
//! 2. executing from a data page mapped `EXECUTE_NEVER`.
//!
//! Both must raise an attributable VM fault. The root supervises each one,
//! moves this thread's PC past it, and resumes, so the fixture still reaches
//! its ordinary clean exit. A probe that does *not* fault is a protection
//! failure, and the fixture says so loudly rather than continuing quietly.
//!
//! Slot numbers and operation labels here mirror `slime-root/src/task.rs` and
//! `slime-root/src/ipc.rs`. Nothing is discovered at runtime: a fixture that
//! could search for authority would not prove the grant is what conveys it.

#![no_std]
#![no_main]

mod runtime;

/// Badged root service endpoint. `slime-root/src/task.rs::CHILD_SLOT_SERVICE`.
const SLOT_SERVICE: sel4::CPtrBits = 1;
/// This task's own TCB, present only when the root supervises it as
/// self-managed. `slime-root/src/task.rs::CHILD_SLOT_TCB`.
const SLOT_TCB: sel4::CPtrBits = 2;
/// Fixture directive label owned by the root behavioral harness.
const OP_FIXTURE_DIRECTIVE: sel4::Word = 5;
/// Lifecycle exit label owned by the root task mechanism.
const OP_EXIT: sel4::Word = 3;
/// Shared-buffer map label, reused as this fixture's shared-buffer report.
const OP_SHARED_BUFFER_REPORT: sel4::Word = 23;

/// Fixture request tag, `b"SLIMEREQ"` big-endian, so the root can tell this
/// fixture's request from any other traffic on its endpoint.
const REQUEST_TAG: sel4::Word = 0x534c_494d_4552_4551;

/// Root directive returned in the reply's second message register.
const DIRECTIVE_EXIT: sel4::Word = 0;
const DIRECTIVE_FAULT: sel4::Word = 1;

/// Address deliberately left unmapped by the root loader. `child_vspace`
/// rejects an image whose footprint starts at zero, so a store here is always
/// an unmapped data access and always raises a VM fault.
const UNMAPPED_ADDRESS: usize = 0;

// ---- shared-buffer phase contract ----
//
// Every constant below is duplicated in `slime-root/src/main.rs` under the same
// name. They are compile-time agreements, not runtime discovery: a fixture that
// could find its shared region by searching would not prove that the root's
// mapping is what grants access to it.

/// Where the root maps the read-write shared region.
/// `slime-root/src/main.rs::SHARED_RW_VADDR`.
const SHARED_RW_VADDR: usize = 0x4000_0000;
/// Where the root maps the read-only shared region.
/// `slime-root/src/main.rs::SHARED_RO_VADDR`.
const SHARED_RO_VADDR: usize = 0x4001_0000;

/// Byte offset of the deterministic pattern the root writes into each region.
/// `slime-root/src/main.rs::SHARED_PATTERN_OFFSET`.
const SHARED_PATTERN_OFFSET: usize = 64;

/// The exact value the root writes at `SHARED_PATTERN_OFFSET` of the
/// read-write region. `slime-root/src/main.rs::SHARED_RW_PATTERN`.
const SHARED_RW_PATTERN: u64 = 0x5342_5546_5f52_5721;
/// The value the child writes back into the read-write region.
/// `slime-root/src/main.rs::SHARED_CHILD_REPLY`.
const SHARED_CHILD_REPLY: u64 = 0x4348_494c_445f_4f4b;
/// The value this fixture attempts, and must fail, to store into the read-only
/// region. `slime-root/src/main.rs::SHARED_RO_INTRUSION`.
const SHARED_RO_INTRUSION: u64 = 0xdead_beef_dead_beef;

/// Report flags this fixture sets in the shared-buffer report's third message
/// register. `slime-root/src/main.rs::REPORT_*`.
///
/// `REPORT_EXECUTE_REFUSED` is deliberately absent: only the root can decide
/// that one, from its own fault record.
const REPORT_RW_READBACK_OK: sel4::Word = 1 << 0;
const REPORT_RO_WRITE_REFUSED: sel4::Word = 1 << 1;

pub(crate) fn main() -> ! {
    let service = sel4::cap::Endpoint::from_bits(SLOT_SERVICE);

    sel4::debug_println!("SLIME_CHILD request op={OP_FIXTURE_DIRECTIVE} tag={REQUEST_TAG:#x}");
    let reply = service.call_with_mrs(
        sel4::MessageInfoBuilder::default()
            .label(OP_FIXTURE_DIRECTIVE)
            .length(2)
            .build(),
        [REQUEST_TAG, 0],
    );
    let result = reply.msg[0] as i64;
    let directive = reply.msg[1];
    sel4::debug_println!("SLIME_CHILD reply result={result} directive={directive}");

    match directive {
        DIRECTIVE_FAULT => {
            sel4::debug_println!("SLIME_CHILD fault requested addr={UNMAPPED_ADDRESS:#x}");
            // SAFETY: this store is intended to fault. The address is unmapped
            // in this task's VSpace by construction, so the kernel raises a VM
            // fault and delivers it to the root's fault endpoint; this thread
            // never resumes.
            unsafe {
                (UNMAPPED_ADDRESS as *mut u64).write_volatile(REQUEST_TAG);
            }
            sel4::debug_println!("SLIME_CHILD fault escaped");
        }
        DIRECTIVE_EXIT => {
            // The shared-buffer phase runs before the exit send, so the root
            // observes the report and both supervised probes while this task is
            // still live and attributable.
            shared_buffer_phase(service);
            sel4::debug_println!("SLIME_CHILD clean exit status=0");
            // The root answers `Exit` by suspending this thread, so this send
            // is the last thing the fixture does on its service endpoint.
            service.send_with_mrs(
                sel4::MessageInfoBuilder::default()
                    .label(OP_EXIT)
                    .length(1)
                    .build(),
                [0],
            );
        }
        other => {
            // An unrecognized directive is treated as exit rather than as a
            // silent default, so a protocol drift shows up in the record
            // instead of being masked by a catch-all match arm.
            sel4::debug_println!("SLIME_CHILD unknown directive={other} treated as exit");
            service.send_with_mrs(
                sel4::MessageInfoBuilder::default()
                    .label(OP_EXIT)
                    .length(1)
                    .build(),
                [0],
            );
        }
    }

    // Reached only if the root neither suspended this thread nor let the fault
    // stand. A self-managed task stops itself; otherwise it parks so it can
    // never be mistaken for a task still doing work.
    let own_tcb = sel4::cap::Tcb::from_bits(SLOT_TCB);
    if own_tcb.tcb_suspend().is_err() {
        sel4::debug_println!("SLIME_CHILD parked");
        loop {
            sel4::r#yield();
        }
    }
    unreachable!()
}

/// Read what the root wrote through the shared frame, answer it, then prove the
/// two protections the root's mapping rights claim to enforce.
///
/// Each probe deliberately performs an access the page tables must refuse. The
/// root supervises the resulting VM fault, steps this thread's PC past the
/// faulting instruction, and resumes it, so control returns to the next line
/// here. A probe that returns *without* having faulted means the protection did
/// not hold; that is reported as a failure flag rather than silently ignored.
fn shared_buffer_phase(service: sel4::cap::Endpoint) {
    let rw_pattern_addr = SHARED_RW_VADDR + SHARED_PATTERN_OFFSET;
    let ro_pattern_addr = SHARED_RO_VADDR + SHARED_PATTERN_OFFSET;
    let mut flags: sel4::Word = 0;

    // Probe 1: the root wrote a deterministic pattern into a frame it then
    // mapped here. Reading it back is what proves the frame is genuinely
    // shared rather than separately zeroed memory.
    //
    // SAFETY: the root maps a read-write frame covering `SHARED_RW_VADDR`
    // before resuming this thread, `SHARED_PATTERN_OFFSET + 8` is inside that
    // one 4 KiB page, and the address is 8-byte aligned. No Rust reference
    // aliases this address.
    let observed = unsafe { (rw_pattern_addr as *const u64).read_volatile() };
    if observed == SHARED_RW_PATTERN {
        flags |= REPORT_RW_READBACK_OK;
    }
    sel4::debug_println!(
        "SLIME_CHILD shared read vaddr={rw_pattern_addr:#x} observed={observed:#x} expected={SHARED_RW_PATTERN:#x}"
    );

    // Write back through the same mapping so the root can confirm the sharing
    // is bidirectional and not a copy.
    //
    // SAFETY: same page and alignment as the read above, and the mapping is
    // read-write, so this store is permitted by the page tables.
    unsafe {
        (rw_pattern_addr as *mut u64).write_volatile(SHARED_CHILD_REPLY);
    }

    // Probe 2: the read-only mapping must refuse a store. This is the
    // rights-are-mechanism proof: the very same frame object is mapped here
    // with `CapRights::read_only()`, so `maskVMRights` produced VMReadOnly and
    // the page-table entry itself rejects the write.
    sel4::debug_println!("SLIME_CHILD ro write probe vaddr={ro_pattern_addr:#x}");
    // SAFETY: the root maps a read-only frame covering `SHARED_RO_VADDR`. The
    // store is *expected* to fault; the root supervises that fault and advances
    // this thread's PC past it. If the mapping were wrongly writable the store
    // would simply succeed, which the check below detects and reports.
    unsafe {
        (ro_pattern_addr as *mut u64).write_volatile(SHARED_RO_INTRUSION);
    }
    // SAFETY: as for the read in probe 1; the read-only mapping permits loads.
    let after_write = unsafe { (ro_pattern_addr as *const u64).read_volatile() };
    if after_write != SHARED_RO_INTRUSION {
        flags |= REPORT_RO_WRITE_REFUSED;
    }
    sel4::debug_println!(
        "SLIME_CHILD ro write result observed={after_write:#x} intrusion={SHARED_RO_INTRUSION:#x}"
    );

    // Probe 3: a data page must not be executable. The root maps every shared
    // frame `EXECUTE_NEVER`, so branching into one must raise an instruction
    // abort rather than executing whatever bytes happen to be there. This is a
    // live regression test on `child_vspace::page_attributes`: without the
    // execute-never attribute the branch would run into arbitrary data.
    //
    // This fixture cannot adjudicate its own result. On an instruction abort
    // the faulting PC is the branch *target*, so the root resumes this thread
    // at its link register — which is exactly where a genuinely-executed page
    // would also have returned. The two paths are indistinguishable from here,
    // so the verdict belongs to the root, which either observed an Execute
    // fault at this address or did not.
    sel4::debug_println!("SLIME_CHILD wx exec probe vaddr={SHARED_RW_VADDR:#x}");
    branch_to_data_page(SHARED_RW_VADDR);
    // Reached either way: on an instruction abort the root resumes this thread
    // at `x30`, which is exactly where a genuinely-executed page would also
    // have returned. Only the root can tell the two apart.
    sel4::debug_println!("SLIME_CHILD wx exec probe returned vaddr={SHARED_RW_VADDR:#x}");

    // Hand every observation to the root in one bounded message. The root, not
    // this fixture, decides whether the phase passed, and it supplies the
    // execute-never verdict from its own fault record.
    let reply = service.call_with_mrs(
        sel4::MessageInfoBuilder::default()
            .label(OP_SHARED_BUFFER_REPORT)
            .length(4)
            .build(),
        [REQUEST_TAG, observed as sel4::Word, flags, 0],
    );
    sel4::debug_println!(
        "SLIME_CHILD shared report flags={flags:#x} result={}",
        reply.msg[0] as i64
    );
}

/// Branch into `addr`, a page the root mapped as non-executable data.
///
/// The `blr` sets `x30` to the instruction after it, so the root can resume
/// this thread at its link register once it has recorded the instruction abort.
/// That recovery is bounded and lands exactly once: the branch is not retried.
fn branch_to_data_page(addr: usize) {
    // SAFETY: this block performs no memory access. `blr` writes the link
    // register, and `clobber_abi("C")` declares every caller-saved register —
    // including `x30` — as clobbered, so the compiler preserves anything live.
    // The branch is expected to raise an instruction abort; the root supervises
    // it and resumes this thread at `x30`, the instruction after the branch.
    unsafe {
        core::arch::asm!(
            "blr {target}",
            target = in(reg) addr,
            clobber_abi("C"),
        );
    }
}
