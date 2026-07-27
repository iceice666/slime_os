# D1–D7 — Native development, live update, and on-device build roadmap

| Field | Value |
|---|---|
| Date | 2026-07-27 |
| Kind | Decision |
| Status | Proposed |
| Scope | `roadmap/08-native-development.md`, roadmap dependency/release index, P0 producer contract, language ownership, promoted build directions |
| Roadmap | D1, D2, D3, D4, D5, D6, D7, P0 |
| Gates | none |
| Trigger | Request to plan in-system program authoring/execution and admit a new language that emits Slime's native component format |
| Baseline | M6 could execute only generation-declared images built on the host; runtime partial replacement and on-device native builds had no canonical roadmap milestone |

## Summary

A new D1–D7 canonical track now separates in-system authoring, direct language image emission, hermetic building, ephemeral executable admission, transactional live component cutover, authorized on-device generation activation, and complete-generation reproduction. The decision preserves one target-qualified `SLIMECMP` loader contract and Zutai as the only cross-boundary schema language. Status is Proposed: no editor, compiler service, executable factory, live switch, or on-device builder has been implemented or runtime-verified.

## Changes

| Area | Change | Established boundary |
|---|---|---|
| D1 | Source workspace, minimal editor, stored Dango script loop | Writing source is directory-capability activity, not ambient filesystem access |
| D2 | Producer-neutral P0 contract and direct backend for the new language | Direct output uses the same `SLIMECMP` validator as ELF-derived images and carries no authority |
| D3 | Hermetic build protocol, deterministic authority seal, resource accounts, and detached provenance | Source/toolchain/target closure determines outputs; provenance remains separate from release authorization |
| D4 | `ExecutableFactory` admission and bounded ephemeral run | Valid bytes remain inert until explicitly admitted; unsigned output cannot alter BootState or a persistent generation |
| D5 | Narrow signed-generation live cutover | Only unchanged kernel/bootstrap/graph/grants/interfaces/state/resources may switch side-by-side; all other diffs require reboot |
| D6 | On-device generation build, diff, authorization import, switch/boot selection | Local build does not bypass M5.8; live and next-boot activation use their owning checked paths |
| D7 | Clean mixed-language full-generation reproduction | Build location is irrelevant; normalized source/toolchain closure and resulting identities are authoritative |
| P0/index | Added direct-emitter conformance, dependency edges, status lane, and development release gates | Target/artifact identity is producer-neutral and release claims remain compositional |

## Decisions

- Decision: keep the new language language-project-owned and Slime-contract-conforming rather than embedding its parser, type system, or policy in the kernel.
- Rationale: Slime needs a stable producer-neutral executable and syscall contract; language evolution must not create a second loader or kernel ABI.
- Decision: keep Zutai as the only schema/configuration language. The new language consumes generated bindings and may use native layouts only for in-memory data.
- Rationale: a second serialized-format source of truth would violate the repository schema boundary and make compiler output disagree with kernel/component protocol bindings.
- Decision: separate build, ephemeral test, release authorization, live switch, next-boot selection, and known-good promotion.
- Rationale: local iteration needs a no-reboot path, but successful compilation cannot imply executable authority or authorization to persist/boot code.
- Decision: introduce a narrow first live-update class: exact target/kernel/bootstrap/graph/interface/grant/state/resource equality with selected executable identities changed.
- Rationale: this proves useful partial replacement while classifying state migration, authority changes, exclusive-device handoff, and ABI changes as reboot-required instead of hiding unsafe best effort.
- Decision: a successful live switch leaves the new generation pending until an ordinary reboot exercises stage-0/bootstrap health and promotes it.
- Rationale: live service readiness does not prove that the complete boot path is known-good.
- Decision: let the new language emit `SLIMECMP` directly; retain ELF only as the current Rust/toolchain intermediate.
- Rationale: the image schema is already structural. Requiring every language to produce ELF adds a language-independent intermediate without strengthening validation.
- Decision: allow D7 to use X1 for the current pinned Rust toolchain unless a native Rust route independently passes the same hermetic contract.
- Rationale: complete-system reproduction must account for the repository's actual Rust sources rather than claiming self-hosting from one new-language compiler.
- Rejected alternative: make writable source or a valid image automatically executable. This collapses data authority into code authority and defeats the existing generation-only injection boundary.
- Rejected alternative: let unsigned local generations enter a developer boot mode. Ephemeral D4 execution supplies iteration without weakening M5.8 or stage-0.
- Rejected alternative: promote a live-switched generation directly to known-good. That would claim unobserved stage-0 and bootstrap behavior.

## Open risks and follow-ups

- [ ] Name and pin the new language repository/compiler/toolchain identity before D2 implementation; the roadmap deliberately fixes the contract rather than inventing the language name.
- [ ] P0 must settle the target-qualified image revision before a direct emitter can close D2.
- [ ] D3 depends on C9 resource/time/lifecycle authority and must define Entropy authority plus deterministic capability-receipt sealing without creating a C9 ownership conflict.
- [ ] D4 must choose bounded immutable backing and lifetime accounting for runtime-admitted images without shrinking the component-image size contract or adding an avoidable unbounded kernel copy.
- [ ] D5 needs a checked cutover/crash model covering pending BootState, route commit/reversal, update-service loss, and power interruption before implementation can claim transactional activation.
- [ ] D5 initially excludes state-schema migration, grant/graph changes, and exclusive-device handoff; later expansion requires an explicit model and gate rather than widening the classifier silently.
- [ ] D6 imports detached release authorization; secure on-device signing-key custody remains A2/A4 follow-up, not an implicit local-builder privilege.
- [ ] D7's initial full Rust rebuild depends on X1 unless a separately qualified native Rust compiler/linker route lands.

## Artifacts and provenance

- Canonical roadmap track: `roadmap/08-native-development.md`
- Roadmap dependency and release map: `roadmap/README.md`
- Producer-neutral artifact prerequisite: `roadmap/07-architecture-portability.md` (P0)
- Promoted design sources: `docs/directions/03-nondeterminism-as-capabilities.md`, `docs/directions/23-build-provenance.md`, `docs/directions/30-deterministic-on-device-builds.md`
- In-session documentation smoke check: eight changed roadmap/direction files, 81 local links checked, zero missing files/anchors; D1–D7 headings/statuses and all seven planned gate names were present with the expected counts.
