# B6 — scoping the retained-v2 rollback claim to what is provable

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Kind | Defect |
| Status | Verified |
| Scope | `boot-contracts/src/generation.rs` v2 admission tests; C7.1 status and exit condition wording |
| Roadmap | B6, C7.1, C7.7 |
| Gates | `just contracts_check`, `just generation_check` |
| Trigger | Backlog B6, opened by the 2026-07-26 C7 audit (`devlog/2026-07-26-c7-audit/`) |
| Baseline | C7.1 claimed a retained v2 known-good artifact "still decodes **and boots**"; only decode was proven |

## Summary

C7.1's exit condition promised a retained v2 generation "decodes and boots", but
nothing ever booted one. Investigating the boot path showed the boot arm is not
merely unproven, it is **unconstructible from this tree**: `build-generation.py`
has only ever emitted v3, and `stage0::verify_kernel` resolves
`generation.kernel_object`, so each generation embeds and boots *its own* kernel.
A v2 rollback therefore runs its v2-era kernel, not the current one — which is
also why this tree's v3-only rights (bits 24–25) cannot break the rollback
window. The provable and load-bearing part is the stage-0 **admission chain**: a
retained v2 artifact must still pass its identity seal, expose its kernel and
bootstrap objects, and keep its signed release authorized. That chain had zero
coverage. This adds two `boot-contracts` tests for it and rewords C7.1 to claim
admission rather than a completed boot, recording why the boot arm cannot be
staged.

## Observable symptom

Not a defect — an overstated claim.

- Command: `grep -rl "FORMAT_VERSION_V2|MAGIC_V2" --include=*.rs --include=*.py .`
- Expected, if the claim held: some v2 artifact reachable by a boot path.
- Observed: only `boot-contracts/src/generation.rs` (a private test builder) and
  `kernel/tests/sample_plane.rs` (`build_v2_known_good`, an in-memory decode
  probe). `scripts/lib/boot_contracts.py:7-8` pins `GENERATION_MAGIC =
  b"SLIMEG3\0"` / version 3, so no committed or buildable v2 generation exists.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | v2 rights are 24-bit (`RIGHT_ALL_V2 = (1 << 24) - 1`); `bufferCreate` is bit 24 | A v2 generation **cannot encode** the factory grants B4 made mandatory — `1 << 24 > (1 << 24) - 1` |
| 2 | `require_grant` is unconditional, so a current-tree kernel booting a v2 manifest would panic on the missing grant | Looked like B4/B5 had silently broken the rollback window — worth confirming before writing anything |
| 3 | `stage0/src/lib.rs:320-325` (`verify_kernel`) resolves `generation.kernel_object` and decodes that object's bytes | Each generation is self-contained: rollback boots the kernel that shipped *inside* the retained generation |
| 4 | So a v2 rollback never runs this tree's kernel, never reaches this tree's `require_grant`, and never needs bit-24 rights | The step-2 worry is unfounded; B4/B5 did not break rollback. This is also exactly why the boot arm cannot be staged from here |
| 5 | Building a synthetic v2 generation would need a v2 grant packer, a v2 magic/version emitter, **and** a v2-era kernel image — but any kernel built from this tree expects the v3 grant set | A "v2 boot" assembled today would be a fabrication, not the artifact the rollback window actually retains |
| 6 | `release.rs:163` binds a release to `authority_manifest_identity`, whose rights width is version-branched (`generation.rs:386-392`) | Found the real rollback-window risk: if that branch were ever lost, every retained v2 release would fail authorization while the window still *looked* open |
| 7 | `stage0` is `no_std` with UEFI deps and no test harness; `boot-contracts` already has `alloc`, the v2 builder, and 19 tests | Put the coverage in `boot-contracts`, testing the same functions stage 0 calls |

## Root cause

