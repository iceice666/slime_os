# The root task keeps mechanism only, and its source has a target

**Status:** Accepted
**Decided by:** the project's human owner, 2026-11-17
**Amended:** 2026-11-18, by the owner: the fixture role leaves in two steps,
and a ceiling follows its implementation
**Amended:** 2026-11-22, by the owner: the policy role and the timer role
stay, the dispatcher of the console role leaves, and the target of stage one
is restated from a measured count
**Related work item:** `root-size-budget`, UUID
`01a1e925-f21f-7c7b-8c45-b6b46bcefd1e`

## Context

`slime-root` holds every capability the kernel hands out at boot. A defect in
any root line can break every isolation claim the system makes.

`just root_size_check` gives each Rust file under `slime-root/` one role and
counts its code lines. A role is the group of files the gate budgets together.
On `main` at `41db01a`, the nine series roles hold 34294 code lines.

Three facts conflict with the rule that mechanism belongs in the root and
policy in components:

- The `policy` role holds 2077 lines.
- `slime-root/src/ipc.rs` names three product components: `entropy-service`,
  `zenoh-service` and `virtio-blk-driver`.
- `slime-root/src/graph_runtime.rs` and `slime-root/src/graph_runtime/` hold
  7427 code lines. Only the binary compiles them, so `just test_sel4_root`
  does not reach them.

## Decision

1. The root keeps a role only if one of two conditions holds. The role needs
   an authority no component may hold: untyped memory, the root CNode, or
   another task's TCB, VSpace or CSpace. Or the role runs before the first
   component exists.
2. A choice among outcomes that the generation allows is policy. A component
   makes that choice.
3. The root names no product component. It reads composition from generation
   rows.
4. The library compiles every root module, so host tests reach it.
5. The work continues in the existing repository. There is no rewrite.
6. The nine series roles reach 23000 code lines or fewer in two stages, as
   `root_size_check` counts them.

| Role | Lines | Decision | What stays in the root |
|---|---|---|---|
| core | 15037 | stays | all of it |
| boot | 5865 | stays | all of it |
| dispatch | 2538 | stays, without the readers that name product components | operation decoding and the service loop |
| policy | 2077 | leaves, to a supervisor component | the operations that policy invokes |
| timer | 1345 | leaves, to a timer component | the handover of the timer device and its IRQ |
| console | 427 | splits | the root's own diagnostic writer |
| fixture | 950 | leaves in two steps, see below | nothing |
| ioResource | 3029 | leaves in stage two | not decided |
| sharedBuffer | 3026 | leaves in stage two | not decided |

### Stage one: 28850 lines

The amendment of 2026-11-22 withdraws this target. This section states the
plan as the owner decided it on 2026-11-17.

The fixture, policy, timer and console roles leave. The dispatch role loses
its product-name readers. Test-only helpers that sit inside product items move
into `#[cfg(test)]` modules, where the gate counts them as test lines.

| Step | Lines |
|---|---|
| The series today | 34294 |
| fixture, policy, timer and console | −4799 |
| The readers in `ipc.rs`, lines 1036 to 1690 | −409 |
| Test-only helpers inside product items, about | −426 |
| The series with nothing left behind | 28660 |

The target of 28850 allows about 190 lines of what stays from the leaving
roles. The count of 426 comes from a one-off script and is approximate.

### What stage one has reached

The series holds 33194 code lines on `main` after PR 1118. The gate's ceiling
is 33200.

| Step | State |
|---|---|
| fixture | Out of the series since PR 1107. |
| The root names no product component | Done in PR 1117. `just root_size_check` counts 0 string literals that name a product component; it counted 6. |
| The readers in `ipc.rs` | They stay. The builder states who reads a table, and the root answers from the generation's reader table. The dispatch role holds 2510 lines. |
| policy and timer | They stay, by the amendment of 2026-11-22. The timer role grew to 1435 lines in PR 1118, which added the backend for the BCM2835 system timer. |
| console | The dispatcher leaves. Not started. |

