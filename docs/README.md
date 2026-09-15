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
   to end: routing, gates, contracts, evidence, and owning documentation.
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

## Architecture

- [Subsystem architecture](architecture/README.md) — current IPC, fabric,
  time/waiting/lifecycle, private memory, component/image boundaries, target
  profiles, and userspace I/O, with owning source and qualification limits.

## Reference

These two files are load-bearing and have update discipline: each must change
in the same commit as the surface it describes.

- [`capability-matrix.md`](capability-matrix.md) — the object-by-rights
  surface: every capability kind, every rights bit, every gate, every bound.
- [`syscall-abi.md`](syscall-abi.md) — the component ABI: operation labels,
  operand packing, reply convention, error model, CSpace layout.
  Label coverage is machine-checked by `just contracts_check`.

## Decisions

- [`decisions/`](decisions/README.md) — important long-lived cross-module
  choices, including the accepted development-record ownership split. Decision
  records are not required per PR and carry no work-item state.

## Plans

- [`plans/`](plans/README.md) — unimplemented designs, delivery requirements,
  and qualification requirements, without work-item state. The
  [roadmap classification](../roadmap/README.md) identifies retained detail
  not yet extracted and historical source awaiting verified archival.

## Exploration

- [`directions/`](directions/README.md) — the register of exploratory
  directions that follow from the vision but are not committed work.

## Everything else lives elsewhere

| Looking for | Go to |
| --- | --- |
| Work-item state: what is done, open, blocked, deferred | `just tasks_list`, `just tasks_next` over the canonical store in `.tasks/items/` |
| Current subsystem ownership and invariants | [`architecture/`](architecture/README.md); unfinished requirements in [`plans/`](plans/README.md); retained detail classified in [`roadmap/`](../roadmap/README.md) |
| Known defects and regressions (open ones precede milestone work) | The `backlog`-tagged canonical work items; the old frozen index is historical only |
| How userspace drivers receive device/MMIO/IRQ/DMA authority, and what IO gates prove | [Userspace I/O substrate](architecture/io-substrate.md); the authority surface is in [`capability-matrix.md`](capability-matrix.md) and operations in [`syscall-abi.md`](syscall-abi.md) |
| How a historical conclusion was reached | [Private history archive](history.md), pinned by full commit and original path; ordinary changes need no investigation |
| Code map and task-to-file index | [`AGENTS.md`](../AGENTS.md) |
| The schemas every persisted or cross-process format is generated from | `contracts/` |
| Build, test, and gate commands | `Justfile` (`just --list`) |
