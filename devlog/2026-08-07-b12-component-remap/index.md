# B12 — the component build's `--remap-path-prefix` named a path that does not exist

| Field | Value |
|---|---|
| Date | 2026-08-07 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/.cargo/config.toml`, `scripts/build/build-generation.py` |
| Roadmap | B12 |
| Gates | `just generation_check`, `just contracts_check`, `just fmt_check_all`, `just ruff`, `just typos` |
| Trigger | Last open backlog item not blocked on a capability-model decision, after B30 was moved to the resolved log |
| Baseline | `components/.cargo/config.toml:11` and `:21` passing `--remap-path-prefix /home/iceice666/projects/slime_os=.` against a checkout at `/Users/iceice666/code/slime_os` |

## Summary

The component build hardcoded one developer's checkout path in a determinism
flag. It is now computed from the repository root at build time, mirroring what
the seL4 target already did. The defect was real; its **recorded severity was
not**, and measuring that is most of what this entry contributes.

## Observable symptom

`components/.cargo/config.toml` passed
`--remap-path-prefix /home/iceice666/projects/slime_os=.` for both
`x86_64-unknown-none` and `aarch64-unknown-none`, while the checkout is
`/Users/iceice666/code/slime_os`.

## Investigation log

1. Confirmed the literal is not a prefix of the real path, so the flag is an
   outright no-op here. The entry described *mangling* — the stale literal being a
   prefix and leaving a remainder behind — which was true of an older checkout
   layout and is no longer.
2. Found the correct pattern already present for JSON targets:
   `build-generation.py` sets `RUSTFLAGS` including
   `--remap-path-prefix={ROOT}=.`.
3. Established the blast radius the deferral feared, by measurement rather than
   argument. Captured the generation identities from `just generation_check`
   before touching anything: `df40ce7a…13e5` and `ebdf06d0…b092`.
4. Checked whether the flag has anything to act on:
   `strings <component> | grep -c '/Users/iceice666'` is **0** for every x86
   component ELF. These are release builds with no debug info, so no absolute
   source path is recorded to begin with.
5. Applied the fix and re-ran. Identities **byte-identical** to the baseline.

## Root cause

A path literal where a computed value belonged. The seL4 target avoided it only
because it was added later and had to pass its flags explicitly.

## Changes

- `components/.cargo/config.toml`: the `--remap-path-prefix` entries are gone from
  both triples, with a comment stating where the flag now comes from and why.
- `build_rust_components`: for triple targets, appends
  `--remap-path-prefix={ROOT}=.` through **`--config`**.

`--config` and not `RUSTFLAGS` is the whole difficulty. `RUSTFLAGS` *replaces*
config rustflags rather than adding to them, so setting it here would have
silently dropped `relocation-model=static`, `code-model=small`, and three link
args the x86 link depends on. The JSON-target branch can set `RUSTFLAGS` freely
only because a JSON target inherits none of those to begin with — which the
existing comment there already says, and which is exactly why copying that branch's
approach would have been wrong.

## Regression guards

`just generation_check` builds x86 components twice and compares identities; it is
the gate that would catch a remap that changes output. No new gate: the property
is reproducibility, which that gate already asserts.

## Verification

| Check | Result |
|---|---|
| generation identities, before vs after | **byte-identical** (`df40ce7a…13e5`, `ebdf06d0…b092`) |
| `just generation_check` | passes, twice consecutively |
| `just contracts_check` | passes |
| seL4 channel / stream / component-graph gates | pass |
| `just fmt_check_all`, `just ruff`, `just typos` | pass |

The first row is the load-bearing one: it is the deferral's central fear,
measured and found empty.

## Decisions

**Fix it despite the severity being overstated.** A flag that names a nonexistent
path is wrong regardless of whether it currently matters, and leaving it invites
the next reader to trust a determinism guarantee that is not being enforced.

**State the overstatement rather than quietly fixing it.** The deferral was
re-reviewed five times across five slices on the reasoning that the blast radius
was large. It was not, and the reason it was not — release builds record no source
paths — is worth writing down so the same reasoning is not repeated.

**`--config`, not `RUSTFLAGS`.** See Changes. This is the one place where the
obvious approach silently breaks the link.

## Open risks and follow-ups

**The exit condition is partially met.** Two builds from two different checkout
directories were *not* run; that needs a second clone this environment cannot
usefully provide. What was established instead is narrower but sufficient for the
current artifacts: the flag is computed rather than hardcoded, and the ELFs it
guards contain no paths for it to affect.

If components are ever built with debug info, the flag becomes load-bearing and
the two-checkout comparison becomes worth running for real. That is the condition
under which this entry should be reopened.

`aarch64-unknown-none` was fixed symmetrically but is not exercised by any gate
in this environment, so its remap is `[INFERENCE]` correct by symmetry with the
x86 path rather than observed.

## Artifacts and provenance

Identities and gate results are from `just generation_check` and the named plane
gates on `aarch64-apple-darwin` at this entry's date. The `strings` counts were
taken against
`target/components/x86_64-qemu-virtio/generation-1/x86_64-unknown-none/release/`.