The row of 409 lines in the table above assumes that the readers leave the
root. PR 1117 removed the names and kept the readers. A later design decides
how a component reads its own rows without a reader in the root. If the
allowance of stage one does not cover the difference, the owner restates the
target.

### Amendment of 2026-11-22: the policy and timer roles stay

A survey read the three roles that stage one has leave the root. Most of their
lines meet condition 1 of the decision, and the rest decide nothing that a
component could decide in their place. The owner decides:

1. The policy role and the timer role stay in the root.
2. The dispatcher of the console role leaves. The root's own diagnostic
   writer stays.
3. The target of 28850 lines is withdrawn. The owner restates the target of
   stage one from a measured count, after the dispatcher has left.
4. The gate sorts the files of these roles again, so that the name of a role
   states what it holds. That changes no line of the root.

The counts below are code lines, as `just root_size_check` counts them, on
`main` at `7e93cf4e`.

| Part of the policy role | Lines | Finding |
|---|---|---|
| `notification.rs`, `wait_set.rs` | 337 | They allocate Notification objects from untyped memory, mint badged capabilities in the root CNode, and copy one into a child's CNode. Condition 1 keeps them. |
| `scheduling.rs` | 179 | A promotion calls `tcb_set_priority` on another task's TCB, with the root's TCB as the authority. Condition 1 keeps it. |
| `lifecycle.rs`, and its requests in `services/policy.rs` | 709 | The root records why an instance ended and refuses a restart that the generation does not admit. It decides no restart. |
| `directory.rs` | 337 | The console thread serves two of its three operations. The design of the console dispatcher decides where they go. |
| Reclaim and memory census in `services/policy.rs` | 165 | Task teardown. It is core mechanism. |
| Clock requests in `services/policy.rs` | 187 | They serve the timer role. |
| Supervision and scheduling requests in `services/policy.rs` | 155 | Request decoding for the parts above. |

The policy role holds 2069 lines. Decision point 2 already holds for it: a
component with supervision authority decides a restart, waits and spawns.
`robot-supervisor` and `lifecycle-restart-probe` do so today.

`lifecycle.rs` does not move for one reason. `SPAWN` reads the attempt count
and the backoff instant of an instance in the same step that creates the task.
The module states the rule: a supervisor that skips its wait is refused, not
trusted. A component that held those counts would have to be trusted.

The timer role holds 1435 lines, and it does not move for four reasons:

- The root proves its timer in the boot sequence, before a component exists.
- The root uses the timer queue itself. One slot is reserved for its cleanup
  retries, and restart admission and the spawn backoff read the clock.
- The timer's Notification is bound to the root's service thread, so requests
  and timer delivery share one blocking point and no wake is lost. A component
  runs at priority 254 or below, and the root above it. A timer component
  would not preempt the other components.
- On RISC-V, on x86-64 and in a Raspberry Pi 5 system-timer image, the root
  answers `CLOCK MONOTONIC READ` for every task.

A timer component would therefore add a second clock and remove none.

The console role holds 443 lines:

| Part of the console role | Lines | Decision |
|---|---|---|
| `console.rs`, `graph_runtime/console_runtime.rs` | 300 | The dispatcher: a second root thread that serves debug writes, terminal input and two directory operations. It leaves. |
| `diagnostic.rs`, `graph_runtime/services/diagnostic_once.rs` | 143 | The root's own writer and its once-per-key filter. They stay. |

The dispatcher maps a caller's frame into the root's VSpace to copy a debug
payload, and it shares the UART with the root's own writer. A design under
`docs/plans/` settles both before an item plans the move.

Stage one therefore removes at most 300 more lines, and the series ends near
32900. The lines that a later decision can still move are in stage two, and in
the 8164 lines of `object_allocator` that the alternatives below name.

One consequence below no longer holds: a timer wake keeps passing through the
root.

