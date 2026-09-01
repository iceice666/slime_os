# B92: the resident product graph inherited a finite verification-plane lifetime

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime-root/src/graph_runtime/services.rs`, `scripts/check/check-duo-slisp.py`, resident product dispatch |
| Roadmap | B92, P3.F |
| Gates | `just duo_slisp_check`, `just sel4_component_graph_check`, `just sel4_fabric_aggregate_check` |
| Trigger | A physical Milk-V Duo Slisp session stopped after printing `SLIME_GRAPH exhausted live=4 iterations=32768 certified=1` |
| Baseline | P3.F declared console, init, spawn-service, and Slisp resident, but its physical gate ended before the dispatcher request ceiling |

## Summary

The Milk-V Duo resident Slisp product stopped accepting input after ordinary idle polling accumulated 32768 root-service requests. `serve_instance_graph` applied the finite verification-plane livelock ceiling to every boot action, including authenticated `product`, then returned to root teardown after reporting certified exhaustion. Product dispatch now has no request-count lifetime; finite planes retain the same watchdog. The physical gate crossed the former boundary and evaluated a stateful Slisp expression afterwards.

## Observable symptom

- Command: boot `/boot/slime-sel4-cv1800b-duo.itb`, leave the Slisp prompt resident, and observe UART0.
- Expected: the declared product services remain live and Slisp continues accepting input.
- Observed: the root printed `SLIME_GRAPH exhausted live=4 iterations=32768 certified=1`, followed by service and allocator summaries, then stopped serving the four live resident tasks.
- Exit/fault/serial evidence: no component fault or nonzero exit preceded the stop; `certified=1`, `live=4`, and `tasks=4` showed a healthy resident graph had reached the generic request ceiling.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The terminal line names exactly `MAX_GRAPH_ITERATIONS` and four live tasks | The stop is the root dispatch bound, not a Slisp crash |
| 2 | Product init, Slisp, and spawn-service poll non-blocking capabilities and yield when they receive `WouldBlock` | Healthy idle operation continuously consumes root requests |
| 3 | `serve_instance_graph` bounded every generation action with the same `while iterations < MAX_GRAPH_ITERATIONS` loop | A resident product had a deterministic finite lifetime |
| 4 | `BootAction::Product` is authenticated generation data and init already treats it as the resident-service action | The root can distinguish resident product policy without a board-specific or component-specific exception |
| 5 | Traffic and fault aggregate planes still completed two boots each after retaining the finite ceiling | Separating product lifetime did not remove finite-plane wedge detection |
| 6 | The physical Duo crossed request 32768, then evaluated `(+ answer 3)` as `43` | The reported failure boundary no longer terminates service and Slisp state remains live afterwards |

## Root cause

`MAX_GRAPH_ITERATIONS` is a finite-workload progress watchdog. It lets QEMU verification planes fail or report when a graph consumes 32768 component requests without draining. The same loop surrounded `BootAction::Product`, although product init deliberately supervises console, spawn-service, and Slisp forever. Their non-blocking status and input reads are ordinary root requests, so an idle healthy product inevitably reached the watchdog. The post-loop path then printed accounting summaries and returned to `main`, which stopped dispatching. The violated invariant was that a declared resident product lifetime is bounded by task health and explicit lifecycle events, never by the number of valid requests it has served.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/graph_runtime/services.rs` | Derive the iteration limit from authenticated `generation.boot_action`: `Product` is unbounded; every other action retains `MAX_GRAPH_ITERATIONS` | Valid product requests cannot consume the resident graph's lifetime |
| `slime-root/src/graph_runtime/services.rs` | Emit one `SLIME_GRAPH resident checkpoint` at request 32768 without returning | The former failure boundary is directly observable without changing product behavior |
| `scripts/check/check-duo-slisp.py` | Wait for the checkpoint, fail immediately on the old exhaustion marker, then evaluate `(+ answer 3)` and require `43` before reset | The physical product gate proves service and state survive beyond the old ceiling |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Product dispatch regains a finite request lifetime | `just duo_slisp_check <serial>` | `SLIME_GRAPH exhausted ... certified=1`, missing checkpoint, or no post-boundary `=> 43` |
| Product behavior breaks before the lifetime boundary | `just sel4_component_graph_check` | Missing resident graph, Slisp evaluation, or authorized `sysinfo` completion |
| The product exception accidentally disables finite-plane wedge accounting | `just sel4_fabric_aggregate_check` | Either aggregate schedule fails its own gate or does not cleanly exit across two boots |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_component_graph_check` | Passed; the QEMU product launched all four required residents, evaluated Slisp input, and launched `sysinfo` | Direct |
| `just sel4_fabric_aggregate_check` | Passed; traffic and fault schedules each passed two boots with 139 and 140 byte-identical semantic records | Direct |
| `python3 scripts/check/check-duo-slisp.py --serial /dev/serial/by-id/usb-1a86_USB_Serial-if00-port0 --no-build --evidence-dir devlog/2026-09-01-b92-resident-graph-lifetime` | Passed on the named Milk-V Duo; crossed 32768 requests, evaluated `(+ answer 3)` as `43`, observed zero framing errors, and returned to vendor Linux | Direct; [`duo-slisp-session.log`](duo-slisp-session.log), [`duo-slisp-identities.json`](duo-slisp-identities.json) |
| Fixed product FIT deployment | `/boot/slime-sel4-cv1800b-duo.itb` and the local build both hashed `7e7be80d2d266d9f63468f19a108f398af09ab881c200c32a1e25de3df4e3d59` | Direct |
| `just sel4_qemu_image_check` | Passed; rebuilt and installed the pinned AArch64 QEMU image and seL4 prefix | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed with warnings denied | Direct |
| `just ruff` | Passed | Direct |
| `just devlog_check` | Passed: 275 entries, 275 indexed | Direct |
| Architecture-matched `slime-root` host tests under `qemu-aarch64` | Passed: 214/214 against the pinned AArch64 seL4 prefix | Direct |

## Decisions

- Decision: exempt only authenticated `BootAction::Product` from the request-count ceiling.
- Rationale: product is the one action whose declared success state is a resident service graph; the distinction is generation policy already consumed by init, not an inferred workload shape.
- Rejected alternative: raise `MAX_GRAPH_ITERATIONS`. Any finite replacement only delays the same deterministic death.
- Rejected alternative: remove the ceiling globally. Finite verification planes still need bounded failure and B74's explicit exhaustion evidence.

## Open risks and follow-ups

- [ ] Resident components still poll non-blocking capabilities and therefore generate continuous IPC while idle. A future blocking wait path may reduce CPU and serial diagnostic pressure, but it is not required for lifetime correctness and must preserve explicit capability authority.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`duo-slisp-session.log`](duo-slisp-session.log), frozen from the passing physical gate after the reviewer fix.
- Serial/debugger/model output: [`duo-slisp-identities.json`](duo-slisp-identities.json) binds the FIT, ELF, generation, Slisp ELF, board identity, zero framing-error count, and transcript SHA-256.
- Related roadmap item: [B92](../../roadmap/00-backlog.md), [P3.F](../../roadmap/07-architecture-portability.md#p3f--interactive-slisp-shell-on-the-milk-v-duo)
