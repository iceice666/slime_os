# Generation manifest format 1

This directory defines the first typed cross-project contract between Zutai and Slime OS. `schema.zt` is the normative host-side structural schema. The fixtures prove that the pinned Zutai compiler accepts the supported shape and rejects a structural mismatch.

Format 1 describes:

- target and generation identity;
- immutable content-addressed objects;
- components and dependency names;
- directed capability grants;
- persistent-state ownership and policy;
- boot health policy.

The schema intentionally uses closed records, lists, scalars, and text-backed enumerated identifiers. This keeps `.zti` inert, deterministic, and directly decodable by Zutai without choosing the boot-time binary encoding prematurely.

Format 1 serializes enumerated identifiers as text. The builder accepts only:

- object `kind`: `kernel`, `bootstrap`, `component`, `resource`;
- component `role`: `init`, `service`, `driver`, `application`;
- state `policy`: `immutable`, `ephemeral`, `preserve`, `snapshotBeforeUpgrade`, `discardOnRollback`.

This is intentional: Zutai `.zti` bare atoms decode as atom-singleton types, while a heterogeneous manifest needs one stable field type for each closed set. The builder enforces the closed sets until a later manifest encoding supplies explicit numeric or tagged discriminants.

## Validation levels

Zutai decoding validates structural shape. The Slime builder must additionally enforce semantic invariants that cannot be expressed by the data shape alone:

- `formatVersion` is exactly `1`;
- generation and object sizes are non-negative;
- object and component names are unique;
- every object reference resolves to exactly one object;
- `kernelObject` names an object of kind `#kernel`;
- `bootstrapComponent` names exactly one `#init` component;
- component dependencies exist and form an allowed graph;
- grant sources and targets exist;
- state owners and required health components exist;
- hashes use a supported algorithm and match object bytes;
- a parent generation exists when `parent` is present.

These checks belong to the builder because successful structural decoding must never be confused with authorization or integrity verification.

## Compatibility rules

- Readers reject unknown `formatVersion` values.
- Existing fields do not change meaning within format 1.
- New required fields require a new format version.
- Optional fields may be added only when old readers can safely ignore them; the boot decoder may enforce a stricter rule than Zutai's host-side record decoder.
- Manifest identity is computed from a canonical future binary encoding, not from source `.zti` bytes.

## Fixtures

- `fixtures/valid.zti` is the minimal vertical-slice generation: init, console, Dango, and sysinfo.
- `fixtures/invalid.zti` uses text where `formatVersion` requires an integer.
- `check-valid.zt` must evaluate to `#valid`.
- `check-invalid.zt` must evaluate to `#invalid` with a `formatVersion` decode path.

Run validation with the Zutai compiler pinned by the repository submodule:

```sh
ZUTAI_STDLIB_ROOT="$PWD/deps/zutai/stdlib" \
  cargo run --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli -- \
  run contracts/generation/v1/check-valid.zt
ZUTAI_STDLIB_ROOT="$PWD/deps/zutai/stdlib" \
  cargo run --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli -- \
  run contracts/generation/v1/check-invalid.zt
```
