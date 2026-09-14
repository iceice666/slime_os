# Two source gates refused the tree after #61: the run-record generator and the closure aggregate

| Field | Value |
|---|---|
| Date | 2026-09-14 |
| Kind | Defect |
| Status | Fixed |
| Scope | `scripts/generate/generate-system-test-runs.py` (`NOT_A_PLANE`), `scripts/check/check-system-image-aggregate.py` (`booted_images`, `IMAGES_WITHOUT_CLOSURE`, `LEGACY_PENDING_DELETION`, `DUAL_PATH`); the gates `just system_test_run_check`, `just system_test_run_bless`, `just system_image_closure_aggregate_check`, and `just contracts_check` through them |
| Work items | 01a0a06d-61e3-7b94-b8b9-d6228201b139 |
| Gates | `just system_test_run_check`, `just system_image_closure_aggregate_check`, `just contracts_check` |
| Trigger | Upstream `0361bff4` (#61): a new checker that boots nothing, a shared `scripts/lib/sel4_boot.py` that composes image names the aggregate gate scanned for in `scripts/check/` alone, a pc99 build knob, and four gates given a legacy pc99 arm |
| Baseline | Before #61, every `scripts/check/check-sel4-*.py` either booted a `build/slime-*.elf` it named in its own source or was listed in `NOT_A_PLANE`, and every image name, knob, and dual-path gate the aggregate gate saw was classified |

## Summary

Two source-and-contract gates that run in seconds and guard the mapping between checkers, images, closures, and build knobs were red on the baseline after #61, before any IO8 change. `generate-system-test-runs.py` derives one run record per `check-sel4-*.py` and refuses one that names no image unless `NOT_A_PLANE` says it owns no plane; #61's `check-sel4-x86-64-image.py` admits pc99 artifacts on the host and boots nothing, and was not listed, so the generator stopped at it and `system_test_run_check`, `system_test_run_bless`, and `contracts_check` failed before reading a record. `check-system-image-aggregate.py` learns which images a checker boots by scanning `scripts/check/` for literal `build/slime-*.elf` names and for the in-file `f"slime-…{suffix}.elf"` idiom; #61 moved that composition into `scripts/lib/sel4_boot.py` (`artifact_paths(stem, platform)`), so the product root-boot RV64 image became invisible and its own exemption "stale", and behind that the gate would have refused the pc99 knob `SLIME_PC99_COM1_PORT` and the four gates #61 gave a legacy pc99 arm. Both gates now see the tree #61 left: the checker is listed, the scanner expands `artifact_paths("slime-…", …)` stems with the platforms a checker spells out, the pc99 images and knob are classified, and the four dual-path gates are named with their reason.

## Observable symptom

- Command: `python3 scripts/generate/generate-system-test-runs.py --check` and `python3 scripts/check/check-system-image-aggregate.py` (as `just system_test_run_check` and `just system_image_closure_aggregate_check` run them), on the tree at `0361bff4`.
- Expected: exit 0 from each.
- Observed: `system test run generation: check-sel4-x86-64-image.py: boots no build/slime-*.elf image`; `system image aggregate check: exemption(s) naming an image no checker boots: ['slime-sel4-qemu-riscv-virt.elf']`.
- Exit/fault/serial evidence: the first from `just system_test_run_bless` on the IO8 branch, which is how it was found, and from the baseline's own generator (`git show upstream/main:scripts/generate/generate-system-test-runs.py`, run in place with `--check`) against the same checker set; the second from `just system_image_closure_aggregate_check` on the IO8 branch, whose only changes to `scripts/check/` are in files that name no RV64 image.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `just system_test_run_bless` on the IO8 branch stopped at `check-sel4-x86-64-image.py` before blessing anything | Not an IO8 record problem: the generator never reached the records |
| 2 | `booted_image` extracts `build/slime-*.elf` from the checker's source; `check-sel4-x86-64-image.py` reads `build/slime-sel4-qemu-pc99.identity.json`, `build/sel4-pc99-prefix/bin/kernel.elf`, and `build/sel4-artifacts/…/slime-root.elf`, none of which match | The checker is an admission check over built artifacts, not a plane |
| 3 | The baseline's generator, run unchanged against the same `scripts/check/`, fails with the same line | The defect is #61's, and every tree since carries it |
| 4 | `just system_image_closure_aggregate_check` refused the exemption for `slime-sel4-qemu-riscv-virt.elf` as naming an image no checker boots; `just sel4_riscv64_root_boot_check` still boots it through `check-sel4-root-boot.py --platform qemu-riscv-virt` | The image is booted; the scanner lost sight of it |
| 5 | `check-sel4-root-boot.py` now calls `platform_artifact_paths("slime-sel4", platform)`; the `f"{stem}{suffix}.elf"` composition lives in `scripts/lib/sel4_boot.py`, outside `CHECK_ROOT`, and the scanner's suffix idiom requires the platform vocabulary in the same file | Neither of the scanner's two patterns can see an `artifact_paths` caller |
| 6 | With the stems expanded, the same gate refused, in turn, `slime-sel4-sample-qemu-pc99.elf` (no closure, no reason), the knob `SLIME_PC99_COM1_PORT` (no class), and `check-sel4-boot-layout.py` (closure and legacy builder both) | Every pc99 arm #61 added was unclassified in this gate |
| 7 | `just/product.just` boots pc99 arms of root-boot, wait-set, sample, component-graph, and boot-layout; `build-sel4.py` sets `SLIME_PC99_COM1_PORT` only for the legacy interactive graph on pc99, beside `SLIME_PRODUCT_UART_PADDR` | The exemptions, the knob's class, and the dual-path reasons follow from the Justfile and the builder |

## Root cause

`NOT_A_PLANE`, `IMAGES_WITHOUT_CLOSURE`, `LEGACY_PENDING_DELETION`, and `DUAL_PATH` are the only ways those two gates know a checker, image, knob, or gate is outside their domain, and #61 added one of each kind without adding it there. Separately, `booted_images` assumed a checker composes its own image name; once `sel4_boot.artifact_paths` composed it for every multi-platform checker, the scanner's view of what the repository boots was wrong, not merely incomplete: an image really booted counted as unbooted, which is the inverse of the drift the gate exists to catch. The invariants the gates enforce are the right ones; the tree stopped declaring itself to them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/generate/generate-system-test-runs.py` | `check-sel4-x86-64-image.py` joins `NOT_A_PLANE` as an artifact-admission check that boots nothing | Every `check-sel4-*.py` is either a recorded plane or declared not one |
| `check-system-image-aggregate.py` `booted_images` | A checker calling `artifact_paths("slime-<stem>", …)` contributes `slime-<stem>.elf` and `slime-<stem>-<platform>.elf` for every non-default platform of `sel4_boot.PLATFORMS` it spells out; the in-file suffix idiom keeps its rule and now names `DEFAULT_PLATFORM` rather than a literal | An image a recipe really boots is visible to the gate whether its name is spelled in the checker or composed in the library |
| `IMAGES_WITHOUT_CLOSURE` | `slime-sel4-qemu-pc99.elf`, `slime-sel4-graph-qemu-pc99.elf`, `slime-sel4-sample-qemu-pc99.elf`, `slime-sel4-wait-set-qemu-pc99.elf`: the pc99 arms build legacy because Multiboot2 media has no loader image | Every booted image maps to a closure or carries a reason |
| `LEGACY_PENDING_DELETION` | `SLIME_PC99_COM1_PORT`, set only by the legacy interactive-graph path on pc99, the twin of `SLIME_PRODUCT_UART_PADDR` | Every `SLIME_*` knob is closure-declared, non-keying, or named legacy |
| `DUAL_PATH` | `check-sel4-boot-layout.py`, `check-sel4-component-graph.py`, `check-sel4-sample-plane.py`, `check-sel4-wait-set-plane.py`: their pc99 arm has no closure | A gate holding both build paths is enumerated with its reason, never silent |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future checker that boots nothing lands unlisted | `just system_test_run_check` | `<checker>: boots no build/slime-*.elf image` |
| A checker moves its image composition somewhere the scanner cannot see | `just system_image_closure_aggregate_check` | `exemption(s) naming an image no checker boots`, or a closure reported unexercised |
| A pc99 arm gains a closure while its exemption or dual-path entry stays | the same gate | `listed as having no closure, but closure … exists` / `gate(s) listed as dual-path that no longer hold both` |
| The gate's own controls | the same gate's built-in mutations (an invented knob, an unlisted dual-path gate) | the mutation is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just system_test_run_bless` after the change | 53 records frozen: 47 changed only in `imageClosureIdentity` for the IO8 tree on the same branch, one new record for `sel4-pwm`, and none for the x86-64 checker | Direct |
| `just system_test_run_check`, `just contracts_check` | Pass on the IO8 branch that carries this fix | Direct |
| `python3 scripts/check/check-system-image-aggregate.py` | `23 booted plane image(s) — 10 reachable from exactly one canonical closure, 13 exempt with a declared reason; all 55 closures exercised by an owning build or boot gate; 7 plane build flag(s) each reachable from a just target`, and its self-mutation controls pass | Direct |
| `just ruff` | Pass | Direct |

## Decisions

- Decision: list the checker rather than teach the generator to skip any checker naming no image.
- Rationale: the refusal is the guard; a silent skip would let a real plane checker with a typo'd image path drop out of the records unnoticed.
- Decision: expand `artifact_paths` stems by the platforms a checker spells out, not by every platform the library knows.
- Rationale: `check-sel4-component-graph.py` admits every platform in its `--platform` choices but builds only pc99 through the legacy image; the literal it spells is the arm a Justfile target really runs, and an image nobody boots must not be counted as booted, which is the same error in the other direction.
- Decision: the pc99 knob is legacy pending deletion, not non-keying.
- Rationale: it changes the root's bytes (which port the interactive root reads), and only the legacy builder sets it; the closure builder scrubs it.

## Open risks and follow-ups

- [ ] The pc99 arms and their knob leave the exemption and legacy sets larger than before #61; each shrinks when closure packaging supports Multiboot2 media, and the gate refuses a stale entry when that happens.

## Artifacts and provenance

- Focused report: this entry.
- Related work item: `01a0a06d-61e3-7b94-b8b9-d6228201b139`.
