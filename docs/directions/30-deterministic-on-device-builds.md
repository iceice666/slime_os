# 30. Deterministic on-device builds

| | |
| --- | --- |
| Status | parked |
| Route | determinism |
| Depends on | [entry 3](03-nondeterminism-as-capabilities.md); M6 spawn, generation inspection/staging, and object-store paths (complete); M5.8 release authorization (complete) still governs locally built generations |
| Enables | self-hosting: rebuilding the running system on-device, byte-identical to the host build |
| Now | Mostly paper: normalized build-step semantics and content-addressed inputs are legal design work; execution waits on entry 3 and an on-device builder not delivered by M6. |

## Motivation

M6 completed native generation inspection/staging and the spawn mechanisms
an on-device builder can consume, but build semantics remain open. Define a build step as a
manifest-declared deterministic component (consuming
[entry 3](03-nondeterminism-as-capabilities.md)) whose inputs and
outputs are content-addressed objects; the object store deduplicates
build products naturally, and rebuilding the running system on-device
reproduces the host build byte-identically. "Builds are pure functions"
becomes enforceable inside the OS, and stage-0 verification applies to
locally built generations unchanged — the system cannot distinguish a
generation by where it was built.

## What exists today

- M5.5 (complete) proves the target: the host build already produces
  deterministic, byte-identical generation artifacts validated by
  `just generation_check`. On-device builds must reproduce exactly this
  output.
- M5.4 (complete) provides the content-addressed object store that
  deduplicates build inputs and products.
- M5.8 (complete) defines release authorization; local builds do not
  bypass it.
- Missing: deterministic-component authority semantics (entry 3) and an
  on-device builder. M6's completed spawn machinery supplies its launch path.

## Design sketch

A build step is a component declared deterministic under entry 3 — no
clock, no entropy — whose channel inputs carry content-addressed build
inputs (source objects, toolchain objects) and whose outputs are sealed
objects. The build graph is manifest data: which steps, in which
dependency shape, with which grants. Because inputs and outputs are
content-addressed, caching is identity-based and the store deduplicates
across builds and across machines for free.

The byte-identical claim has teeth: an on-device build of the same
normalized source must produce the same generation object identity as
the host build, which is checkable by comparing hashes — no trust in
the on-device toolchain required, only in the determinism construction.
A build component holding no clock or entropy grants cannot embed a
timestamp; nondeterministic toolchains fail the declaration statically
or the comparison dynamically.

Toolchain inputs are the hard part: the compiler is itself a
content-addressed object, so "same source" must mean "same source plus
same toolchain plus same normalized parameters" — the same closure
[entry 23](23-build-provenance.md) records for host builds.

## Open questions

- What exactly is "normalized source" — the input closure (source
  revision, toolchain identity, parameters) and in what canonical form?
- Where do build components get their authority — a builder service
  holding derive-limited grants over the store, per M6 spawn
  prerequisites?
- Build-time resource bounding interacts with
  [entry 25](25-resource-accounts.md): are builds charged to an account?
- How are locally built generations staged without M5.8 authorization —
  rejected outright, or stageable-but-unbootable until signed?

## Exit-condition sketch

An on-device build and the host build of the same normalized source
produce byte-identical generation objects; a build component holding no
clock or entropy grants cannot embed a timestamp.

## Probe guidance

Paper today: write the build-step contract (input closure, output
sealing, determinism declaration) and verify the concept host-side by
showing two independent host builds of the current tree already produce
identical generation objects under `just generation_check` — which
isolates what on-device execution must additionally guarantee.
