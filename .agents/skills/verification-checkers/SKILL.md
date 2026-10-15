---
name: verification-checkers
description: Discipline for Slime OS verification code — when a new `scripts/check/check-*.py` is justified, where seL4 plane mechanism lives, how `just` targets map to checkers, and what an independent judge (host reference, capture reader, receipt comparator) and a pinned external artifact must guarantee. Use when adding or changing anything under `scripts/check/`, `scripts/lib/sel4_*`, an exam judge, a host reference decoder, a `just` recipe, or a negative-control set.
---

# Verification checkers

`AGENTS.md` keeps the two-sentence summary. This skill is the full rule set.

## When a new checker is justified

Do not create a new `scripts/check/check-*.py` merely because a change adds
an invariant or roadmap gate. Identify the verification mechanism that owns
the invariant, extend its checker when one exists, and add a case or module
when execution and evidence collection are shared.

Add a top-level checker only for a genuinely new mechanism or independently
reusable boundary:

- a different execution environment;
- a distinct input/output protocol;
- a stable subsystem boundary with multiple checks;
- independent reuse;
- materially different semantics.

Roadmap items, backlog IDs, compositions, individual regressions, and
individual QEMU planes do not by themselves justify top-level checker
executables. Public `just` targets remain narrow and descriptive; several
targets may invoke one checker with different cases (`--arm`, `--slice`,
`--controls-only`).

## seL4 planes

The shared mechanism lives in `scripts/lib/sel4_plane.py` (QEMU invocation,
transcript collection, common failures, control and mutation machinery) and
`scripts/lib/sel4_gate_markers.py` (marker chains and ordering). Each
`scripts/check/check-sel4-*-plane.py` owns only its concrete expectations for
boot, lifecycle, fabric, IO, storage, and similar domains.

Move repeated QEMU or transcript handling into those libraries rather than
copying it into a new plane script. Extract only observed repetition; do not
build a generic class-heavy test framework, and keep existing public gates.

## Independent judges

An independent judge — a host reference decoder, a capture reader, a receipt
comparator, a composition judge — must be at least as strict as the code it
judges on every boundary it claims:

- An input the Rust side refuses (non-canonical LEB128, an overflowing
  length, an unknown extension), the judge also refuses. A judge looser than
  the decoder accepts malformed wire evidence as proof.
- A frame, record, or ordering the exam says is excluded is checked by
  content, not by length, by protocol number alone, or by its first element.
  "No other address" means every ARP sender and target is parsed; "in the
  order sent" means every sample's position is checked, not sample 0's.
- A counted observation is counted from transcript or output evidence, not
  assigned from a literal or a configuration value the fixture already
  knows.
- Every positive assertion has a negative control that makes it fail: a
  missing marker, a reordered chain, an explicit failure marker, a wrong
  count, absent bytes, a wrong holder. An assertion with no control is
  unproven.
- A gate that runs a subset of an exam (a `--slice`, an `--arm`) still runs
  the pinned corpus judge and the controls that protect what the subset
  relies on; an implementation must not be able to alter a pinned refusal
  on a path the narrower gate never looks at.
- A record a judge consumes from outside (a listener's receipt, a peer's
  capture) is read only through its Zutai contract's decoder, and its
  presence is recorded from the event that produced it, not from a poll
  that may miss a short-lived peer.

## Pinned external artifacts

A pinned external artifact (wheel, crate, image, toolchain) is verified by
hash on every run: `pip install --require-hashes --no-deps`, a checked
`sha256` on the file, a pinned revision in the lockfile. A cached environment
(`build/<exam>/venv`, a scratch directory) is rebuilt, never trusted from a
version string, a metadata file, or the directory's existence; the cost of a
rebuild is seconds, the cost of a forged cache is a false pass.

## Closures and recipes

Adding or editing any recipe under `just/` or `Justfile` changes every
closure's `just-recipes` identity: regenerate the closures and refresh the
test-run records in the same PR, in a derived commit. `scripts/check/` and
`scripts/lib/` are not closure inputs except the files `RELEASE_INPUTS` in
`scripts/generate/generate-system-image-closures.py` names by path (today
`scripts/lib/elf_delivery.py` and the builders under `scripts/build/`); a
change confined to other checker or library files leaves the closures
current. Confirm either way with
`generate-system-image-closures.py --check`.

A checker, recipe, pin, or fixture under `.devloop/policy.json` `codePaths`
stales the evidence recorded against it. A change to one is a planning
change that lands with its item, and the item's acceptances bind only
closures the change does not edit (see `work-item-lifecycle`).
