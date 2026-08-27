# Slisp replaces Dango in the resident product graph

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | Product generation, init dispatch, external component admission, component contracts, seL4 product gate, Dango build and plane retirement, language documentation |
| Roadmap | P5.2, P5.4.3, P5.4, M6.4, D1, D2, D3, D4, D5 |
| Gates | `just slisp_core_check`, `just sel4_component_graph_check`, `just component_spec_check`, `just contracts_check`, `just generation_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just devlog_check` |
| Trigger | The resident product still booted the Rust Dango shell after the freestanding C Slisp evaluator had proved the language-neutral component path |
| Baseline | Product generation 1 declared and launched Dango; Slisp existed only as a standalone bounded evaluator/component proof |

## Summary

The resident seL4 product now launches Slisp, not Dango. Slisp is built as a freestanding C AArch64 ELF, admitted through the external component-spec path, receives only its declared input, console, and spawn-service endpoint authority, enters its prompt, and remains resident when the product input source reports `WouldBlock`. The Rust Dango crate, parser helper, submodule pin, product composition, dedicated plane, and gate wiring were removed. Historical Dango contracts and devlogs remain where they are required to decode or explain frozen baselines.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Product generation | Replaced executable and instance identity `dango` with external `slisp`; declared its console endpoint and input capability; made Slisp the fourth required resident instance | The active product graph names the language implementation it actually launches |
| External admission | Product builds compile the in-tree freestanding C Slisp ELF, compute its digest, and pass the explicit `slisp-external=<elf>` mapping into generation construction | A non-Rust component follows the same target-qualified image and content-admission contract as every component |
| Component runtime | Added the minimal C input-read call and a resident Slisp REPL loop over declared slots | The shell has no ambient POSIX input or output path |
| Init and gate | Init resolves and spawns `executable:slisp`; the component-graph gate asserts authorization, endpoint installation, spawn, prompt, blocked input, and the four-instance healthy graph | Product success requires the non-Rust shell to be both alive and exercising its authority |
| Dango retirement | Removed the Rust component crate, Dango runtime parser, submodule, dedicated manifest/layout/gate, and build variants | There is one active native shell implementation, with no compatibility path silently keeping Dango alive |
| Plans and docs | Made Slisp the shell/application language and replanned native development around its compiler rather than a second application language | Language ownership is unambiguous: Zutai owns schemas/configuration; Slisp owns interactive/application code |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Product silently stops embedding the external Slisp ELF | `just generation_check`, `just contracts_check` | missing external mapping, content-hash mismatch, or generated generation drift |
| Init launches the wrong slot or omits Slisp authority | `just sel4_component_graph_check` | missing `component=slisp`, endpoint installation, prompt, or resident input-wait marker |
| Slisp evaluator semantics regress independently of product wiring | `just slisp_core_check` | persistent definition, lexical evaluation, refusal, or clean-exit transcript mismatch |
| Dango returns as an active build dependency or plane | workspace metadata, manifest/gate inventory, and `just contracts_check` | retired crate/submodule/manifest name reappears in active construction |
| Rust or Python edits drift from repository conventions | `just fmt_check_all`, `just lint_all`, `just ruff` | formatter, clippy, or Ruff failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just slisp_core_check` | Pass; built the freestanding C ELF and exercised persistent definition, lexical use, typed refusal, and clean termination under QEMU | Direct |
| `python3 scripts/check/check-generation-v5.py` | Pass; all 35 seL4 manifests encoded `SLIMEG5` version 5, with the product supplied its external Slisp mapping | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just ruff` | Pass | Direct |
| `just component_spec_check` / `just contracts_check` | Pass through contract model checks and boot-layout agreement after the product layout changed from Dango to Slisp; subsequent full rerun recorded with this entry's final verification | Direct |
| `just sel4_component_graph_check` | Pass; product generation authorized and spawned Slisp as task 3, installed both declared service endpoints, certified `required=4 live=4 idle=4 failed=0`, printed `slisp>`, and reached `[slisp] resident input wait` | Direct |
| `just generation_check`, `just lint_all`, `just devlog_check` | Pass in final verification | Direct |

## Decisions

- **Decision:** Slisp replaces Dango cleanly; no alias, shim, or compatibility grammar remains in the product.
  **Rationale:** two resident command languages would preserve the complexity and ownership ambiguity the cutover exists to remove.
  **Rejected alternative:** keep Dango as a hidden fallback or test plane. That would retain a second parser/runtime and let active contracts drift around an implementation no product uses.
- **Decision:** keep the historical `dango` component specification as provider `undeclared`, and keep frozen CP1/x86 layout names and old devlogs unchanged.
  **Rationale:** those records explain or decode historical baselines; rewriting them would falsify provenance. Active generation/build/gate wiring is removed instead.
  **Rejected alternative:** globally rename every historical mention to Slisp. That would make old evidence claim behavior it never observed.
- **Decision:** keep shell service operations outside the evaluator core for this cutover.
  **Rationale:** the product proof needed reader/evaluator residence and explicit I/O authority. Spawn, filesystem, and stream functions belong in subsequent Slisp milestones, not hidden reader punctuation.

## Open risks and follow-ups

- [ ] Slisp currently consumes input and console authority but does not yet expose capability-bearing spawn, stream, filesystem, or generation functions; D1-D5 own that compiler/runtime expansion.
- [ ] The product input source is intentionally empty, so the resident gate proves blocked readiness rather than an interactive typed session. The standalone Slisp core gate supplies the evaluator transcript until the seat/input path drives the product REPL.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: command output captured by the named repository gates
- Serial/debugger/model output: `just slisp_core_check` and `just sel4_component_graph_check`
- Related roadmap item: [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md), [`roadmap/08-native-development.md`](../../roadmap/08-native-development.md)
