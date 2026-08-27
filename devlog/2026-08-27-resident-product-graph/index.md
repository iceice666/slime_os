# P5.2 resident product graph and native Dango prompt

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | Product generation manifest, init supervision, Dango input residence, product boot layout, component-graph and gate-control checks |
| Roadmap | P5.2 |
| Gates | `just sel4_component_graph_check`, `just contracts_check`, `just generation_check`, `just sel4_gate_control_check` |
| Trigger | The product component graph completed its verification scenario and exited instead of leaving usable services running after boot |
| Baseline | The five-executable product generation launched console and spawn-service, explicitly shut both down, and certified success only after all required tasks exited |

## Summary

The product generation now boots a resident native Dango graph rather than a completed test scenario. It declares Dango as a sixth executable and fourth required live instance, installs only its generation-declared endpoint, input, directory, and shared-buffer authority, and has init retain supervision handles for Dango, console, and spawn-service. An empty product input source now reports `WouldBlock` instead of synthesizing Escape; Dango completes its startup probes, reaches the prompt, reports its first blocked input wait, and remains resident. The product QEMU gate certifies this live state instead of requiring task reclamation.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Product generation | Added Dango, its required instance, native console/spawn endpoints, input and directory capabilities, shared-buffer quota, and executable authority held by init | The declared product graph contains the interactive shell and every authority it actually uses |
| Init lifecycle | Product boot launches Dango after console and spawn-service, retains all three supervision handles, and loops while each remains alive; the demo composition retains its completion path | A successful product boot leaves required services running instead of shutting them down as test cleanup |
| Input source | Empty `ScriptedInput` returns `WouldBlock`; non-empty plane scripts still end with Escape | A product with no input driver waits for input while finite verification scripts terminate deterministically |
| Dango evidence | Dango emits one marker on its first blocked input read | The gate proves startup passed the shared-buffer probe and entered the resident input loop |
| Boot contract | Updated the derived product layout fixture and component-graph expectations for six executables and four live required instances | The frozen layout and QEMU gate match the generation the root actually admits and places |
| Roadmap | Updated P5.2's delivered graph, observed exit condition, gates, and evidence link | The canonical milestone record describes the current resident product rather than the retired five-component completion scenario |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Dango is absent, under-authorized, or assigned the wrong fixed runtime slots | `just contracts_check`, `just generation_check` | Manifest closure, boot-layout agreement, or deterministic generation construction fails |
| Init shuts down a required product service or fails to supervise it | `just sel4_component_graph_check` | Missing live-health, resident-supervision, prompt, or input-wait marker; any nonzero component exit is fatal |
| Empty product input fabricates Escape and closes the shell | `just sel4_component_graph_check` | `[dango] resident input wait` is absent and the gate does not reach its terminal marker |
| Gate evidence is weakened or made order-insensitive incorrectly | `just sel4_gate_control_check` | Marker count pin or a missing/reordered/failure mutation is accepted |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | Passed; 281 contract tests, product boot-layout agreement, all 36 seL4 manifests encoded generation v5, generated bindings current | Direct |
| `just generation_check` | Passed; deterministic generation and contract aggregate completed | Direct |
| `just sel4_component_graph_check` | Passed after rebuilding; six ELF payloads admitted, four required instances live, init resident, Dango startup probe and first blocked input wait observed | Direct |
| `just sel4_gate_control_check` | Passed; 41 gates rejected 1613 mutated transcripts and layouts | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed with warnings denied, including `slime-component-dango` | Direct |
| `just ruff` | Passed | Direct |

## Decisions

- Decision: reuse `ScriptedInput` as both the finite plane source and the empty product source, distinguishing them by whether the declared byte slice is empty.
  Rationale: the service already owns per-task cursor and `WouldBlock` semantics; a second input implementation would create a parallel convention before hardware input exists.
  Rejected alternative: always synthesize Escape on exhaustion, which makes an empty product session terminate immediately.
- Decision: retain Dango's established slot ABI and align the product manifest to it.
  Rationale: the binary's slots are the existing runtime contract exercised by the Dango plane; changing runtime constants or adding fallback slot probes would hide a malformed generation.
  Rejected alternative: accept the first continuous slot assignment and teach Dango a product-specific mapping.
- Decision: terminate the product gate on Dango's first blocked input wait, with root live-health and init supervision as ordered prerequisites.
  Rationale: root health is emitted immediately after spawn reply and can precede component startup; the Dango-ready line precedes its shared-buffer probe. The blocked-input marker is the first evidence that all startup checks passed and the shell actually entered residence.
  Rejected alternative: stop on the root health line or prompt, both of which allowed a later startup failure to escape observation.

## Open risks and follow-ups

- [ ] The resident product input source is intentionally empty; physical or QEMU keyboard delivery remains owned by the planned seat/input service and hardware work.
- [ ] The product gate proves the shell reaches its prompt and blocks correctly; it does not inject a command into this resident session. The scripted Dango plane continues to cover profile resolution, command execution, denials, and clean session shutdown.

## Artifacts and provenance

- Focused report: none; this entry is the focused record
- Raw transcript: command output observed directly in the implementation session; no separate frozen transcript was added
- Serial/debugger/model output: `just sel4_component_graph_check` observed live health, resident supervision, Dango prompt, and resident input wait; `just sel4_gate_control_check` observed 1613 rejected mutations
- Related roadmap item: [P5.2 — Native component images on seL4](../../roadmap/07-architecture-portability.md#p52--native-component-images-on-sel4)
