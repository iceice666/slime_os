# System-image closure source format 3

A closure source names every input to one bootable-image build by path. `schema.zt` is the normative
layout. A source has the fields of a version 2 `SystemImageClosure` except the identities: no
artifact `identity`, no `systemIdentity`, and no component-spec `identity`. The source records no
digest, so an edit to an input tree changes no committed source.

`scripts/generate/generate-system-image-closures.py` writes one source per closure into `closures/`.
It rewrites a source only when that closure's input set changes. `--check` refuses a missing, stale or
orphaned source.

## Resolution

The resolver reads a source in one checkout and writes a version 2 `SystemImageClosure`:

| Source field | Resolved field |
| --- | --- |
| each `Locator` of kind `file` | SHA-256 of the file's bytes |
| each `Locator` of kind `tree` | `slime-component-sdk-tree-v1` digest of the files git lists, tracked and untracked but not ignored, through every submodule; outside git, of a walk of the tree |
| `systemSpec` | the locator's digest plus the compiled system's `systemIdentity` |
| each implementation | the locator's digest plus the compiled component spec's `identity` |

The resolved closure's identity is the build's derivation id. A resolution is dirty if an input holds
a modified tracked file or an untracked or ignored file, or a submodule is checked out away from its
pin. The resolved closure does not record the dirty flag; the build result does.

Builds write resolved closures to `build/closure-resolved/`, which no source names.
`--resolve NAME --output PATH --report REPORT` writes one to PATH and a
[`closure-resolution/v1`](../../closure-resolution/v1/README.md) report to REPORT.

## Compatibility

A consumer without git, such as an exported component SDK, receives the resolved version 2 closure,
not the source. Negative build cases stay in `../v2/negative/` and name their base closure by source
name. A new field, locator kind or identity rule is a new format version.
