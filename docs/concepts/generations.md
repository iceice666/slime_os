# Generations

A generation is a complete bootable system as one deterministic, versioned,
integrity-checked artifact: every executable's bytes, the instance graph,
every capability grant, every resource budget, persistent-state bindings,
health policy, and rollback metadata. It is the unit of deployment, of
atomic upgrade, and of rollback — for system services and (by design) for
agents alike.

## One artifact, not a package set

Atomicity in Slime OS covers more than files. Changing anything — a
component's code, a grant, a budget, an agent's prompt — produces a *new*
generation with a new identity; there is no in-place mutation of a running
system. The manifest format is a versioned Zutai contract
(`contracts/generation/`, decoded by `boot-contracts/src/generation.rs`),
and its construction (`scripts/build/build-generation.py`) is deterministic:
same inputs, byte-identical generation, so identity can be a digest and any
drift is an alarm.

The generation is also where *policy about authority* lives. The capability
model supplies the vocabulary; the generation is the sentence: which
instance holds which grant at which rights, who may spawn whom under what
budget, which endpoint edges exist at all. Admission enforces hard bounds on
all of it before anything runs — a graph that does not fit is refused
whole, never partially constructed.

## Admission: trust nothing, verify at the boundary

At boot the root decodes and admits the embedded (or selected) generation:

- format version accepted or the generation is **refused, never migrated** —
  a superseded wire format counts as a failed generation, which is what makes
  format bumps rollback-safe by refusal;
- every executable hash-verified and qualified for exactly this target
  profile, before a byte is mapped;
- the declared graph costed against the root's real ceilings, up front;
- budgets (shared buffers, spawn, private memory) read from the manifest
  into the tables that will enforce them, deny-by-default for anyone the
  manifest does not name.

After admission, the manifest is the running system's authoritative
self-description: introspection operations answer from it, and the gates
assert the boot against it.

## Selection, health, and rollback

Activation never overwrites the running generation in place, and the
previous known-good generation always remains selectable. The boot selector
walks a small, exhaustively model-checked state machine
(`contracts/bootstate/`): a pending generation gets a bounded number of
attempts, spent *before* decoding (so an undecodable candidate cannot retry
forever); it becomes known-good only after userspace health confirmation —
never merely by booting; exhausted attempts roll back to known-good
automatically.

Generation *management* — staging, selection, promotion, rollback commands,
recovery — is deliberately not root mechanism. It is userspace components
holding block capabilities, mediated like any other client
(`components/bins/sel4-generation-manager/` and its siblings).

## State crosses generations deliberately

Persistent state is declared in the manifest as bindings with an owner, a
schema version, and an upgrade/rollback policy (snapshot before upgrade,
discard on rollback). State transitions ride the same atomic activation as
code — there is no way for data migration to half-happen against a
half-upgraded system. This is also the agent-memory story: an agent's
long-term state is a state binding like any other, versioned and rolled back
with the generation that owns it.

## Working rules

- The built format and its invariants are pinned by `just contracts_check`
  and `just generation_check`; run both for any generation-format or builder
  change.
- Grants, slots, and budgets for a component come from the fixtures under
  `contracts/generation-manifest/v1/fixtures/` — change authority there, not in
  component code, and expect the boot-layout gate to show the diff.
- Retired format versions keep their directories as decodable history for
  the bounded rollback window; deleting one is a format decision, not a
  cleanup.

## Related

- [Contracts](contracts.md) — the schema discipline the manifest follows.
- [Capabilities](capabilities.md) — what the grants convey.
- Bounds and grammar for what a generation may declare:
  [`../capability-matrix.md`](../capability-matrix.md).
