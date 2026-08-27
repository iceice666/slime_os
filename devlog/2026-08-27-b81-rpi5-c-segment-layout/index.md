# B81: the freestanding C linker emitted an unaligned writable segment

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Defect |
| Status | Fixed |
| Scope | `components/runtime/c/component-aarch64.ld`, `just sel4_rpi5_image_check`, SDK publication workflow runs 33063988008 and 33065116457 |
| Roadmap | B81, P4 |
| Gates | `just sel4_rpi5_image_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | The replacement two-profile SDK publication run advanced past the clean-shell Rust target fix and failed while wrapping the product Slisp ELF for the RPi5 generation |
| Baseline | The QEMU seL4 profile carries native ELF images and therefore admitted the same Slisp bytes by their own program headers; the RPi5 profile converts fixed-base ELF segments into the component-image v2 segment table and requires every load segment to begin on a page boundary |

## Summary

The freestanding C linker now declares its three load segments explicitly: executable text, read-only data, and writable data plus BSS. Clang 21 emitted an 8-byte `.got` orphan after `.data`; LLD assigned it a writable `PT_LOAD` at `0x403180`, which the RPi5 component-image converter correctly refused. Collecting GOT input fixed Darwin, but hosted Linux LLD still synthesized the invalid load, proving output-section placement alone did not control program headers across hosts. `PHDRS` plus explicit section-to-header assignments now emits exactly three page-aligned loads on Darwin and defines the same required shape for Linux.

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
| 4 | Collecting `.got` and `.got.*` into `.data` moved the writable file-backed load to `0x404000` on Darwin, and the local full RPi5 gate passed | Output-section placement was sufficient for Darwin LLD but still needed hosted proof |
| 5 | Hosted run 33065116457 still failed at the same refusal after the `.got` change | Linux LLD's orphan/program-header policy differed; the first fix was host-dependent |
| 6 | Declaring `PHDRS` and assigning `.text`, `.rodata`, `.data`, and `.bss` explicitly emitted exactly three loads at `0x400000`, `0x403000`, and `0x404000` | The linker script now owns the cross-host program-header contract rather than relying on LLD inference |

## Root cause

`components/runtime/c/component-aarch64.ld` originally left both GOT placement and program-header construction to LLD. The omitted GOT input first created an unaligned orphan load. Adding it to `.data` repaired Darwin LLD, but hosted Linux LLD still synthesized an invalid writable load, exposing the deeper defect: section placement did not specify which `PT_LOAD` each output section belonged to. The native seL4 ELF loader tolerated the host-dependent layout, while component-image v2 requires page-granular segment bases. The linker script must therefore declare the load headers and their permissions as part of the artifact contract.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/runtime/c/component-aarch64.ld` | Collect GOT inputs in `.data`; declare explicit RX, R, and RW `PT_LOAD` headers; assign BSS to the RW header | Freestanding C program headers and permissions are deterministic across host LLD variants, and every load starts on a target page boundary |

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
| `objdump -p build/slisp-product.elf` after explicit `PHDRS` | Pass: exactly three loads at `0x400000` RX, `0x403000` R, and `0x404000` RW; the writable header carries BSS in its memory size | Direct |
| Exact `component_image("slisp", ..., aarch64-rpi5)` admission | Pass: emitted a component-image v2 blob instead of the segment refusal | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` after explicit `PHDRS` | Pass | Direct |
| `nix develop --command just sel4_rpi5_image_check` after explicit `PHDRS` | Pass on Darwin: built generation `97b99209a8ed840874ff8cd0ae031bb9d61f7e9265118b60849a2a9deea66711` and wrote the RPi5 ELF plus identity manifest | Direct |
| GitHub Actions run 33065116457 | Reproduced cross-host gap: QEMU passed; Linux RPi5 still refused Slisp after the Darwin-only GOT repair | Direct |
| Replacement hosted two-profile SDK publication run with explicit `PHDRS` | Pending after merge | Not observed |

## Decisions

- Decision: declare the C ELF program headers and their permissions explicitly.
- Rationale: the component-image contract consumes program headers, so output-section placement alone is insufficient when host LLD versions infer different segment boundaries.
- Rejected alternative: round or merge unaligned segments in `build-generation.py`. That would rewrite ELF mapping semantics, risk combining incompatible permissions, and hide future linker drift.

## Open risks and follow-ups

- [ ] The explicit-PHDR repair needs a hosted two-profile publication run after merge.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [initial segment failure 33063988008](https://github.com/iceice666/slime_os/actions/runs/33063988008), [host-dependent first fix 33065116457](https://github.com/iceice666/slime_os/actions/runs/33065116457).
- Serial/debugger/model output: none; generation construction failed before packaging or boot.
- Related roadmap item: [P4](../../roadmap/07-architecture-portability.md#p4--raspberry-pi-5-board-bring-up).
- Predecessor: [`devlog/2026-08-27-b80-rpi5-rust-target/`](../2026-08-27-b80-rpi5-rust-target/index.md)
