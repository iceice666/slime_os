# Component specification contract, format 1

This directory defines CP0's component-level model: what a Slime OS component
*is*, independently of how any one generation composes it. `schema.zt` is the
normative shape, `components/*.zti` is the corpus, and
`scripts/check/check-component-spec.py` owns semantic admission.

## Why this exists

Before CP0 the repository had no component-level specification at all.
`contracts/generation/v1/schema.zt`'s `Executable`/`Instance` pair was the only
description of a component anywhere, so "what this component is" and "how this
generation composes it" were the same hand-authored text — the coupling
[B70](../../../roadmap/00-backlog.md) opens. `contracts/component/v2`'s
`ImageHeader` did not close the gap either: it carries only target-qualification
fields (`magic`, `architecture`, `abi`, `page_profile`, `required_features`, a
segment table), so nothing described a component's identity, authority,
interfaces, or lifecycle.

CP0 separates the two. A spec describes the component; a manifest composes an
instance of it. CP1 derives the manifest from these records rather than
hand-authoring both, and CP2 moves slot resolution off `build.rs`-private
compile-time constants.

## What a record declares

The twelve sections `spec/requirement-document-v0.6.md` §2.1 names: Identity
(`name`, `componentType`, `version`, `owner`), `purpose`, `implementation`,
Capability (`provides`/`requires`), `interfaces`, `dependencies`,
`communication`, `configuration`, `lifecycle`, `runtime`, `health`,
`compatibility`, and `test`.

Three choices are worth stating outright:

- **`componentType`, not `type`.** `type` is Zutai's type-declaration keyword.
  The value set is the manifest's own `Executable.role` vocabulary, so a spec and
  the manifest that composes it cannot disagree on what kind of thing the
  component is.
- **QoS is reused, not redefined.** `QosPolicy` is
  `contracts/generation/v1/schema.zt`'s `FabricParticipant` QoS fields spelled
  identically, carrying the same closed value sets and the same two agreement
  rules the generation builder enforces (`retained` durability needs a retained
  depth; `manual` liveliness needs a lease). Two vocabularies could only be
  compared by translation, and a translation table is where they would diverge.
- **`implementation.provider` is closed and includes `undeclared`.** See below.

## Identity

SHA-256 over `identityDomain` (`slime-component-spec-v1:`) followed by the
normalized record bytes: sorted-key, whitespace-free, ASCII-escaped UTF-8 JSON
plus one trailing newline. That is `contracts/interface-schema/v1`'s convention
verbatim rather than a second normalizer, so a component identity and an
interface identity are computed the same way. The gate proves the identity is
invariant under source field order and source formatting, and that it changes
when any field's content does.

## Two components are declared without an implementation

`generation-list` and `storage-store-probe` are declared in
`contracts/generation/v1/fixtures/valid.zti`, in every
`contracts/boot-layout/v1/fixtures/*.layout`, and (for `generation-list`) in
`components/bins/src/default_fabric_profile.rs`, but no `[[bin]]` target or
source file exists for either. Both were deleted as unreachable clients of
retired syscalls — see
[`devlog/2026-08-10-b44-policy-labels-deleted/`](../../../devlog/2026-08-10-b44-policy-labels-deleted/index.md)
and
[`devlog/2026-08-10-b43-block-service-endpoint/`](../../../devlog/2026-08-10-b43-block-service-endpoint/index.md)
— while their manifest entries stayed.

`provider = "undeclared"` records that fact rather than inventing a source file
for them. The gate pins the set to exactly those two and refuses a record that
claims to be undeclared while its binary exists, so a third component losing its
implementation fails this gate instead of passing silently. Deciding whether to
delete the manifest entries or build the missing components is not CP0's call;
recording the gap accurately is.

Two further names resolve to a binary that is *not* spelled like the component:
`generation-manager` is built by `sel4-generation-manager` and
`filesystem-service` by `sel4-filesystem-service`. `implementation.binary` makes
that a declared fact instead of a convention a reader must rediscover.

## Validation levels

Zutai decoding validates the closed record shape.
`scripts/check/check-component-spec.py` owns semantics, and every rule it
enforces is grounded in real repository state rather than in a literal:

- interface references resolve against
  `contracts/interface-schema/v1/interfaces/*.zti`, and a reference's `tag` is
  checked against that interface's own `kind` — a stream cannot be tagged as a
  command;
- `communication.semantic` is *derived* from the referenced interfaces' kinds and
  compared, so it cannot claim a semantic no interface backs;
- every lifecycle state is drawn from the closed set, appears in canonical order,
  includes every required state, and each conditional state (`Configure`,
  `Ready`, `Degraded`, `Stop`) is declared exactly when the fact it depends on
  is;
- `runtime.resource` is bounded by the constants the builder and root already
  enforce (`COMPONENT_MAX_STACK_BYTES`, `MAX_SPAWN_BUDGET`, `MAX_CHILD_THREADS`,
  `MAX_TOTAL_PAGES`);
- `test.requiredTestEnvironment` must be a real Justfile target, on the same
  terms `just devlog_check` enforces for a devlog's `Gates` front matter, and
  `test.passFailCriteria` must appear in a string literal `ast`-parsed out of
  that gate's own check script — so a criterion is text the gate matches on
  rather than any fragment of its source;
- `compatibility.interface` must be a `contracts/<name>/v<N>` path declaring a
  `schema.zt`, `compatibility.platform` must equal
  `runtime.executionEnvironment`, and the `dependency`/`resource`/`runtime`/`qos`
  modes are each derived from a fact the record already states rather than
  chosen;
- every `configuration` parameter must name a `runtime.resource` field and
  default to the value that field holds, so configuration cannot drift from the
  requirement it configures;
- corpus-wide: names and identities are unique, dependencies resolve, the
  dependency graph is acyclic, and every required capability kind is provided by
  some component — except `executable`, whose provider is the hash-verified
  generation module rather than a component.

The gate also cross-checks each record against
`contracts/generation/v1/fixtures/valid.zti` field by field: type, owner, health,
dependencies, spawn budget, stack bytes, extra threads, shared-buffer budget,
target, `provides`/`requires` derived from the manifest's `grants[]`, and every
fabric route role with its exact QoS values. The fabric projection runs both
ways: a declared interface entry must be authorized by a participant role, an
interposition hop, `fabricComponent` ownership, or a route worker's partition, so
a record can neither omit a role the graph gives it nor invent one it does not. A
spec free to disagree with the generation that composes it would be
documentation, not a contract, and CP1 could not derive one from the other.

37 named malformations are refused, each paired with an admitted baseline of the
same shape so no arm can pass by tripping an unrelated guard — the discipline
[B67](../../../roadmap/00-backlog.md) established after two negative controls
were found to be structurally incapable of failing.

## Scope boundary

CP0 declares and validates the model. It does not derive generation manifests
from it (CP1), move slot resolution to runtime (CP2), split components into
independent crates (CP3), admit externally built artifacts (CP4), or prove
out-of-tree development (CP5). No component source, root code, or generation
byte changes in CP0.

Run the focused gate with:

```sh
just component_spec_check
```
