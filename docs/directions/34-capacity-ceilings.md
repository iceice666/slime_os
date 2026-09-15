# 34. Root capacity ceilings: threads and cores

## Motivation

A workload can fit its memory budget yet require more blocking-IPC concurrency
than the component runtime admits. More threads and more cores are separate
changes: threads can overlap waits on one core, while SMP permits children to
execute concurrently and requires target-specific assurance and invariant review.
Neither follows from a larger private-memory budget.

## Thread-count boundary

[`component-runtime-abi/v1`](../../contracts/component-runtime-abi/v1/schema.zt)
declares `maxThreads = 2`. The worker form of
[`entry!`](../../components/runtime/src/lib.rs) emits one worker stack, entry
point, and anchor; the [child loader](../../slime-root/src/child_vspace.rs)
resolves that worker's symbols. Raising the count must change both producer and
loader, not only the ABI constant.

The generation's thread budget, per-thread descriptor index, IPC buffer,
transfer window, and shared private-heap synchronization must all support the
admitted count. Existing arrays or permissive field types alone do not qualify
N workers. Each additional thread needs its own TCB, IPC buffer, and transfer
window, with bounded construction and reclamation.

### N worker threads

One design emits N worker-symbol sets from `entry!`; another emits an indexed
table that the root resolves once. Either changes the runtime declaration and
loader together and preserves per-thread fault attribution. This work is
independent of private-memory ceiling changes.

## SMP assurance boundary

The current [platform configurations](../../sel4/config/) select
`KernelMaxNumNodes 1`. A future SMP proposal must inspect the pinned seL4 caveats
and exact target configuration rather than inherit proof coverage from an ISA or
from a different board. The [MCS proposal](../decisions/mcs-cpu-budgets.md)
distinguishes the current configurations from upstream verified profiles.

SMP and MCS are distinct but coupled decisions. Non-MCS affinity uses the TCB
interface; under MCS placement moves to scheduling contexts. A combined change
must include the appropriate affinity API and the Reply/scheduling-context
cutover rather than treating either feature as a configuration toggle.

### SMP when a workload requires it

A proposal must name the workload and target, its assurance departure, whether
MCS is enabled in the same change, and the concurrency assumptions of the root's
fixed tables. More cores do not themselves make the single-threaded root
multi-threaded, but they do make children concurrent. Review every root-mediated
invariant that could previously rely on only one child running at a time.

The pinned source's distinctions between plain SMP, SMP with hypervisor
extensions, and SMP with both MCS and hypervisor extensions must be evaluated
separately. An upstream configuration's support or stability statement is not
proof coverage or an observed Slime qualification.

## Open questions

- Should N workers be declared through individual symbols or one indexed table,
  and how does admission bound that representation?
- Which root invariants rely on children being mutually exclusive in time rather
  than on a lock, capability boundary, or explicit protocol ordering?
- Which named workload needs SMP, and what target-specific assurance departure
  is acceptable for it?

## Qualification boundary

For additional threads, a component must run N > 2 threads, each with its own
IPC buffer and transfer window, and a fault in one must be attributed to that
thread. Construction, shared-heap access, and reclamation remain bounded.

No SMP exit is chosen here: it needs a named target, recorded assurance decision,
and review of the root's concurrency assumptions. Paper analysis precedes that
choice; neither a memory-capacity result nor a successful compilation promotes
these sketches to supported runtime behavior.
