# 19. MPK/PKU lightweight compartments

| | |
| --- | --- |
| Status | parked |
| Route | hardware |
| Depends on | [Hardware H track](../../roadmap/04-platform-hardware.md); explicitly an optional optimization that does not block the track's exit conditions |
| Enables | a third isolation tier between full components and same-address-space code |
| Now | Paper: compartment model, fault semantics, and the criteria for when a boundary may use PKU instead of a component boundary. |

## Motivation

A third isolation tier between full components and same-address-space
code for latency-sensitive boundaries, using user-space protection keys
available on the target CPU. Full component isolation costs address
spaces and context switches; some boundaries (a hot parser, a codec, an
in-process plugin) want memory protection without that cost. PKU gives
page-granularity write/execute disablement switchable from userspace —
cheap enough for per-call boundaries.

## What exists today

- The two existing tiers are: separate components (address-space
  isolation, channel-only interaction) and same-address-space code
  (no isolation). The capability model governs the first; the second
  has no authority story at all.
- The target CPU (Framework's AMD Krackan) provides PKU-class user
  protection keys. [INFERENCE: PKU availability on the specific part
  should be confirmed during Hardware H-track bring-up.]
- Nothing in the kernel models protection keys; the entry is
  explicitly an optional Hardware H-track optimization, not a blocking feature.

## Design sketch

A compartment shares its owner's address space but holds a distinct
protection key; data pages are tagged so that exactly one compartment's
key permits access at a time, and crossing the boundary is a
key-switching call gate rather than an IPC. The authority question:
does a compartment hold its own capability set (a lightweight principal)
or operate strictly under its owner's grants with memory isolation as
the only separation? The latter preserves the current model — one
capability table entry per component — and is the conservative reading;
the former makes compartments first-class principals with matrix
consequences.

Fault semantics must match the component model's discipline: a PKU
violation in one compartment is reported as a structured fault without
terminating the other — the register's exit condition — so the fault
classification vocabulary (M5.6's exit/fault/timeout distinction)
extends rather than forks.

The entry's real design output is admission criteria: which boundaries
may use PKU (latency-critical, same trust domain, no independent
rollback of state) and which must remain full components (independent
authority, independent failure domain in the generation graph).

## Open questions

- Are compartments separate principals (own capabilities) or memory
  domains under their owner's grants?
- Who may switch keys — is the call gate a kernel-mediated operation
  or pure userspace (with what confused-deputy exposure)?
- How does a compartment fault interact with supervision
  ([entry 8](08-declarative-supervision.md)) — restart the owner
  component, or the compartment alone?
- State: may a compartment hold `preserve`d state bindings, or is all
  durable state the owner's?

## Exit-condition sketch

Two compartments share an address space; a PKU violation in one is
reported as a structured fault without terminating the other.

## Probe guidance

Paper until Hardware H-track bring-up: the principal question (separate
authority vs owner's grants) answered as a matrix impact note, plus the
admission criteria list. A microbenchmark of PKU switch cost on the target
part belongs to Hardware H-track bring-up, not to this entry's promotion.
