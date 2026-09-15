# Time, waiting, scheduling, and lifecycle

These mechanisms share one rule: generation data declares policy and authority;
`slime-root` enforces the small mechanism it owns; userspace decides what the
system should do.

## Clock and timers

`contracts/clock-authority/v1/` independently declares monotonic-read,
timer-use, simulated-read, and simulated-advance authority per holder. Omission
is denial. Timer authority includes a live-timer quota and the Notification badge
used for expiry, so timer delivery joins the existing userspace wait path.

`slime-root/src/clock.rs` serves the logical clock operations and
`slime-root/src/platform_timer.rs` owns the platform timer mechanism. The service
grants semantics, not exclusive register access. On current AArch64 profiles the
kernel enables EL0 physical counter and timer registers globally because the root
programs that timer from EL0; hostile native code could read or disrupt those
registers. Closing that wall requires a kernel/platform change, not a contract
claim.

Already-decided one-shot expiries must reach their declared badges even when a
later platform step or IRQ acknowledgement fails; cancellation delivers nothing
and termination reclaims live timers. Timer wakes do not consume the bounded
component-request iteration count, and absent authority remains distinguishable
from malformed requests.

## Wait sets

Wait sets are userspace, implemented by `components/runtime/src/wait_set.rs`
over one seL4 Notification plus explicit endpoint drains. The generation's
`contracts/wait-set/v1/` resource maps badge bits to source kind and the waiter's
own drain slot. The root exposes only a self-scoped read of those declared
entries and signals declared timer or supervision badges.
The waiter cannot derive this mapping itself: peer badges come from the
signaller's slot, while timer badges are independent contract data. Each badge
belongs to exactly one source; a supervision wake requires the waiter's declared
slot still to hold authority over the terminated task and adds no authority.

Notification badges coalesce, so they mean readiness, not event counts. A waiter
drains every ready source. Simultaneous bits dispatch in ascending badge order;
that tie rule is contract data and makes the same readiness produce the same
sequence across boots. A waiter may register at most nine sources, matching the
widest admitted fabric wait set.
Duplicate or undeclared badges and over-budget dispatch are refused without
poisoning the waiter; an unnamed instance has no sources. Fixed source tables
and badge deduplication bound the ready queue without allocation or a root-owned
wait set, ready queue, or source registry.

## Scheduling class

`contracts/scheduling-class/v1/` declares three assignable classes — foreground,
normal, and best-effort — and maps each to an exact seL4 TCB priority. It may also
declare a spawner-to-child promotion edge with a ceiling. The promotion right
rides on the child's supervision capability; self-promotion and promotion of a
non-owned task are refused.
Promotion above the declared ceiling or to the distinct `undeclared` class is
refused without changing class. Class and explicit thread priorities must agree,
including inherited worker priorities; an unnamed instance retains the root's
child priority rather than being reported as normal.

A class controls ordering only. The pinned kernels use `KernelIsMCS OFF`, so no
CPU time budget or period exists to charge. Generation schedule records must keep
`budget_us` and `period_us` zero; both readers reject nonzero values. Conserved
CPU accounts are not a current capability.
Higher-class progress must not depend on denying a best-effort worker all CPU.

## Lifecycle and supervised restart

`contracts/lifecycle-policy/v1/` declares the admitted transition graph,
per-instance restart causes and attempt bounds, backoff, health dependencies,
and parameter read/write authority. `slime-root/src/lifecycle.rs` and the task/
supervision mechanisms record terminal state, enforce transitions and restart
admission, and reclaim the old task. They do not decide to restart it.

A userspace supervisor holds the supervision capability, waits through the clock
service, chooses whether policy calls for another launch, and spawns a fresh
instance. Attempts survive task identities, backoff is checked against the
monotonic clock rather than a spin count, and exhaustion enters the declared
terminal state. Fresh construction prevents stale capabilities, mappings, and
resource charges from surviving a restart.
Parameter values and the last terminal cause also persist per instance;
task identities are never reused, and a replacement derives its
lifecycle entry state, class, and quota from the generation. The root refuses
first spawn until health dependencies reach their declared states, refuses early
or exhausted restart attempts, and leaves state unchanged on an undeclared
transition.

Parameter read and write are independent holder-to-subject authorities; omission
denies even self-access, and absent authority is distinguishable from an unset
key. Predecessor handles are refused after replacement.

The currently reachable terminal causes are exit, fault, and declared
unhealthiness. Timeout and native-endpoint peer-death are not declared causes
because no current mechanism produces them.
These terminal causes remain distinguishable.

## Recording and determinism

`contracts/recording-policy/v1/` binds declared record/replay participants to one
bounded stream. Admission joins that policy with the generation's complete
rights classification. A deterministic participant cannot hold an unrecorded
input authority except for the exact declared grant carrying its recording.
The same restriction applies to imported authority at runtime, not only launch
admission; missing or unbound recording grants and unnecessary recorder
exemptions are refused, and rights joins include executable grants.

Clock reads, timer expiries, lifecycle transitions, outputs, and fabric traffic
share one typed trace order; every right has an explicit determinism
classification. The bounded, allocation-free replay stream is validated whole
before exposing input, refusing truncation, reordering, excess capacity, and
arbitrary terminal-flagged resource records as stream openers.

Replay compares typed outputs, recorded reads, lifecycle transitions, and timer
identities field by field; hardware instants are recorded, range-checked inputs,
not cross-boot output-equality requirements. A refused step leaves its cursor
unchanged, and replay never falls back to live clock access.

Determinism is therefore a composition claim under declared inputs and limits.
It is not a scheduler guarantee and does not imply CPU quantity isolation.

## Proposed MCS decision

[`../decisions/mcs-cpu-budgets.md`](../decisions/mcs-cpu-budgets.md) retains the
MCS choice as **proposed**. Current operation remains non-MCS. Enabling MCS is not
a configuration flip: it changes reply handling, adds Reply and scheduling-
context objects, needs per-thread reservation/account ownership and aggregate
admission, and has target-specific assurance consequences.

## Verification

- `just clock_authority_check`
- `just wait_set_check`
- `just scheduling_class_check`
- `just lifecycle_restart_check`
- `just robot_runtime_check`
- `just sel4_gate_control_check` for missing/reordered/failure controls
