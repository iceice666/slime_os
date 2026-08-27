# B81: the freestanding C linker emitted an unaligned writable segment

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Defect |
| Status | Fixed |
| Scope | `components/runtime/c/component-aarch64.ld`, `just sel4_rpi5_image_check`, SDK publication workflow run 33063988008 |
| Roadmap | B81, P4 |
| Gates | `just sel4_rpi5_image_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | The replacement two-profile SDK publication run advanced past the clean-shell Rust target fix and failed while wrapping the product Slisp ELF for the RPi5 generation |
| Baseline | The QEMU seL4 profile carries native ELF images and therefore admitted the same Slisp bytes by their own program headers; the RPi5 profile converts fixed-base ELF segments into the component-image v2 segment table and requires every load segment to begin on a page boundary |

## Summary

The freestanding C linker now places compiler-emitted GOT data in the explicit page-aligned `.data` output section. Clang 21 emitted an 8-byte `.got` orphan after `.data`; LLD assigned it its own writable `PT_LOAD` at `0x403180`. The QEMU native-ELF path accepted that layout, but the RPi5 segment-table converter correctly refused the non-page-aligned load address. Collecting `.got` and `.got.*` into `.data` produces one writable load at `0x404000`, followed by page-aligned `.bss`, so one deterministic Slisp ELF is admissible on both publication profiles.

## Observable symptom

- Command: `nix develop --command just sel4_rpi5_image_check` in GitHub Actions run 33063988008.
- Expected: build the RPi5 seL4 prefix, construct the board-qualified product generation, and package the RPi5 image.
- Observed: the prefix and all Rust components built; `build-generation.py` then refused `slisp: invalid or overlapping segment`.
- Exit/fault/serial evidence: `Build seL4 prefixes` exited 1 and skipped `Publish and verify`; no release deployment was created.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Run 33063988008 passed the QEMU arm and built the RPi5 kernel, root child, Rust components, and product Slisp ELF | The prior compiler and missing-target defects were fixed; the remaining failure was RPi5 component-image construction |
| 2 | The failed path uses `aarch64-rpi5`, whose ordinary component-image wrapper requires page-aligned, non-overlapping `PT_LOAD` virtual addresses | The generic refusal named a real format invariant, not an RPi kernel problem |
| 3 | `objdump -p build/slisp-product.elf` showed writable loads at `0x403180` and `0x404000`; `.got` was the 8 bytes at the unaligned address | LLD had emitted `.got` as an orphan because the C linker script named `.data` but not `.got` |
| 4 | Adding `.got` and `.got.*` to `.data` moved the writable file-backed load to `0x404000` and `.bss` to `0x405000` | Every converted segment now begins on a 4096-byte boundary without weakening admission |

## Root cause

`components/runtime/c/component-aarch64.ld` explicitly placed text, read-only data, ordinary data, and BSS, but omitted the compiler-generated global offset table. LLD therefore applied orphan-section placement and created a separate writable load immediately after `.rodata`, at an address not aligned to the target profile's 4096-byte page size. The native seL4 ELF loader can map program-header footprints directly, while component-image v2's segment-table representation requires each segment base to be page-aligned. The shared C output was consequently valid only for the QEMU publication arm.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/runtime/c/component-aarch64.ld` | Collect `.got` and `.got.*` into the page-aligned `.data` output section | Every freestanding C `PT_LOAD` emitted for segment-table profiles starts on a target page boundary |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Clang or LLD emits a non-page-aligned orphan load again | `just sel4_rpi5_image_check` and exact RPi `component_image` admission | `invalid or overlapping segment` before generation signing |
| The layout change breaks native QEMU C components | Hosted two-profile SDK build and existing C/Slisp component gates | QEMU image construction, component admission, or boot marker failure |
| Repository checks regress | `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` | Any named check fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| GitHub Actions run 33063988008 | Reproduced: QEMU succeeded; RPi5 reached Slisp generation wrapping and failed on the unaligned writable load | Direct |
| `nix develop --command python3 scripts/build/build-c-component.py components/slisp/slisp.c components/slisp/main.c build/slisp-product.elf` | Pass: rebuilt the freestanding product ELF with the repaired linker script | Direct |
| `objdump -p build/slisp-product.elf` | Pass: load addresses were `0x400000`, `0x403000`, `0x404000`, and `0x405000`; every address was page-aligned | Direct |
| Exact `component_image("slisp", ..., aarch64-rpi5)` admission | Pass: emitted a component-image v2 blob instead of the prior segment refusal | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` | Pass | Direct |
| `nix develop --command just sel4_rpi5_image_check` | Pass on Darwin: admitted the repaired Slisp ELF, built generation `3fbba9ca367377c3d799c26840e141d8cfaf1874d4823a913845a84d892a4e8c`, compiled the root and loader, and wrote the RPi5 ELF plus identity manifest | Direct |
| Replacement hosted two-profile SDK publication run | Pending after merge | Not observed |

## Decisions

- Decision: place GOT input sections explicitly rather than relaxing component-image segment admission.
- Rationale: the format and loader require page-granular mappings; the linker owns the layout and can satisfy that contract without copies or runtime relocation.
- Rejected alternative: round or merge unaligned segments in `build-generation.py`. That would rewrite ELF mapping semantics, risk combining incompatible permissions, and hide future linker drift.

## Open risks and follow-ups

- [ ] The two-profile SDK publication must be redispatched after this change reaches `main`.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [GitHub Actions run 33063988008](https://github.com/iceice666/slime_os/actions/runs/33063988008).
- Serial/debugger/model output: none; generation construction failed before packaging or boot.
- Related roadmap item: [P4](../../roadmap/07-architecture-portability.md#p4--raspberry-pi-5-board-bring-up).
- Predecessor: [`devlog/2026-08-27-b80-rpi5-rust-target/`](../2026-08-27-b80-rpi5-rust-target/index.md)
