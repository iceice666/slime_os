# P3.F - Duo Slisp product shell

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation-manifest/v1/compositions/sel4.md`, `scripts/{build,check}/`, `just/hardware.just`, `slime-root` |
| Roadmap | P3.F |
| Gates | `just slisp_core_check`, `just sel4_component_graph_check`, `just duo_slisp_check`, `just fmt_check_all`, `just lint_all` |
| Trigger | P3.E qualified upstream seL4 on the Milk-V Duo, leaving the resident Slisp graph without a board input adapter or observed interactive session |
| Baseline | The Duo graph could boot to READY and Slisp could run under QEMU, but the physical product had no UART0 receive path into `InputRead`, no bounded three-command hardware session, and no product-specific physical gate |

## Summary

P3.F makes Slisp the named resident product shell on the Milk-V Duo. `slime-root` maps the board's pinned DW APB UART0 receive page read-only, consumes line-status errors without forwarding corrupted bytes, and serves received bytes through Slisp's existing `InputRead` grant; Slisp receives no MMIO or device capability. The graph launches `console`, `spawn-service`, and Slisp as supervised residents, while shell commands retain their explicit spawn profile authority.

The physical gate builds and deploys a distinct `-test-terminator` FIT so the gate-only cold-reset byte cannot enter the ordinary product artifact. On the named board, one bounded session defined `answer`, reused that state to produce `42`, launched `sysinfo` through `spawn-service`, observed the matching supervision collection, and recorded zero framing errors before returning to vendor Linux.

## Changes

- Added a CV1800B DW APB UART receive adapter beside, but distinct from, the QEMU PL011 adapter. Its MMIO mapping remains inside `slime-root`; empty RX is `WouldBlock`, and line-status errors are drained and refused.
- Routed the Duo resident-product graph through the existing `InputRead` capability without granting Slisp ambient device authority.
- Preserved the existing resident graph and command profile; `init` launches the three resident services and Slisp spawns only declared command executables.
- Added the target-qualified Slisp build path, physical transcript checker, hardware recipe, and distinct test-terminator artifact identities.

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Production Duo artifact accidentally carries the gate reset trigger | `duo_slisp_check` builds `--duo-test-terminator` into distinct ELF, identity, FIT, and FIT-identity paths | Missing distinct artifact names or a product/gate digest collision fails identity and deployment checks |
| UART status errors become Slisp input | `DwApbInput::poll_byte` rejects errored status before returning a received byte | Component-graph root tests fail the error-before-data ordering contract |
| Slisp gains ambient UART authority | Generation grants expose only `InputRead`; the UART page is mapped inside `slime-root` | Boot-layout/component-graph checks detect a changed grant shape or missing input authority |
| The physical session passes without resident state or command authority | Ordered transcript markers require `define`, `42`, `spawn-service`, `sysinfo`, supervision collection, and zero framing errors | `duo_slisp_check` rejects missing, reordered, failed, or ambiguous markers |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just slisp_core_check` | Passed | Direct |
| `just sel4_component_graph_check` | Passed | Direct |
| `just duo_slisp_check /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0` | Passed on the named Milk-V Duo; retained state across three commands, launched `sysinfo`, collected supervision, and returned to vendor Linux with zero framing errors | Direct physical |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed | Direct |
| `just devlog_check` | Passed | Direct |

## Decisions

- **Decision:** Keep UART mechanism in `slime-root` and expose only `InputRead` to Slisp. **Rationale:** The shell needs bytes, not ambient MMIO authority; this preserves the capability boundary already qualified on QEMU. **Rejected alternative:** Granting the component the UART page would couple application policy to hardware and bypass syscall mediation.
- **Decision:** Compile the reset trigger only behind `--duo-test-terminator` and give that build distinct ELF, identity, FIT, and FIT-identity names. **Rationale:** A gate convenience must not alter or overwrite the ordinary target-qualified product artifact. **Rejected alternative:** An environment-only root build using the production filename made artifact identity indistinguishable after the overwrite.
- **Decision:** Treat UART line-status errors as consumed input errors and continue polling later bytes. **Rationale:** Forwarding errored data corrupts shell input, while permanently blocking after one error would violate the resident input contract.

## Open risks and follow-ups

- Physical evidence qualifies the named Milk-V Duo, UART0, vendor firmware, and observed serial session only. It does not establish storage, display, networking, suspend, or trust-boundary product support.
- The current input adapter polls UART0 from the root event loop. A future interrupt-driven path should preserve the same `InputRead` authority and error-ordering contract.

## Artifacts and provenance

- [`duo-slisp-session.log`](duo-slisp-session.log) is the immutable physical serial transcript captured by `just duo_slisp_check` from the deployed test-terminator FIT.
- [`duo-slisp-identities.json`](duo-slisp-identities.json) records the board, SoC, target profile, firmware, pinned UART identity, zero framing-error count, generation digest, Slisp digest, distinct ELF/FIT digests, and transcript digest.
- The deployed gate artifact was `/boot/slime-sel4-cv1800b-duo-test-terminator.itb`; the checker observed its FIT digest before reset and deployment.
- Related roadmap item: [P3.F](../../roadmap/07-architecture-portability.md#p3f--interactive-slisp-shell-on-the-milk-v-duo).
