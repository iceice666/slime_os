# Monotonic clock reads are ambient

**Status:** Accepted
**Decided by:** the project's human owner, 2026-10-26
**Related work item:** `01a1776e-0e31-7ebc-a877-8a2f3d656baf`
**Amends:** the monotonic-read part of `contracts/clock-authority/v1`

## Context

`contracts/clock-authority/v1` declares four authorities per holder: monotonic
read, timer use, simulated read and simulated advance. Omission is denial. The
root serves a monotonic read (label 44) and a rate read (label 70) only to a
holder that declares `monotonicRead`.

That gate does not hold against native code. On every AArch64 profile the kernel
sets `CNTKCTL_EL1.EL0PCTEN` globally, because the root reads and programs the
EL1 physical timer from EL0. Any component can execute `mrs CNTPCT_EL0`. The
root also answers a read with the raw counter value, so the gate does not
coarsen the clock either. The RISC-V profile grants `rdtime` through
`scounteren.TM` in the same way.

Replay does not depend on the gate. A recording component writes each value it
read into its own `fabric-trace/v1` stream, and a replaying component reads the
value back from that stream. The root never sees either step.

The gate has a cost. Under QEMU TCG one root round trip takes about 32 µs. In the
`sel4-io-tcp-buffers` plane, network-service read the clock 23,453 times. Those
reads took 0.75 s of root time.

## Decision

Reading the monotonic counter and its rate needs no authority.

- `contracts/clock-authority/v2` removes `monotonicRead`. A row that names it is
  refused, and so is a row that declares no remaining authority. The decoder
  refuses v1.
- Generation right bit 26, `CLOCK_MONOTONIC_READ`, is retired. Its position is
  never reused.
- On AArch64 and RISC-V, `slime_rt::monotonic_read` and `monotonic_frequency`
  read the counter and its frequency register directly.
- On x86_64 the root's clock is the HPET, which userspace cannot read. The
  root keeps serving labels 44 and 70 there, to every task, with no authority
  check.
- Timer use, simulated read and simulated advance stay declared authorities.
  The root multiplexes one comparator per core, and the simulated clock is the
  input a simulation or a replay controls.
- A component that the recording policy declares deterministic records its
  clock reads through its recorder, as before.

## Alternatives

- **Keep the right and stop enforcing it.** This changes no contract, but the
  contract would state a denial that the system does not perform.
- **Switch the x86_64 root clock to the TSC.** This makes reads direct on every
  architecture, but it needs TSC frequency calibration and an invariant-TSC
  check. It is left for a later change.
- **Keep the gate and reduce the reads.** Reusing an unchanged timer deadline
  and reading once per wait lowers the count, but each read still costs a round
  trip.

## Consequences

- An AArch64 or RISC-V component reads the clock without a root round trip.
- A clock read is no longer a declared capability. Determinism review relies on
  the recording policy, not on the absence of a read right.
- The timer comparator stays writable from EL0 on AArch64. That integrity gap is
  unchanged and still requires a kernel or platform change.

## Revisit when

- A profile cannot expose its root clock counter to userspace without also
  exposing a timer that the root owns.
- A threat model requires coarsened or denied time for some components.