Not a code defect — a claim written wider than the evidence, and wider than the
system can produce. The C7.1 exit condition was drafted when "retain v2 decoding
for the rollback window" was the design intent; the wording "and boots" implied a
scenario that requires a historical artifact this repository has never emitted.
Nothing in the code is wrong: dual-version decode, the version-branched authority
hash, and the self-contained kernel-per-generation design all work as intended.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/generation.rs` | `retained_v2_generation_passes_stage0_admission` — verifies the identity seal, locates the kernel object and bootstrap component, and confirms a tampered byte breaks the seal | The admission chain a rollback actually traverses is covered |
| `boot-contracts/src/generation.rs` | `retained_v2_authority_manifest_is_width_stable` — pins the 32-bit v2 authority hash as distinct from v3's and stable across decodes | A retained v2 release stays authorized; the C7.1 widening cannot silently close the rollback window |
| `roadmap/02-core-runtime.md` | C7.1 status and exit condition claim decode + release authorization + stage-0 admission, and record why the boot arm cannot be staged; C7.7's retained-v2 note corrected from "gap" to "correct scope" | Roadmap claims match producible evidence |

No production code changed — additions are tests plus documentation.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The v2 authority-hash branch is dropped, silently invalidating every retained v2 release | `retained_v2_authority_manifest_is_width_stable` (via `just contracts_check`) | Assertion fails. **Verified by injection**: removing the `if self.version == FORMAT_VERSION_V2` branch so v2 hashes at 64-bit made the test fail at `generation.rs:860`; restored afterwards |
| A v2 artifact stops being admissible at stage 0 | `retained_v2_generation_passes_stage0_admission` | Identity, kernel-object, or bootstrap assertion fails |
| The claim drifts wider than the evidence again | C7.1's exit condition now names the scope and the reason inline | Review catches a re-widened claim against a stated constraint |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `cargo test -p boot-contracts --lib` | 21 passed (19 prior + 2 new) | Direct |
| Fault injection: v2 authority hash widened to 64-bit | `retained_v2_authority_manifest_is_width_stable` FAILED as intended; reverted, 21 pass again | Direct |
| `just contracts_check` | pass | Direct |
| `just generation_check` | pass, two byte-identical builds | Direct |
| `just transfer_check` | pass (install, pending boot, promotion, rollback retention) | Direct |

## Decisions

- Decision: scope C7.1's claim to admission rather than fabricating a v2 boot.
- Rationale: the boot arm needs an artifact this tree cannot produce. Any "v2
  generation" built today would pair a v2 manifest with a v3-era kernel — a
  configuration that has never existed and never will, since each generation
  embeds its own kernel. Passing that off as rollback evidence would be worse
  than the overstated claim it replaced.
- Rejected alternative: add a v2 emitter to `build-generation.py` — it would
  produce a synthetic artifact proving only that the emitter and decoder agree,
  while implying the rollback window had been exercised.

- Decision: cover the admission chain and the authority-hash width instead.
- Rationale: these are the properties a real rollback depends on, they are
  reachable from this tree, and the authority-hash branch is a genuine hazard —
  losing it would close the rollback window while leaving every gate green.
- Rejected alternative: record B6 as a pure documentation fix — it would leave
  the one real hazard uncovered.

- Decision: put the tests in `boot-contracts` rather than `stage0`.
- Rationale: they exercise the same functions stage 0 calls, and `boot-contracts`
  already has `alloc`, the v2 builder, and a test harness. `stage0` is `no_std`
  with UEFI dependencies and no harness.
- Rejected alternative: add a harness to `stage0` — infrastructure for tests that
  would call straight through to `boot-contracts` anyway.

## Open risks and follow-ups

- [ ] If a real v2 generation is ever recovered from history (a released image,
  an archived bootstore), booting it under QEMU would upgrade this from
  admission to a true rollback boot. Worth doing opportunistically; not
  constructible on demand.
- [ ] The rollback window is still not time- or generation-limited in code;
  v2 retention is unconditional decode support. Noted since C7.1 and unchanged.
- [ ] B7 (`map` vs `bufferMap`) and B8 (budget aggregate) remain open.

## Artifacts and provenance

- Focused report: this entry.
- Raw evidence for the original finding: `devlog/2026-07-26-c7-audit/transcript.txt` §7.
- Related roadmap items: `roadmap/00-backlog.md` B6 (resolved by this entry);
  `roadmap/02-core-runtime.md` C7.1, C7.7.
- Related prior entries: `devlog/2026-07-26-c7-audit/` (opened B6);
  `devlog/2026-07-24-c7-1-generation-v3-u64-rights/` (introduced the claim and
  the version-branched authority hash this pins).
