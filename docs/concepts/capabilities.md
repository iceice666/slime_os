# Capabilities

A capability is unforgeable authority to use one specific object or service:
a named thing plus a rights mask, held in a slot of a component's capability
table, issued and authenticated by the root. If a component does not hold a
capability for something, that thing does not exist for it — there is no
name it could guess, no path it could open, no flag it could set.

This page is the mental model. The exact kinds, rights bits, gated
operations, and bounds are [`../capability-matrix.md`](../capability-matrix.md),
which is updated in the same change as the surface it describes; numbers
quoted anywhere else are stale by construction.

## The model in five rules

1. **Authority is explicit and enumerable.** A component's complete authority
   is its grant list in the generation manifest plus whatever was later
   transferred or derived from it. "Why can this component do X?" always has
   a finite answer rooted in the manifest.
2. **One right names one root-checked operation.** A rights bit is not a
   policy concept ("trusted", "admin"); it gates exactly one operation the
   root serves. Policy — who should hold what — lives in the manifest and in
   userspace services, never in the rights vocabulary.
3. **Rights only narrow.** Derivation produces a copy with equal or fewer
   rights; nothing widens at runtime. A component wanting more authority than
   it was granted has exactly one option: ask a component that legitimately
   holds it (the powerbox pattern), and receive a narrowed, provenance-carrying
   delegation.
4. **Creation is root-only, and bounded.** Userspace cannot forge object
   identities; it can only hold, derive, and transfer. The few runtime mints
   that exist go through a factory capability and a generation-declared
   budget. Every resource table has a hard bound — "unbounded" is treated as
   a bug, not a strategy.
5. **Receiving costs nothing and proves everything.** A capability arrives
   with exactly the rights the sender named and the root authenticated. The
   receiver need not — and cannot — trust the sender's description of it.

## How authority moves

Three distinct movements, deliberately not interchangeable:

- **Grant** (boot-time): the generation manifest installs capabilities into
  an instance's declared slots at construction. This is the root of all
  provenance.
- **Spawn grant** (non-consuming): a parent gives a child narrowed copies of
  capabilities it holds, at spawn, checked by the root against
  transferability. The parent keeps its own.
- **Transfer** (consuming): the export/import protocol moves a capability
  from one holder to another across a declared channel, with the root
  authenticating kind and rights from the request itself — never from bytes a
  component wrote.

Deliberately absent: ambient inheritance, capability lookup by name, and any
global registry. If a movement path exists that is not one of these three,
that is a defect.

## What this buys

- **Confinement by construction:** a compromised component's blast radius is
  its grant list. Prompt injection, memory corruption, or plain bugs cannot
  widen authority — only misuse what was already granted.
- **Interposition:** because nothing is ambient, any capability can be
  replaced by a proxy (a membrane) without the component knowing — auditing,
  recording, dry-runs.
- **Legibility:** the authority graph is static data. Admission checks it
  against hard ceilings before any component runs; tooling can answer
  authority questions without executing anything.

## Related

- Enforcement sites: rights types in `slime-root/src/graph.rs`, checks in the
  dispatcher and owning mechanism modules.
- The vocabulary itself is generated from
  `contracts/generation/v5/schema.zt` — see [Contracts](contracts.md).
- [Channels](channels.md) — the transfer path in context.