### How the fixture role leaves

The fixture role holds two parts, and they leave at different times.

| Part | Lines | What it is | When it leaves |
|---|---|---|---|
| `slime-root/child/` | 390 | the program of the fixture's child task. It is a crate of its own and runs without the root's authority. | first: the crate moved to `components/root-child/` in PR 1053 |
| `slime-root/src/fixture_runtime.rs` | 560 before PR 1107, 1007 after | the root side of the fixture. It runs with the root's authority. PR 1107 moved 440 lines of `main` into it. | second: PR 1107 put its module behind `cfg(slime_root_fixture)` |

The child crate cannot move to `components/testkit/`. `just
component_crate_split_check` holds every crate there to the shape of a
component, and the child is not one.

The root side shares the first part of `main` with the product: both run one
boot sequence and diverge after it. The fixture can become a root binary of its
own only when the library holds that sequence. Until then every root image,
the product included, links the fixture's root side and embeds the child image.

PR 1068 moved that sequence and the graph runtime into the library, as
`slime_root::boot::run` and `slime_root::graph_runtime`. The binary alone now
compiles `slime-root/src/main.rs` and `slime-root/src/fixture_runtime.rs`, 1119
code lines.

PR 1107 made the fixture a root build of its own. `slime-root/src/main.rs`
declares `fixture_runtime` behind `cfg(slime_root_fixture)`, and
`slime-root/build.rs` sets that cfg for the fixture root alone. A product root
compiles 124 code lines of the binary and embeds no child image. The package
keeps one binary target, because both console audits hold the one install call
to `slime-root/src/main.rs`. `just root_size_check` leaves the 1007 lines that
only the fixture root compiles out of the series, which holds 33073.

Three recipes boot the fixture through `scripts/check/check-sel4-root-boot.py`:

| Recipe | Platform |
|---|---|
| `just sel4_root_boot_check` | `qemu-arm-virt` |
| `just x86_64_sel4_root_boot_check` | `qemu-pc99` |
| `just riscv64_qemu_check` | `qemu-riscv-virt` |

The fixture stays the subject of all three. Only its place in the source
changes.

### Stage two: 23000 lines

The ioResource and sharedBuffer roles leave. Both map memory into another
task's VSpace, so a design under `docs/plans/` must be accepted first. Stage
two does not start before that.

28660 minus the 6055 lines of the two roles is 22605. The target of 23000
allows about 395 lines of what stays from every leaving role.

## Alternatives

- **Restructure without moving anything.** The trusted source stays the same
  size.
- **Stop after stage one.** The series ends near 28850 lines. This is the
  outcome if the stage-two design is rejected.
- **A target below 23000.** The `object_allocator` module and its tree hold
  8164 code lines. A lower target needs a simpler adaptive-memory model, which
  is a separate design decision.
- **A rewrite.** It would discard the QEMU evidence of every plane.

## Consequences

- Each leaving role gets its own work item. The item's acceptance bounds the
  role's count. A planning pull request lowers the role's ceiling in
  `scripts/check/check-root-size.py` after the implementation lands: CI runs
  the gate on every pull request, so a ceiling below the count on `main` would
  fail every one of them.
- A component that receives a role holds only the authority that role needs.
- Code that leaves the root stays in the trusted base of every component that
  depends on it. The line count measures the root's own source, not the
  system's trusted base.
- The count excludes the five crates the root links. The gate pins their
  names, so a sixth crate is a grader change.
- A timer wake no longer passes through the root once the timer role leaves.

## Revisit when

- What stays from the leaving roles exceeds the allowance of a stage. The
  owner then restates that stage's target. This happened for stage one on
  2026-11-22.
- The verified kernel profile lands. The kernel then exports no generic timer
  to EL0, and the owner weighs a timer component again.
- The stage-two design is rejected. The target then stays at 28850.
- A sixth crate enters the root's dependencies.
