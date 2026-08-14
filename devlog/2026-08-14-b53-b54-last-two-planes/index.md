# B53, B54 — a line one byte past the message bound, and a component that never ends

| Field | Value |
|---|---|
| Date | 2026-08-14 |
| Kind | Defect |
| Status | Verified |
| Scope | `components/bins/src/bin/{dango,spawn-service}.rs`, `components/bins/src/bin/init.rs`, `contracts/generation/v1/fixtures/sel4-stress.zti` |
| Roadmap | B53, B54 |
| Gates | `just sel4_dango_check`, `just sel4_stress_check` |
| Trigger | B50's minted-endpoint deletion took both planes from *failing to admit* to booting, exposing what was behind that |
| Baseline | Both gates were red before B50's conversion and stayed red after it, each one layer deeper |

## Summary

Two unrelated defects, both hidden for as long as their planes could not boot at
all. `sel4_dango_check`: the shell's line buffer is 128 bytes and the transport's
message bound is 64, so echoing the second scripted line — 65 characters — was
refused with `ERR_INVALID_ARG` and ended the session one byte past the bound. Two
further B46 residues sat behind it: `spawn-service` read a working-directory
capability out of `received_caps[0]`, which since the native cutover carries only
Endpoint handles, and nothing told either service the session was over, so both
blocked in `recv` and held the graph open. `sel4_stress_check`: all 21 instances
ran `sample-worker`, which blocks in `recv_blocking` on a loopback endpoint its
*second thread* sends to — and the stress fixture declares no `extraThreads` and
no bindings, so nothing ever sent and no instance ever terminated. Both gates now
pass; all 27 gates pass.

## Observable symptom

- Command: `just sel4_dango_check`
- Expected: four scripted lines, two commands run, one denied, one parse error,
  session closes, plane completes.
- Observed: the first command ran end to end, then `dango` exited 1 with no
  diagnostic at all. `boot exceeded 300s without completing the plane`.
- Exit/fault/serial evidence: `result:exit:0` immediately followed by
  `SLIME_GRAPH component exit task=3 status=1`.

- Command: `just sel4_stress_check`
- Expected: 23 instances staged, then the graph reclaims to zero live tasks.
- Observed: `budget: the graph plans 3078 root CSlots of 3222 free`,
  `construction: all 23 declared instances were staged`, then
  `the graph never reclaimed to zero live tasks`.
- Exit/fault/serial evidence: 21 × `[sample-worker] main thread running`, and no
  `component exit` for any of them.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Dango exited 1 with no marker; every exit site was a bare `slime_rt::exit(1)` | Added a named `fail()` writing to `debug_write` — deliberately not `console`, since the console path is one of the suspects. This was the whole difference between guessing and knowing. |
| 2 | `[dango] fail: console send` | The failing call is a console send, not the input read the first hypothesis named. |
| 3 | `console` (task 1) never exits and its endpoint is installed on both ends (`native endpoint task=1 slot=33`, `task=3 slot=34`) | Not a missing or dead peer. |
| 4 | `MAX_LINE_BYTES = 128`, `MAX_MSG = 64`; script line 2 is 65 bytes | `send` refuses an oversized payload before the kernel sees it. `console(&line[..len])` echoing a 65-byte line cannot succeed. |
| 5 | Chunked the console writes: lines 1–4 all ran, dango exited 0 | Root cause confirmed. Two further failures surfaced behind it: `spawn-error` on line 2, and three live tasks at the end. |
| 6 | Line 2's cwd capability was exported (`capability exported task=3 id=2 kind=directory`) and `spawn-service` still refused the request | The service reads `received_caps[0]`, which is empty: a directory export arrives alone and must be claimed. |
| 7 | After claiming it, line 2 ran (`[echo-agent] command=echo … cwd=explicit stdin=explicit`) and only teardown remained | `console` and `spawn-service` both block in `recv` forever; a native Endpoint reports no peer death, so neither can infer the shell is gone. |
| 8 | For the stress plane: 21 × `[sample-worker] main thread running` and no exits | `sample-worker`'s main thread blocks in `recv_blocking` on `LOOPBACK_SLOT`, whose sender is its own worker thread. |
| 9 | The stress fixture declares no `extraThreads` and `bindings = []` | There is no second thread to send and no endpoint installed, so the receive can never complete. The component is wrong for this plane, not the plane for the component. |

## Root cause

**B53.** `dango.rs` sized its line buffer (`MAX_LINE_BYTES = 128`) independently
of the transport bound it echoes through (`MAX_MSG = 64`). A buffer larger than
one message cannot assume one send, and `send` returns `ERR_INVALID_ARG` for an
oversized payload rather than fragmenting. The invariant violated: *a component
that owns a buffer larger than the message bound owns the chunking too.*

Behind it, two instances of the residue class B46 named. `spawn-service` read a
transferred capability from `received_caps`, which since the cutover carries only
native Endpoint handles — every other kind travels as an export the receiver
claims with `capability_import`. And neither service had any way to learn the
session ended: the spawn protocol has carried `REQUEST_FLAG_SHUTDOWN` all along
and nothing sent it, while `console` exits on a specific close message nobody
sent. *Endpoints carry messages; a peer's exit is not one of them.*

