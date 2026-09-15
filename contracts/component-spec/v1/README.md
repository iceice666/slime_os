# Component specification contract, format 1

This directory defines the component-level model: what a Slime OS component
*is*, independently of how any one generation composes it. `schema.zt` is the
normative shape, `components/*.zti` is the corpus, and
`scripts/check/check-component-spec.py` owns semantic admission.

## Ownership

A spec describes a component independently of its deployment. A system spec
composes instances and authority; the generation manifest is derived from those
records. Runtime binding resolution reads the admitted instance's bindings.
The [component/system/image reference](../../../docs/architecture/component-system-image.md)
owns these boundaries; an executable image describes mapping and target
qualification, not component identity, interfaces, lifecycle, or grants.

## What a record declares

`schema.zt` declares identity (`name`, `componentType`, `version`, `owner`),
`purpose`, `implementation`,
capabilities (`provides`/`requires`), `interfaces`, `dependencies`,
`communication`, `configuration`, `lifecycle`, `runtime`, `health`,
`compatibility`, and `test`.

Three choices are worth stating outright:

- **`componentType`, not `type`.** `type` is Zutai's type-declaration keyword.
  The value set is the manifest's own `Executable.role` vocabulary, so a spec and
  the manifest that composes it cannot disagree on what kind of thing the
  component is.
- **QoS is reused, not redefined.** `QosPolicy` is
  `contracts/generation-manifest/v1/schema.zt`'s `FabricParticipant` QoS fields spelled
  identically, carrying the same closed value sets and the same two agreement
  rules the generation builder enforces (`retained` durability needs a retained
  depth; `manual` liveliness needs a lease). Two vocabularies could only be
  compared by translation, and a translation table is where they would diverge.
- **`implementation.provider` is closed and includes `undeclared`.** See below.
- **External implementations are content-bound, not path-bound.**
  `implementation.contentHash` is empty for `workspace` and `undeclared`, and
  exactly one lowercase SHA-256 for `external`. The generation builder receives
  the operator-local ELF path separately, verifies the bare bytes against this
  digest, and only then admits and signs the ordinary generation.

## Identity

SHA-256 over `identityDomain` (`slime-component-spec-v1:`) followed by the
normalized record bytes: sorted-key, whitespace-free, ASCII-escaped UTF-8 JSON
plus one trailing newline. That is `contracts/interface-schema/v1`'s convention
verbatim rather than a second normalizer, so a component identity and an
interface identity are computed the same way. The gate proves the identity is
invariant under source field order and source formatting, and that it changes
when any field's content does.

## Declared components without an implementation

The frozen reference generation retains component identities without a current
implementation. `provider = "undeclared"` represents that explicitly rather
than inventing a source path. `scripts/check/check-component-spec.py` owns the
expected reference set and checks it against discovered component crates; a
reference identity gaining or losing an implementation must update both facts.
Newer product compositions are validated independently, not projected backward
onto the frozen generation.

Component identity and implementation binary name are separate. For example,
`generation-manager` uses `sel4-generation-manager`, and `filesystem-service`
uses `sel4-filesystem-service`; `implementation.binary` declares that mapping.
The [original contract note](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/contracts/component-spec/v1/README.md)
preserves the initial missing-provider account.

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
- `test.requiredTestEnvironment` must be a real Justfile target, and
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

Reference records are cross-checked against
`contracts/generation-manifest/v1/fixtures/valid.zti`: type, owner, health,
dependencies, resource budgets, target, and declared interfaces. Fabric roles
are authorized against the committed compositions, not only the reference
graph: a component may have a role in one composition and not another. An
interface entry must correspond to a participant role, an interposition hop,
fabric ownership, or a route-worker partition declared by a composition. A
spec must agree with the authority declared by its consuming compositions.
Malformed-case checks pair each refusal with an admitted baseline of the same
shape so an unrelated guard cannot make a negative control pass.

## Scope boundary

This contract describes and validates a component, not a deployment or build
result. System specifications own composition and grants; runtime binding
resolution uses the admitted instance's authority; executable admission and
SDK publication have separate contracts. The
[component, system, and image reference](../../../docs/architecture/component-system-image.md)
maps those owners. The [original contract note](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/contracts/component-spec/v1/README.md)
preserves CP0's delivery scope and validation history.

Run the focused gate with:

```sh
just component_spec_check
```
