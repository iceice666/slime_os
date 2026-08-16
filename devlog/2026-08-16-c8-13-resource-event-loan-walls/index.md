# C8.13 — why `resourceEvent` and `resourceLoan` are structural walls, not scenario gaps

| Field | Value |
|---|---|
| Date | 2026-08-16 |
| Kind | Audit |
| Status | Root-caused |
| Scope | `components/bins/src/{operation_broker.rs,call_broker.rs}`, `components/runtime/src/syscall.rs`, `components/runtime/src/syscall/sel4_transport.rs`, `deps/rust-sel4/crates/sel4/src/syscalls.rs`, `contracts/fabric-trace/v1/schema.zt`, `components/proto/src/trace_sink.rs`, `roadmap/02-core-runtime.md` |
| Roadmap | C8.13 |
| Gates | none |
| Trigger | Asked to close C8.13's `resourceEvent` gap first; tracing the primitive it depends on found the roadmap's framing understated the difficulty |
| Baseline | The roadmap described `resourceEvent` as needing "the traffic schedule itself changed to create real backpressure on a client's delivery endpoint" and `resourceLoan` as needing "headroom freed in the call plane's trace sink or the schema's `maxTraceDepth` ceiling reconsidered" |

## Summary

Both open C8.13 resource classes were investigated by reading the mechanism
each depends on, not by attempting a fixture change first. Neither is a
scenario-design gap. `resourceEvent`'s backing `pending_deliveries` table can
never populate under *any* schedule: `operation_broker.rs::queue_delivery`
guards its retry path on `ERR_WOULDBLOCK`/`ERR_PEER_DEAD` from
`slime_rt::send`, but that call resolves to seL4's blocking `Cap::send` —
traced through three layers to a method that returns `()` and cannot produce
either code. `resourceLoan`'s backing state is real and traffic-varying, but
its worker's trace sink has zero ordinary capacity left: the call plane's 62
ordinary slots (`traceDepth=64` minus the schema's `TERMINAL_RESERVE=2`) are
already fully occupied by evidence C8.4–C8.13 already verified, and
`MAX_TRACE_DEPTH=64` is a page-sized (`64 × 64`-byte records) schema ceiling
the C8.11 conformance suite already exercises as a negative test rather than
a tunable fixture value. No code changed. The roadmap's open-risk wording is
corrected to state the real blocker for each, and both remain open pending an
explicit decision to accept the tradeoff either would require.

## Observable symptom

- Command: none run; this is a static trace, corroborated by the existing
  `just sel4_traffic_check` transcript already on record (2026-08-15/16
  passes), which never emits `[fabric] operation delivery queued` — the
  `queue_delivery` marker that would fire if `pending_deliveries` were ever
  populated.
- Expected (per roadmap wording): stalling a client should make
  `queue_delivery`'s `slime_rt::send` return `ERR_WOULDBLOCK`.
- Observed: `slime_rt::send`'s call chain has no path that returns
  `ERR_WOULDBLOCK` or `ERR_PEER_DEAD`, independent of scheduling.
- Evidence: source reading, cited in the Investigation log below.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `operation_broker.rs::queue_delivery` (`components/bins/src/operation_broker.rs:1117-1153`) calls `slime_rt::send(slot, ...)` and matches `ERR_SUCCESS`/`ERR_WOULDBLOCK`/`ERR_PEER_DEAD`, queuing into `pending_deliveries` only on `ERR_WOULDBLOCK`. | The evidence depends entirely on this call being able to return `ERR_WOULDBLOCK`. |
| 2 | `components/runtime/src/syscall.rs:145` documents `send` as blocking: "blocks until a receiver arrives, which deadlocks any sender that must stay responsive" — explicitly contrasted with `try_send`, which is offered for exactly that reason. | `send` is the wrong primitive for a broker that must stay responsive to other peers while queuing a deferred delivery; the doc comment already says so. |
| 3 | `sel4_transport::send` (`components/runtime/src/syscall/sel4_transport.rs:110-130`) can only return `ERR_INVALID_ARG` (oversized payload/cap count, from the top-of-function guard, or `stage_native_message`'s register-overflow check) or `Ok(ERR_SUCCESS)` after `send_staged_native`. `send_staged_native` (line 334-346) calls `endpoint.with(ipc_buffer).send(info)` and discards its result as a statement. | No code path in the wrapper can produce `ERR_WOULDBLOCK` or `ERR_PEER_DEAD`. |
| 4 | `Cap::send` (`deps/rust-sel4/crates/sel4/src/syscalls.rs:41-49`) — "Corresponds to `seL4_Send`" — has return type `()`. `seL4_Send` is the kernel's blocking send: it either delivers immediately to an already-blocked receiver or queues the sender in-kernel until one arrives. It has no non-blocking outcome to report. | Confirms step 3 down to the actual syscall: `slime_rt::send` is architecturally incapable of signaling "would block." |
| 5 | `operation_broker.rs::observe_client_death` (line 1274-1301) reads, in its own comment: "A native Endpoint never reports that peer's death, so its supervision transition is the authoritative close signal." Peer death is detected exclusively via a separate `slime_rt::supervision_status()` poll, never via `send`'s return code. | Independently confirms `ERR_PEER_DEAD` after `slime_rt::send` is also unreachable, by the codebase's own documented invariant. |
| 6 | `call_broker.rs::try_send_terminal` (line 1705-1735) is the codebase's own working pattern for exactly this problem: it uses `try_send` (`seL4_NBSend`), whose result the wrapper cannot distinguish either — so the helper deliberately maps every `ERR_SUCCESS` from `try_send` to `ERR_WOULDBLOCK`, and relies on an explicit client acknowledgement (`retire_terminal`, line 785-801) to know when a re-offered record has actually landed, not on the send's own return value. | Confirms the correct fix shape (non-blocking offer + explicit application-level ack), and that it is a real wire-protocol addition, not a broker-internal change — `queue_delivery` currently pushes ACCEPTED/FEEDBACK/RESULT/TERMINAL records, of which only TERMINAL is idempotent to re-offer the way the call plane's terminal is. |
| 7 | `contracts/fabric-trace/v1/schema.zt:157-163` and `components/proto/src/trace_sink.rs:187-190` (`ordinary_capacity() = capacity - TERMINAL_RESERVE`): with the traffic fixture's declared `traceDepth = 64` (`contracts/generation/v1/fixtures/sel4-traffic.zti:425`) and `TERMINAL_RESERVE = 2`, every worker's sink has exactly 62 ordinary slots. | Sets the exact ceiling `resourceLoan` would have to fit inside. |
| 8 | The 2026-08-16 queue/history-evidence devlog's own raw measurement: `call complete capacity=64 records=63` (62 ordinary + 1 terminal, the second reserved slot unused because nothing was dropped). | The call worker's 62 ordinary slots are already fully spent on existing, already-verified evidence; there is no partial headroom, not even for one record, let alone the peak+baseline pair `resourceLoan` would need under the established convention. |
| 9 | `devlog/2026-08-15-c8-11-semantic-trace/index.md`'s own conformance table: "`FABRIC_TRACE_DEPTH` hand-edited to 65 → `E0080` at compile time in two workers", exercised as a *negative* test the C8.11 mutation-style conformance suite relies on. | `MAX_TRACE_DEPTH = 64` is not a fixture knob C8.13 can turn — the project's own test suite already treats exceeding it as the tampering case a gate must reject, and each `WireTraceRecord` is 64 bytes, so a 64-record sink is exactly one page (`64 x 64 = 4096`) — raising it changes a page-aligned sizing convention used throughout this codebase, not a single constant in isolation. |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/02-core-runtime.md` | C8.13's "Not yet done" paragraph rewritten to name the real blocker for each of `resourceEvent` and `resourceLoan` (unreachable send-return-code path; fully saturated page-sized trace sink) instead of the prior "structural zero under this schedule" / "no trace-sink headroom" phrasing, which read as scenario-fixable | The roadmap states what would actually have to change to close each gap, not a description a fixture edit could appear to satisfy |

No component or contract code changed; this pass is investigation only, per the decision to accept both gaps as currently out of scope rather than attempt either fix.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Source trace: `operation_broker.rs` → `syscall.rs` → `sel4_transport.rs` → `deps/rust-sel4/crates/sel4/src/syscalls.rs` | `slime_rt::send`'s only possible returns are `ERR_SUCCESS` and `ERR_INVALID_ARG`; `Cap::send` returns `()` | Direct (source reading) |
| Source trace: `contracts/fabric-trace/v1/schema.zt`, `components/proto/src/trace_sink.rs`, `sel4-traffic.zti` | `ordinary_capacity() = 62` for every worker under the traffic fixture's declared `traceDepth = 64` | Direct (source reading) |
| Inherited: `devlog/2026-08-16-c8-13-queue-history-evidence/index.md`'s raw boot measurement (`call complete capacity=64 records=63`) | Call worker's 62 ordinary slots are fully spent | Inherited, same-day, cited above |
| Inherited: `devlog/2026-08-15-c8-11-semantic-trace/index.md`'s conformance table (`FABRIC_TRACE_DEPTH` hand-edited to 65 -> `E0080`) | `MAX_TRACE_DEPTH` is defended by an existing negative test, not adjustable as a fixture value | Inherited, cited above |
| No gate run; no runtime code touched | n/a | n/a |

## Open risks and follow-ups

- [ ] `resourceEvent`: making it real needs the call plane's `try_send` + explicit-ack pattern ported into `operation_broker.rs`'s mandatory-record delivery (ACCEPTED/FEEDBACK/RESULT/TERMINAL), which needs a wire-protocol acknowledgement addition (`contracts/fabric-operation/`) since only TERMINAL is idempotent to re-offer the way the call plane's terminal is. Declined: real regression risk to the verified `sel4_operation_check`/`sel4_boot_check`/`sel4_matrix_check` gates for one resource counter.
- [ ] `resourceLoan`: making it real needs either trimming existing verified call-plane trace evidence to free 2 ordinary slots (rejected in `devlog/2026-08-16-c8-13-queue-history-evidence/index.md` as trading real evidence for new evidence) or raising the schema's `maxTraceDepth` past its page-aligned 64-record ceiling, which the C8.11 conformance suite currently defends against as tampering. Neither attempted.
- [ ] Both gaps remain open against C8.13's exit condition (`roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings`); closing either requires an explicit decision to accept one of the tradeoffs above, not further scenario work.

## Artifacts and provenance

- Focused report: none; the investigation is summarized above and in the *Investigation log*.
- Raw transcript: none captured separately.
- Serial/debugger/model output: none generated by this pass; cites the two inherited entries' own transcripts.
- Related roadmap item: [C8.13](../../roadmap/02-core-runtime.md#c813--concurrent-cross-plane-traffic-and-resource-ceilings).
</content>