**B54.** `sel4-stress.zti` names `sample-worker` for all 21 stress instances.
That component exists to prove B47's two-threads property: its main thread blocks
in `recv_blocking` on a loopback endpoint and its *worker thread* sends. The
stress fixture declares neither the extra thread nor the endpoint binding, so
every instance parked on a receive with no possible sender. B49's claim is about
the *number* of instances the CSpace admits, so the component only has to be one
that terminates — `supervision-child` is exactly that and nothing else.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `dango.rs` | `console()` writes in `MAX_MSG`-sized chunks | A buffer larger than one message does not assume one send |
| `dango.rs` | Every `exit(1)` becomes `fail(b"…")` writing a named reason to `debug_write` | A component that stops says why, over a path independent of the mechanism under test |
| `dango.rs` | On `Escape`, sends `REQUEST_FLAG_SHUTDOWN` to the spawn service and the close message to the console | The party that owns an edge closes it; a native Endpoint reports no peer death |
| `spawn-service.rs` | Claims the working directory with `capability_import`; `valid_request` checks the declared role against what actually arrived | The received-capability array carries native Endpoint handles only (B46) |
| `init.rs` | Emits `[init] console spawned`, `[init] spawn service spawned`, `[init] dango spawned` | The gate's composition markers name events that occur |
| `sel4-stress.zti` | 21 instances run `supervision-child` instead of `sample-worker` | A plane that asserts teardown uses a component that terminates |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component echoes a buffer past the message bound again | `just sel4_dango_check` | The second scripted line is 65 bytes, one past `MAX_MSG`, so an unchunked send fails immediately |
| A dango failure becomes silent again | `just sel4_dango_check` | Every exit prints `[dango] fail: <reason>`; the gate's failure markers catch the line |
| A service reads a transferred capability from `caps[0]` | `just sel4_dango_check` | Line 2's `with-cwd` leg reports `spawn-error` |
| A service is left with no way to learn its client is gone | `just sel4_dango_check` and `just sel4_stress_check` | `graph iterations exhausted live=N` — the graph does not quiesce |
| A plane asserting teardown adopts a blocking component | `just sel4_stress_check` | `the graph never reclaimed to zero live tasks` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_dango_check` | PASS — all four scripted lines, both commands, both denials, clean close | Direct |
| `just sel4_stress_check` | PASS — 23 instances staged and reclaimed to zero | Direct |
| All 27 gates: the two above plus `sel4_boot_layout_check`, `generation_check`, `sel4_gate_control_check`, `sel4_root_boot_check`, `sel4_channel_check`, `sel4_crossing_check`, `sel4_loan_check`, `sel4_sample_check`, `sel4_stream_check`, `sel4_qos_check`, `sel4_visibility_check`, `sel4_spawn_check`, `sel4_supervision_check`, `sel4_reclamation_check`, `sel4_input_check`, `sel4_directory_check`, `sel4_filesystem_check`, `sel4_generation_check`, `sel4_storage_check`, `sel4_store_check`, `sel4_rollback_check`, `sel4_recovery_plane_check`, `sel4_transfer_check`, `sel4_powerbox_check`, `devlog_check` | PASS | Direct |
| `just fmt_check_all`, `just lint_all`, `just test_sel4_root`, `just test_host`, `just contracts_check` | PASS | Direct |

## Decisions

- Decision: instrument dango's exit sites before hypothesising further.
- Rationale: three successive hypotheses were wrong — the input read, the script
  keying, and the spawn RPC's transferability — and each cost a full build-and-boot
  to disprove. A component whose every failure path is a bare `exit(1)` makes a
  transcript that shows only success, which is worse than no transcript. The named
  `fail()` found the real call on the first run after it landed.
- Rejected alternative: reading further hypotheses out of the source. The
  transferability change was made this way, measured, and reverted — the leg that
  would have needed it was on a script line the run never reached.

- Decision: `fail()` writes to `debug_write`, not `console`.
- Rationale: the console path was itself the fault. A diagnostic that travels over
  the mechanism under test says nothing exactly when it is needed.

- Decision: the stress plane changes its component rather than the component
  gaining a second thread.
- Rationale: B49's exit condition is the *count* of instances the root's CSpace
  admits, and `sample-worker` is B47's two-thread subject — the stress fixture had
  borrowed a component whose whole body depends on a thread and an endpoint it does
  not declare. `supervision-child` is the component that exists to run and end.
- Rejected alternative: declaring `extraThreads` and a loopback endpoint on all 21
  instances. That would add 21 TCBs, 21 IPC buffers, and 21 Endpoints to a plane
  whose entire point is to sit at the CSpace ceiling — it would change what the
  gate measures in order to keep an accident.

## Open risks and follow-ups

- [ ] `MAX_LINE_BYTES` (128) still exceeds `MAX_MSG` (64) by construction. The
      chunking makes it correct, but any future component pairing a large buffer
      with a single send repeats this. A `debug_assert` or a shared write helper in
      `slime_rt` would make the coupling explicit rather than remembered.
- [ ] Every backlog entry is now resolved. The two failures this entry closes were
      the last, so the next reader starts from a fully green suite.

## Artifacts and provenance

- Focused report: none; the decisive chain is in the investigation log.
- Raw transcript: none retained. Each observation is a serial line reproducible by
  the named `just` target.
- Serial/debugger/model output: quoted inline where a marker is the evidence.
- Related roadmap item: [B53 and B54](../../roadmap/00-backlog.md), both created by
  [B50's minted-endpoint deletion](../2026-08-14-b50-minted-endpoint-deletion/index.md).
