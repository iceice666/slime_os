# Orientation

One page of bearings before you build anything. The root
[`README.md`](../../README.md) is the canonical statement of vision, status,
and non-goals; this page only tells you how to hold the project in your head.

## What you are looking at

Slime OS is an experimental capability-based operating system: a Rust
`no_std` userspace graph running on the upstream seL4 microkernel. It is not
a Linux distribution, not a Unix clone, and not trying to become either —
POSIX-shaped things (paths, environment, fork, signals, ambient authority)
are deliberately absent from the native model.

The system is built from five first-class concepts, each with a concept page:

| Concept | One line | Page |
| --- | --- | --- |
| Component | isolated, versioned executable; fault containment unit | [components](../concepts/components.md) |
| Capability | unforgeable, explicit, narrowing-only authority | [capabilities](../concepts/capabilities.md) |
| Channel | typed IPC that can carry capabilities | [channels](../concepts/channels.md) |
| State | owned, schema-versioned persistent data | in [generations](../concepts/generations.md) |
| Generation | the whole bootable graph as one atomic, rollbackable artifact | [generations](../concepts/generations.md) |

One cross-cutting rule binds them: every format that crosses a process,
persistence, or boot boundary is a versioned Zutai schema under `contracts/`,
with all bindings generated from it — see
[contracts](../concepts/contracts.md) before you write any wire code.

## How to orient in the tree

Three files answer most "where is..." questions:

- [`AGENTS.md`](../../AGENTS.md) — the code map: execution path, a
  task-to-file index routing every kind of change to its owning module, and
  the navigation traps.
- `Justfile` — every build, run, check, and regeneration command
  (`just --list`).
- [`roadmap/README.md`](../../roadmap/README.md) — status, invariants, and
  what is actually open. The backlog (`roadmap/00-backlog.md`) sits ahead of
  all milestone work.

## The house epistemology

Two habits distinguish this repository; adopting them early saves friction:

- **Evidence over intention.** A behavior exists when a gate observes it on
  a real boot, not when code for it lands. Gates fail closed: missing
  hardware evidence is a failing check, never a skip. Claims about how
  conclusions were reached live in `devlog/`, kept separate from the
  roadmap's outcomes.
- **Refusal over accommodation.** Malformed data, wrong-target binaries,
  superseded formats, unknown operations, unbudgeted requests — all are
  refused at the boundary rather than tolerated, migrated, or guessed at.
  When you meet a check that seems obstinate, it is usually load-bearing.

## Suggested path

1. [Build and run](02-build-and-run.md) — get a boot on your machine.
2. [Boot walkthrough](03-boot-walkthrough.md) — understand what you saw.
3. The five concept pages, in table order above.
4. [Your first change](04-first-change.md) — the workflow, end to end.
5. [Add a component](05-add-a-component.md) — take new code through
   declaration, composition, authority, and QEMU evidence.
