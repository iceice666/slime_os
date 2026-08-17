# Component platform track: component/system specs as data, and out-of-tree components as the forcing proof

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/10-component-platform.md` (new), `roadmap/00-backlog.md` (B70 opened, B65 follow-up cross-referenced), `roadmap/09-rpi5-ros2-demo.md` (RP4 depends on CP5), `roadmap/README.md` (ledger row, track map, sequencing item 5) |
| Roadmap | CP0, CP1, CP2, CP3, CP4, CP5, B70, RP4, B65 |
| Gates | none |
| Trigger | Investigation of whether a component can be authored and built outside this repository, 2026-08-17 |
| Baseline | `contracts/generation/v1`'s hand-authored `Executable`/`Instance`/`Object` records were the only definition of "a component"; every component was a `[[bin]]` in one crate whose `build.rs` parsed those fixtures at compile time |

## Summary

Slime OS had no component-level specification independent of the generation
manifest, and no roadmap track intending one. An investigation grounded against
`components/bins/Cargo.toml`, `components/bins/build.rs`, and
`scripts/build/build-generation.py` established that this is not a
configuration gap but a structural one: a component's CSpace and notification
slot numbers are compiled into its own source through `build.rs`-private
`OUT_DIR` files derived by ad hoc string parsing of
`contracts/generation/v1/fixtures/*.zti`, and the host builder has exactly one
component-build command with no parameter accepting bytes produced elsewhere.
The root-side admission path needs no change at all. This entry records the
decision to write that gap down as backlog item **B70** and to plan its removal
as a new six-milestone track, `roadmap/10-component-platform.md` (CP0–CP5),
whose last milestone makes RP4's two Arm data-path components the forcing proof
that the specification layer actually works. Status is `Proposed`: no
engineering work was performed, no contract or code was written, and no gate
was added.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/10-component-platform.md` | New track: CP0 `component-spec/v1`, CP1 `system-spec/v1` + generation derivation, CP2 runtime-resolved binding, CP3 crate-per-component SDK boundary, CP4 external-artifact admission, CP5 out-of-tree proof. Each milestone carries deliverables, required checks, a planned verification target, and an exit condition | A serialized cross-boundary format is Zutai-declared and derived, not hand-authored twice (repository Zutai rule) |
| `roadmap/00-backlog.md` | Opened **B70** with the observed problem, evidence, proposed fix, and exit condition; the `## Open` section was `(none)` | Known structural debt is tracked ahead of new track milestones |
| `roadmap/00-backlog.md` | B65's deferred follow-up ("the 52-binary fixture population uncollapsed") now names CP3 as its planned resolution | A deferred follow-up points at the successor that closes it |
| `roadmap/09-rpi5-ros2-demo.md` | RP4 now depends on CP5, requires its two components authored and built out-of-tree, and inherits CP5's build-isolation and in-tree-fallback checks | RP4's exit condition cannot be claimed from in-tree components alone |
| `roadmap/README.md` | Component platform ledger row, six `CP*` track-map nodes with ten edges (`Backlog → CP0/CP2`, `CP0 → CP1/CP4`, `CP2 → CP3/CP4`, `CP3 → CP4/CP5`, `CP4 → CP5`, `CP5 → RP4`), and sequencing item 5 rewritten | The index reflects track status and dependency edges |

## Decisions

- Decision: CP5's "out-of-tree" means a genuinely separate git repository, not a
  new crate inside this workspace's Cargo members.
- Rationale: matches this repository's existing refusal to conflate weaker
  evidence with stronger (roadmap invariant 8; QEMU cannot close a physical
  board milestone). CP3's in-repo crate split is a mechanical precondition and is
  explicitly recorded as insufficient by itself.
- Rejected alternative: the cheaper proof — a crate inside the workspace but
  outside `components/bins`. It would pass while still depending on this
  workspace's `Cargo.toml`, target spec resolution, and toolchain environment,
  proving none of the things the milestone exists to prove.

- Decision: CP4 reuses the existing whole-generation release signing
  (`boot-contracts/src/release.rs::INITIAL_TRUST_ROOT`) as the sole trust
  boundary for a generation containing an externally supplied component; it adds
  no per-component signature, provenance record, or new trust root.
- Rationale: no per-component signature check exists for in-tree components
  today either — root-side admission is producer-agnostic — so requiring one for
  external components only would be unrequested new scope with no in-tree
  counterpart.
- Rejected alternative: per-component signing and provenance at CP4. Recorded as
  separate future work layered on top of CP4, needed only if mutually untrusted
  third-party suppliers become a requirement.

- Decision: this track is independent of D1–D7; CP4 and D4 are not implemented in
  terms of each other.
- Rationale: D4's `ExecutableFactory`/`EXECUTABLE_ADMIT` is a runtime,
  in-booted-system, ephemeral admission mechanism for un-persisted dev-loop
  execution. CP4 is host-side and build-time and produces an ordinary,
  persistent, normally signed generation. They answer different questions.
- Rejected alternative: folding CP4 into D4's authority, which would put a
  build-time packaging decision behind a runtime capability and make the demo
  path depend on a deferred track.

- Decision: CP1's exit condition requires deriving `valid.zti` and the *smallest*
  `sel4-*.zti` only; converting the remaining fixtures is deferred follow-on
  work.
- Rationale: 27 `sel4-*.zti` fixtures exist and B62 already reshaped their
  variance into declared deltas. Proving the generator on two real fixtures
  establishes the mechanism; converting all of them is bulk work whose failure
  mode is unrelated to the schema decision.
- Rejected alternative: requiring all fixtures, which would make the milestone's
  size dominated by transcription rather than by the derivation it is proving.

## Open risks and follow-ups

- [ ] CP0's `ComponentSpec` field set is transcribed from
      `spec/requirement-document-v0.6.md` §2.1 and not yet validated against a
      real corpus; whether every existing `valid.zti` component is describable
      by it is CP0's own first required check, unobserved here. **[INFERENCE]**
- [ ] CP2 assumes a badge-authenticated root-served binding query can replace
      every compile-time constant at the 19 confirmed `include!` sites. The
      `command_profile.rs` consumer resolves slots derived from a *different*
      instance's grants (`components/bins/build.rs:107–137`), so the query's
      identity model needs that case designed explicitly. **[INFERENCE]**
- [ ] CP5's exit condition needs an `aarch64-sel4-qemu-virt` boot, so it cannot
      be claimed from host checks. No gate exists yet for any CP milestone; all
      six "Planned verification target" names are unimplemented and were
      confirmed absent from `just --list`.
- [ ] B70's exit condition and the CP0–CP2 milestones overlap deliberately.
      Closing B70 requires CP0, CP1's derivation, and CP2; the track file records
      this with a `Closes:` line rather than duplicating the exit condition.

## Artifacts and provenance

- Focused report: none; the track file itself carries the analysis.
- Raw transcript: not retained.
- Serial/debugger/model output: none — no runtime work was performed.
- Related roadmap item: [`roadmap/10-component-platform.md`](../../roadmap/10-component-platform.md), [B70 in `roadmap/00-backlog.md`](../../roadmap/00-backlog.md), [RP4](../../roadmap/09-rpi5-ros2-demo.md)
- Evidence for the motivating gap, all read directly this session:
  `components/bins/Cargo.toml` (52 `[[bin]]` entries, `autobins = false`, the
  `store`-feature allocator comment at lines 23–28),
  `components/bins/build.rs:1–197` (three generator functions emitting four
  `OUT_DIR` files), the 19 `include!(concat!(env!("OUT_DIR"), …))` sites across
  17 files, `scripts/build/build-generation.py:2415–2458` (the single
  `cargo build … -p slime-components --bin <name> …` command) and `:2506`
  (`elf_component_image`), `contracts/component/v2/schema.zt`,
  `contracts/interface-schema/v1/schema.zt:8–10,57–58` (the SHA-256
  identity/domain convention CP0 reuses),
  `contracts/generation/v1/schema.zt:189,204` (`FabricParticipant`,
  `FabricRoute`), `boot-contracts/src/component_image.rs:145` (`admit`),
  `boot-contracts/src/release.rs` (`INITIAL_TRUST_ROOT`).
