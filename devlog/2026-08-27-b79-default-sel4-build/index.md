# B79: the default seL4 build lost its external Slisp input

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/build/build-sel4.py`, `slime-root/src/main.rs`, `just sel4_qemu_image_check`, SDK publication workflow run 33060731037 |
| Roadmap | B79, P5.2 |
| Gates | `just sel4_qemu_image_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | The first GitHub-dispatched SDK publication run invoked `just sel4_qemu_image_check` in a clean hosted checkout and failed while constructing the default generation |
| Baseline | The Slisp product cutover taught only the explicit component-graph image path to build and map the external Slisp ELF; local default-image builds had not been re-exercised from a clean checkout |

## Summary

The default seL4 image build again constructs the resident product generation. The Slisp cutover made the shared `sel4` manifest require an external `slisp-external` ELF but injected that mapping only for the explicit graph variant. A clean `just sel4_qemu_image_check` therefore failed before packaging, and once that path advanced it exposed an untyped disabled-keyboard `None` hidden by the graph build's QEMU-keyboard cfg. The default fixture and graph variants now share the same in-tree Slisp build, digest override, and explicit external mapping; the disabled input value is typed and compiled only when the non-fixture graph consumes it.

## Observable symptom

- Command: `just sel4_qemu_image_check` in GitHub Actions run 33060731037.
- Expected: build and install the QEMU seL4 prefix, construct the default generation, compile the root task, and package `build/slime-sel4.elf`.
- Observed: `build-generation.py` refused the generation with `missing external component ELF mapping(s): ['slisp-external']`; after restoring that mapping locally, the root task reported `E0282` for `let qemu_input = None` in the keyboard-disabled fixture build.
- Exit/fault/serial evidence: the hosted `Build seL4 prefixes` job failed with exit 1 and skipped the credential-touching publish job; no deployment approval was created.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Run 33060731037 built and verified the kernel prefix, then failed in `build-generation.py` on the missing `slisp-external` mapping | The workflow and hosted runner were not the failure source; default product generation construction was |
| 2 | `build-sel4.py` built and supplied Slisp only when `variant == GRAPH_VARIANT`, while the default fixture resolves the same `sel4` manifest | The mapping condition excluded a caller that consumes the identical external-component contract |
| 3 | Extending the condition to `FIXTURE_VARIANT` advanced compilation to `E0282` at the keyboard-disabled `None` | The graph variant's `SLIME_QEMU_KEYBOARD=1` cfg had masked a second default-only compile failure |
| 4 | Typing and cfg-scoping the disabled input let `just sel4_qemu_image_check` package the default image | Both clean-checkout blockers were removed at their shared source |

## Root cause

`build_application` treated external Slisp construction as a graph-image property rather than a property of the `sel4` generation manifest. Both the default fixture and graph variants select that manifest, whose authoritative component specification declares `slisp` with provider `external` and binary `slisp-external`. The fixture therefore reached `resolve_component_sources` without a required mapping. Independently, `qemu_input` relied on inference from `launch_instance_graph`; `slime_root_fixture` compiles that consumer out, so the keyboard-disabled `None` had no remaining type constraint.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-sel4.py` | Build the in-tree Slisp ELF and supply `slisp-external=<elf>` plus its digest for both default fixture and graph variants when no explicit component source override is present | Every builder selecting the resident `sel4` manifest supplies its declared external implementation |
| `slime-root/src/main.rs` | Give the disabled keyboard input its concrete `Option<Pl011Input>` type and compile the input binding only for non-fixture graphs | Each cfg combination type-checks independently rather than borrowing inference from code compiled out in fixture builds |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Default product generation omits Slisp again | `just sel4_qemu_image_check` | `missing external component ELF mapping(s)` or a Slisp content-hash refusal |
| A root cfg combination stops compiling | `just sel4_qemu_image_check` and `just lint_all` | Rust compilation or denied-warning failure |
| Python or Rust edits drift | `just ruff`, `just fmt_check_all` | Ruff or rustfmt failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| GitHub Actions run 33060731037 | Reproduced: prefix installation passed; default generation failed on missing `slisp-external`; publish was skipped | Direct |
| `nix develop --command just sel4_qemu_image_check` after both fixes | Pass: built external Slisp, admitted all six product executables, compiled the fixture root, and wrote `build/slime-sel4.elf` plus its identity manifest | Direct |
| `just lint_all`, `just ruff`, `just fmt_check_all`, `just devlog_check` | Pass | Direct |
| `nix develop --command ruff check scripts/build/build-sel4.py` | Pass | Direct |

## Decisions

- Decision: derive external Slisp setup from the variants that select the resident manifest, not from whether the final image is called a graph image.
- Rationale: the schema and component specification define the executable requirement; image naming does not.
- Rejected alternative: weaken `build-generation.py` to tolerate the missing external implementation. That would suppress the admission failure and produce no valid product generation.

- Decision: cfg-scope the QEMU input binding away from fixture builds in addition to typing its disabled value.
- Rationale: the fixture never launches the graph and should not carry a dead binding or warning; each active graph cfg still has one concrete input type.
- Rejected alternative: globally allow the unused variable. That would retain dead fixture code and hide future cfg drift.

## Open risks and follow-ups

- [ ] The SDK publication workflow needs a replacement dispatch after this fix reaches `main`; the failed run never reached the environment review or `m3air` publish job.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [GitHub Actions run 33060731037](https://github.com/iceice666/slime_os/actions/runs/33060731037).
- Serial/debugger/model output: none; the defect failed during hosted construction before boot.
- Related roadmap item: [P5.2](../../roadmap/07-architecture-portability.md#p52--native-component-images-on-sel4).
- Predecessor: [`devlog/2026-08-27-slisp-product-cutover/`](../2026-08-27-slisp-product-cutover/index.md)
