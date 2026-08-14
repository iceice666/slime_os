# `sel4-supervision.zti` — the B16 supervision-plane generation

An eighth seL4 generation, beside [`sel4.zti`](sel4.md),
[`sel4-channel.zti`](sel4-channel.md), [`sel4-loan.zti`](sel4-loan.md),
[`sel4-spawn.zti`](sel4-spawn.md), [`sel4-sample.zti`](sel4-sample.md),
[`sel4-stream.zti`](sel4-stream.md), and the frozen x86
[`valid.zti`](valid.zti). It declares the smallest graph that can reach backlog
**B16**'s exit condition: *a graph that creates more than `MAX_RECORDS` tasks
over its lifetime still answers `supervision_status` correctly for every live
handle.*

## Why an eighth generation

The same mechanical reason as the six before it: `init.rs` selects its scenario
with `option_env!`, resolved at compile time, so one component build cannot
serve two gates. A separate generation — and so a separate image — is required
rather than preferred.

## The three components

### `init` — the parent

Holds two `exec | spawn` grants. `spawnBudget`
stays at 4, as `sel4-spawn.zti` uses: the budget bounds children *live at once*
through `TaskTable::live_children`, which is derived from the table rather than
from a counter, so a loop that spawns and reaps sequentially never approaches it
however long it runs.

### `supervision-child` — the loop child, and why it is new

Thirty-five of these are created over the boot: the 33-iteration loop plus the
two whose handles are held across it (one retained, one held by a native export
ticket). It writes one marker and returns, and it is the only component in the
tree that takes **no endpoint**.

That is the whole reason it exists rather than reusing `sysinfo`. A child that
reads a launch context needs an endpoint. A per-child endpoint loop would spend
unrelated kernel objects while this fixture is intended to cross the bounded
supervision-record lifetime. With this child, the only dynamic endpoint is the
carrier used to export and later import the held supervision capability.

The cost is stated plainly: this weakens the "no new component binary" property
the other planes have. It does not touch the frozen oracle, which is `kernel/`
rather than `components/`, but it does mean `contracts_check` and
`generation_check` carry a real duty here — they are what confirms a bin unused
by the oracle perturbed neither contract validation nor generation identity.

### `sysinfo` — declared, unmodified, and not the subject

The root launches every component a generation declares (P5.2), so this boot
also starts one unconfigured `sysinfo` that no one hands a channel to. It exits
non-zero, which is expected rather than a failure; every marker the gate asserts
names the spawn that produced the task it is about.

## Why `init-supervision-child` is `transferable = true`

This is the one field in the fixture that is load-bearing and non-obvious, and
it is a second instance of the B10 fixture/layout coupling.

The gate must park a supervision handle in `Transit` **across** the crossing —
the state where a capability is held by no table at all, and so the case a sweep
reading only live capability tables frees by mistake. Moving a handle there
requires `cap_transfer`, which gates on the mover holding `RIGHT_TRANSFER` on
the capability itself. (Not `send`'s capability attachment: that path gates on
`Resource::is_transferable`, which answers true for a loan and nothing else, by
design.)

A supervision handle carries `RIGHT_TRANSFER` only when the **executable** the
spawn resolved carried it — `SpawnPlan::transferable_supervision`, read from the
executable rather than from any grant. So the authority to move a child's
supervision handle is declared here, on the parent's grant for the child's
image.

Because the boot layout and the fixture must agree bit for bit, the matching row
in `scripts/build/boot_layout.py` is `0x1000c` rather than the usual `0x10008`
for an executable — the extra `0x4` is exactly `RIGHT_TRANSFER`. Changing one
without the other is refused at admission with `RightsMismatch`, which is B10
working as intended.

## Why the graph declares no channel edge

Like `sel4-spawn.zti`, and for the same reason: this plane's subject is what a
parent hands over at spawn, not what the generation pre-wires. Here it needs
exactly one edge, and init holds **both** ends — it moves the handle to itself
and collects it after the loop. That is deliberate rather than a shortcut: the
capability-transfer path needs a peer that collects a capability, and every
unmodified component either ignores the capability array or never receives at
all, so a second component would have to be written for the purpose. Init as its
own peer keeps the in-flight window open across the loop without inventing a
binary whose only job is to wait.
