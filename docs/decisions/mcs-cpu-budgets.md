# MCS and conserved CPU budgets

**Status:** Proposed
**Related work items:** `01a00b4d-2400-75f0-82f5-9abe614054e9`,
`01a03480-0400-7e1f-b198-2d33228dd7f8`,
`019ff18d-5800-70c6-a987-6a926141c775`,
`01a02f59-a800-71ef-977f-bb263c65b2d4`

## Context

The current scheduling-class contract maps generation-declared foreground,
normal, and best-effort classes to seL4 TCB priorities. It controls ordering but
cannot reserve CPU quantity because every pinned kernel profile is built with
`KernelIsMCS OFF`. Generation schedule records carry reserved `budget_us` and
`period_us` fields, and both validators reject nonzero values.

The pinned source shows target-specific proof coverage: AArch64 MCS functional-
correctness proofs are in progress, and RISC-V MCS has a verified configuration.
Both current AArch64 builds are already outside the verified set. The Pi 5
overlay includes upstream's verified bcm2712 profile but forces verification
off and debug/printing on for serial observability. The earlier proposal's
verified-Pi-versus-unverified-QEMU framing no longer describes these builds;
future assurance must be judged against each exact target configuration.

## Proposed decision

Keep MCS disabled until one target-specific proposal covers the complete API,
resource, admission, assurance, and verification cutover. Do not expose CPU
budgets or periods before the kernel has a quantity it can actually charge.

This does not reopen the current scheduling-class mechanism. Priority classes
remain the declared ordering axis; conserved CPU accounts would be a second axis.

## Required cutover scope

Enabling MCS is not a CMake toggle:

1. migrate every call-receiving Endpoint path from implicit reply authority to
   explicit Reply objects; `sel4::reply` is absent under MCS;
2. construct, distribute, account, and reclaim Reply and scheduling-context
   objects through spawn, restart, fault, and task teardown;
3. define per-thread reservations, aggregate admission against the target's CPU
   capacity, account ownership, timeout-fault routing, and structured exhaustion;
4. update generation and component declaration surfaces without silently
   reinterpreting the reserved non-MCS fields;
5. run the full semantic plane corpus against the same configuration the target
   is intended to ship, plus target-specific assurance review.

## Alternatives and trade-offs

- **Enable MCS on QEMU only:** locally cheap in assurance terms, but makes the
  primary development corpus exercise a kernel configuration the physical Arm
  target does not share and still leaves reply/resource semantics unfinished.
- **Keep nonzero budget fields and ignore them:** rejected. It authenticates a
  policy the runtime cannot enforce; current readers correctly refuse it.
- **Model budgets only in userspace:** cannot provide conserved CPU time against
  hostile components because priority alone carries no consumable quantity.

## Consequences

- Current scheduling authority remains class/priority ordering only.
- No component or plan may claim a CPU reservation from `budget_us`,
  `period_us`, or a scheduling-class grant.
- Work that needs bounded accelerator or service throughput must choose a unit it
  can actually account — requests, tokens, bytes, or queue time — rather than
  inheriting a fictional general CPU account.
- A future MCS proposal has a known source-level seam but may reveal additional
  breakage when compiled and booted; the existing survey was static.

## Revisit conditions

Revisit when a product workload requires conserved CPU time, a target's
assurance position is accepted explicitly, and an implementation item owns the
Reply-object migration, scheduling-context lifecycle, declaration changes, and
full target verification together.

## References

- `sel4/config/qemu-arm-virt.cmake` and `sel4/config/bcm2712-rpi5.cmake`
- `sel4/pins.toml`
- `deps/sel4/CAVEATS.md`
- `deps/rust-sel4/crates/sel4/src/`
- `contracts/scheduling-class/v1/schema.zt`
- `slime-root/src/{task,ipc,fault}.rs`
- `components/runtime/src/syscall/sel4_transport.rs`
- [`../architecture/runtime-authority.md`](../architecture/runtime-authority.md)
