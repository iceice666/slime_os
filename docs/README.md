# Slime OS documentation

Routing page. Nothing here is canonical on its own; every page below either
narrates or points at a source that is.

## If you are new

Read the root [`README.md`](../README.md) first — what Slime OS is, the five
first-class concepts, current status, and non-goals. Then:

1. [Orientation](getting-started/01-orientation.md) — how to hold the project
   in your head, and where everything lives.
2. [Build and run](getting-started/02-build-and-run.md) — prerequisites,
   first boot under QEMU, and what the common failures mean.
3. [Boot walkthrough](getting-started/03-boot-walkthrough.md) — what actually
   happens between `just run` and the `SLIME_GRAPH HEALTHY` marker, and how
   the verification gates consume that serial record.
4. [Your first change](getting-started/04-first-change.md) — the workflow end
   to end: routing, gates, contracts, devlog.
5. [Add a component](getting-started/05-add-a-component.md) — create the crate,
   declare it, compose it, grant authority, and prove it under QEMU.

## Concepts

The five first-class ideas, as mental models. These pages state invariants
only; every number, bit, label, and bound lives in the reference documents
below, which have update discipline these pages do not.

- [Components](concepts/components.md) — the isolation and fault-containment
  unit, and why it is not a process.
- [Capabilities](concepts/capabilities.md) — explicit, unforgeable,
  narrowing-only authority.
- [Channels](concepts/channels.md) — the two IPC paths, and capabilities in
  motion.
- [Contracts](concepts/contracts.md) — Zutai schemas as the only source of
  truth for every boundary-crossing format.
- [Generations](concepts/generations.md) — the atomic, rollbackable unit of
  deployment, including persistent state.

## Reference

These two files are load-bearing and have update discipline: each must change
in the same commit as the surface it describes.

- [`capability-matrix.md`](capability-matrix.md) — the object-by-rights
  surface: every capability kind, every rights bit, every gate, every bound.
- [`syscall-abi.md`](syscall-abi.md) — the component ABI: operation labels,
  operand packing, reply convention, error model, CSpace layout.
  Label coverage is machine-checked by `just contracts_check`.

## Exploration

- [`directions/`](directions/README.md) — the register of exploratory
  directions that follow from the vision but are not committed work. Read its
  rules before adding an entry; numbers are never reused.

## Everything else lives elsewhere

| Looking for | Go to |
| --- | --- |
| Canonical plan, milestone status, acceptance criteria | [`roadmap/`](../roadmap/README.md) |
| Known defects and regressions (resolve before milestone work) | [`roadmap/00-backlog.md`](../roadmap/00-backlog.md) |
| How a conclusion was reached: investigations, evidence, decisions | [`devlog/`](../devlog/README.md) |
| Code map and task-to-file index | [`AGENTS.md`](../AGENTS.md) |
| The schemas every persisted or cross-process format is generated from | `contracts/` |
| Build, test, and gate commands | `Justfile` (`just --list`) |
