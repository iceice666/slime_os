# Contracts: Zutai schemas as the single source of truth

Every serialized format that crosses a persistence, process, or boot
boundary — on-disk formats, IPC messages, manifests, handoff structures — is
defined once, as a versioned Zutai schema under `contracts/`, and everything
that reads or writes that format is generated from or validated against it.

This is the repository's strictest rule, and the one most likely to trip a
newcomer, because the reflex it forbids is so normal elsewhere: *do not
hand-write a wire format.* No `#[repr(C)]` structs as wire truth, no
hand-counted field offsets, no `struct.pack` layouts, no second schema
language. Purely in-memory types are exempt; the moment bytes cross a
boundary, they need a contract.

## The shape of a contract

```
contracts/<name>/v<N>/
├── schema.zt        # the source of truth: types, constants, wire layouts
├── gen_rust.zt      # pure renderer producing the Rust bindings
└── fixtures/        # (where applicable) deterministic instances, e.g. generations
```

`schema.zt` declares the logical types *and* the explicit wire layout —
field order, widths, signedness — and the generator cross-checks the two
against each other before emitting anything. `contracts/block/v1/schema.zt`
is a small, readable example.

## The one-way flow

```
contracts/<name>/vN/schema.zt        ← edit here
        │  scripts/generate/generate-<name>-bindings.py   (just <name>_gen)
        ▼
generated Rust                        ← never edit here
  components/proto/src/<name>.rs
  boot-contracts/src/generated/
```

Generated files open with `// @generated ...; do not edit.` and mean it: they
are build outputs that happen to be checked in. Editing one "works" until
the next regeneration silently reverts it. The correct move is always:
change `schema.zt` (or `gen_rust.zt`), run the matching `just *_gen`, and
commit schema and output together. `just contracts_check` validates every
contract and generated binding agree.

One consumer is generated at build time rather than checked in:
`components/bins/build.rs` derives command and fabric profiles from the
generation fixture into `OUT_DIR`.

## Why this is worth the ceremony

- **Layout disagreement becomes impossible**, not unlikely. Root, runtime,
  components, and host tooling all import the same generated constants; a
  renumbered operation or moved field cannot leave two of them disagreeing,
  because there is only one declaration. The rights vocabulary and the
  syscall label table earned this rule the hard way — each was hand-declared
  at dozens of sites before being consolidated (B57, B59).
- **Versioning is structural.** A contract's directory is its major version.
  An incompatible change is a new `vN` directory; the superseded schema stays
  behind, type-checked as format history, and old formats are *refused* by
  decoders rather than migrated — which is what makes rollback safe.
- **Typed interposition.** Because every cross-boundary message has a schema,
  audit, record, and replay tooling sees typed messages, not opaque bytes —
  and an agent tool schema and a system IPC schema can literally be the same
  artifact.
- **Determinism.** Zutai evaluation is pure and lazy; the same sources
  produce byte-identical outputs, which is what lets generation identity be
  a hash and drift be an alarm rather than a shrug.

## Working rules

- New boundary format → new schema under `contracts/`, generator script,
  `just *_gen` target, and a `just contracts_check` pass. Look at a recent
  small contract and copy its structure.
- Changing an existing format → edit the schema, regenerate, and update the
  reference doc the change touches ([`../syscall-abi.md`](../syscall-abi.md)
  or [`../capability-matrix.md`](../capability-matrix.md)) in the same
  change. For the syscall ABI this is machine-enforced.
- Reading generated code to understand a format is fine and encouraged; the
  schema tells you *what*, the generated file tells you *where the bytes
  land*.

## Related

- Zutai itself: [`deps/zutai/docs/`](../../deps/zutai/README.md) — the
  language manual and specification.
- [Generations](generations.md) — the largest and most consequential
  contract.
