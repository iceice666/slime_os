# B93: the block driver's shutdown drain raced the root's reclamation of its client

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Defect |
| Status | Monitoring |
| Scope | `components/lib/src/block_io.rs`, `components/services/virtio-blk-driver/src/main.rs`, `contracts/system-image-closure/v1/closures/`, `contracts/system-test-run/v1/runs/`, `.tasks/items/` |
| Work items | 01a08f34-5e0f-7c61-90de-ce3268c872d2 |
| Gates | `just sel4_rollback_check`, `just io_block_check`, `just bootstate_trace_check` |
| Trigger | Upstream CI run 34474105011, the push of `65443ce0` (PR #26's merge) to `main`, failed `sel4_rollback_check` although the same tree had passed that job on the PR branch 47 minutes earlier |
| Baseline | Every block-holding plane pins `[virtio-blk-driver] peer complete, exiting` before its client's completion marker, and the driver stays resident until then |

## Summary

One CI run of the rollback plane ended with the userspace block driver faulting immediately
after the root reclaimed its only client: the probe exited, was reclaimed, and then
`slime-root` reported a required-instance fault in `virtio-blk-driver`, so the checker timed
out at 240 s. The client's shutdown was one blocking send, which completes the moment the
polling driver receives it; the driver then runs a final drain over the ring the client lent
it. Nothing kept the client from exiting first, and when it did the root reclaimed its loans
and unmapped the ring from under the drain. Fifteen unmodified local boots never took that
interleaving, but widening the driver's window between receiving the command and draining
reproduced the CI transcript, numbers and fault line included, on every boot. The shutdown is
now a call the driver answers only after its drain, so the client cannot exit while the ring
is in use; with the window still widened the fault is gone, and every block-holding plane
passes.

## Observable symptom

- Command: CI job "Rollback, release trust, and BootState trace" (`.github/workflows/ci.yml`),
  step `nix develop --command just bootstate_trace_check`, which runs `just
  sel4_rollback_check`; runner `ubuntu-24.04-arm`; run 34474105011, job 102860627500.
- Expected: `[virtio-blk-driver] peer complete, exiting`, then `[sel4-rollback-probe] rollback
  plane complete`, then `[init] rollback plane complete`; the checker reports 19 observed
  markers and 7 durable transitions.
- Observed: `[sel4-rollback-probe] rollback plane complete` came first, its reclamation
  reported `SLIME_GRAPH holder reclaimed task=3 charges=6 actions=36`, then `SLIME_ROOT FATAL
  SLIME_GRAPH FAIL required instance virtio-blk-driver fault kind=VirtualMemory { access:
  Execute, status: 64 } instruction=Some(1527) address=Some(64)`, then `seL4 rollback plane
  check: boot exceeded 240s without completing the plane`.
- Exit/fault/serial evidence: [`ci-run-34474105011-rollback-job.log`](ci-run-34474105011-rollback-job.log)
  lines 1820-1863.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | The failing run is the push of `65443ce0` to `main`. Run 34470021612 of the identical tree on the PR branch passed the same job 47 minutes earlier; PRs #28 and #27, rebased onto it, passed; both later pushes to `main` passed | Intermittent; not a regression in the merged content |
| 2 | Local: `just sel4_qemu_image_check`, then `just sel4_rollback_check` five times on `a1119ca6` ([`campaign-summary.txt`](campaign-summary.txt)), then ten boots through the checker's own `build_fixture`, `boot`, `check_transcript`, and `check_slots_durable` with every transcript kept ([`repro.py`](repro.py), [`repro-summary.txt`](repro-summary.txt), [`transcript-1.log`](transcript-1.log) … [`transcript-10.log`](transcript-10.log)) | 15/15 passed; no fault |
| 3 | In every local transcript the driver prints `peer complete, exiting` and is reclaimed (`holder reclaimed task=2 charges=2 actions=9`) *before* the probe prints `rollback plane complete` and is reclaimed (`task=3 charges=4 actions=27`). In the CI transcript the probe went first and its reclamation carried `charges=6 actions=36` | The two extra charges are the two loans the driver still held: the root reclaimed the ring out from under a live driver |
| 4 | `BlockIo::shutdown` was one blocking `send(&[1])`, which returns as soon as the polling driver's non-blocking `recv` takes it (`components/lib/src/block_io.rs`); the driver then runs a final `drain()` before exiting (`virtio-blk-driver/src/main.rs`), whose own comment names a drain after the client's reclamation as a use-after-free | The window between the driver's receive and the end of its drain was unguarded; whichever side the scheduler ran first decided the outcome |
| 5 | Widening that window with a bounded `yield_now()` loop after the command was received, then booting under the checker: the transcript reproduced the CI ordering and every number in it, `charges=6 actions=36`, `slots=2791`, `live_objects=240` ([`widened-checker-transcript-1.log`](widened-checker-transcript-1.log)); the checker failed on the missing driver marker | The CI interleaving is the widened one; the ordered marker chain fails on it even before any fault |
| 6 | Booting the widened image directly for 15 s ([`repro_raw.py`](repro_raw.py), [`widened-raw-boot-1.log`](widened-raw-boot-1.log), [`widened-raw-boot-2.log`](widened-raw-boot-2.log)): after the probe's reclamation, `SLIME_ROOT FATAL SLIME_GRAPH FAIL required instance virtio-blk-driver fault kind=VirtualMemory { access: Execute, status: 64 } instruction=Some(1356) address=Some(64)` on both boots | Reproduced deterministically; same fault class, status, and address as CI, with the instruction pointer differing only by build |
| 7 | With the shutdown changed to a call answered after the drain, and the widened window left in place: the checker passed twice ([`fixed-widened-checker-transcript-1.log`](fixed-widened-checker-transcript-1.log)) and two 15 s direct boots showed no fault and the driver's exit before the probe's ([`fixed-widened-raw-boot-1.log`](fixed-widened-raw-boot-1.log), [`fix-verify-summary.txt`](fix-verify-summary.txt)) | The rendezvous closes the window; the fault does not recur where it was deterministic |
| 8 | The driver image links at `0x200000`; both reported values, `0x54c`/`0x5f7` and `0x40`, lie in page zero, and seL4 derives both from the same restart PC for a prefetch abort (`deps/sel4/src/arch/arm/api/faults.c:37-38`, `src/arch/arm/64/kernel/vspace.c:732-744`) | Which instruction the driver executed last is not established; the reproduction and the fix do not depend on it |

## Root cause

`BlockIo::shutdown` completed when the driver *received* the command, not when the driver was
done with the client's memory. The driver answers a shutdown by draining the ring one last
time, so a request published between its poll and its receive is still completed and its
lease settled. That drain reads the ring page and the data pages the client lent it. A client
that returns from `shutdown` and exits has its holdings reclaimed by the root, which revokes
the loans and unmaps them from the driver's VSpace. The ordering between the client's exit and
the driver's drain was left to the scheduler: on this host the driver always ran first, on the
CI runner it once did not, and a widened window makes it never run first. The violated
invariant is the one the driver's peer-gone arm already protects: the driver must not touch a
client's loaned pages after the root may have reclaimed them.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/lib/src/block_io.rs` | `shutdown` is a `call`, not a `send`; it returns when the driver replies | A client cannot exit, and have its loans reclaimed, while the driver is still draining its ring |
| `components/services/virtio-blk-driver/src/main.rs` | On `Shutdown`: drain, print `peer complete, exiting`, then `reply`, then exit; the exit marker moved from after the loop into the arm | The driver's last access to the ring precedes the reply; the exit marker precedes anything the client prints after its call returns, so the plane checkers' ordered chains no longer depend on the scheduler |
| `contracts/system-image-closure/v1/closures/*.zti`, `contracts/system-test-run/v1/runs/*.zti` | Regenerated for the changed component identities | Every closure names the bytes it builds |
| `.tasks/items/` | B93 (`01a08f34-5e0f-7c61-90de-ce3268c872d2`) filed as a `backlog` bug | The backlog-first rule sees the defect |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The client exits before the driver's drain | `just sel4_rollback_check` (in CI through `just bootstate_trace_check`) | `marker out of order` on the driver's exit marker, or `SLIME_ROOT FATAL … virtio-blk-driver fault` then the 240 s timeout |
| The rendezvous deadlocks a client | `just io_block_check` and the seven other block-holding planes | The plane's client never prints its completion marker; the gate times out |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| CI run 34474105011, job 102860627500 (`bootstate_trace_check`) | Failed as described; log saved as [`ci-run-34474105011-rollback-job.log`](ci-run-34474105011-rollback-job.log) | Inherited: <https://github.com/iceice666/slime_os/actions/runs/34474105011> |
| CI runs 34470021612 (PR branch), 34483741557 (PR #28), 34482381956 (PR #27), 34485529620 and 34494980760 (`main`) | The same job passed | Inherited: GitHub Actions for `iceice666/slime_os` |
| `just sel4_rollback_check` ×5 and ten checker-driven boots, unmodified tree | Passed 15/15 ([`campaign-run-1.log`](campaign-run-1.log) … [`campaign-run-5.log`](campaign-run-5.log), [`transcript-1.log`](transcript-1.log) … [`transcript-10.log`](transcript-10.log)) | Direct |
| Widened window, unfixed: two 15 s direct boots | Both faulted with the CI signature ([`widened-raw-boot-1.log`](widened-raw-boot-1.log), [`widened-raw-boot-2.log`](widened-raw-boot-2.log)) | Direct |
| Widened window, fixed: two checker boots and two 15 s direct boots | All passed, no fault, driver exit first ([`fixed-widened-checker-transcript-1.log`](fixed-widened-checker-transcript-1.log), [`fixed-widened-raw-boot-1.log`](fixed-widened-raw-boot-1.log)) | Direct |
| `just sel4_rollback_check`, `just io_block_check`, `just sel4_storage_check`, `just sel4_store_check`, `just replay_check`, `just sel4_generation_check`, `just sel4_filesystem_check`, `just sel4_recovery_plane_check`, `just sel4_transfer_check`, `just bootstate_trace_check` | All passed on the final source (closures regenerated, test-run records re-blessed). `replay_check` failed once with `missing marker: \[virtio-blk-driver\] authority rings=1 rights=read,write source=generation`, the first driver marker of its ordered chain, while two zutai-bound gates saturated a core; the checker keeps no transcript. Eight transcripts kept through the checker's own `boot()` all satisfy its matcher and the gate passed on rerun; the one-off is filed as B94 (`01a08fa2-ffe1-7b90-a842-b2b22166b9fa`) | Direct |
| `just system_spec_check`, `just system_image_builder_check`, `just system_test_run_check`, `just system_image_closure_check`, `just system_image_closure_aggregate_check` | All passed (the two spec-bound gates took 21 and 63 minutes) | Direct |
| `just fmt_check_all`, `just lint_all`, `just test_host`, `just sel4_gate_control_check`, `just devlog_check`, `just tasks_check` | All passed; `just typos` passed as well; `devlog_check` and `tasks_check` passed after B93 closed and every evidence file was linked | Direct |

## Decisions

- Decision: make the shutdown a rendezvous the driver answers after its drain, on the client side of the existing protocol byte.
- Rationale: the invariant belongs to the client-driver protocol, not to the scheduler; a `call` is the primitive the runtime names for exactly this, and one byte on the wire stays one byte.
- Rejected alternative: skip the final drain. The driver's own comment explains why the drain exists: a request published between the driver's poll and its receive would otherwise be left with no completion and an unsettled lease.
- Rejected alternative: have the client wait on the driver's `state_changed` notification or its termination. Both add a second channel to a protocol that already has a request/answer pair.
- Rejected alternative: defer without a fix. The widened window turned a one-in-six CI failure into a deterministic reproduction, and the fix removes it under the same widening.

## Open risks and follow-ups

- [ ] The two fault registers CI and the reproduction reported (`instruction` 0x5f7/0x54c, `address` 0x40) lie in page zero and are not explained by a data abort on the unmapped ring; the last instruction the driver executed was not decoded. Nothing in the fix depends on it.
- [ ] The same shape exists wherever a service drains a client's loaned pages after a command: the TCP lane's network service must answer a client's close only after its last access to that client's queue and data pages.
- [ ] One CI run of the rollback job on this change is the remaining observation; the item closes on it.
- [ ] The replay plane's ordered chain expects the root's `SLIME_RECORD entry … instance=replay-unrecorded` line before the driver's startup markers; one gate run under host load matched the chain differently and the checker kept no transcript. Filed as B94 (`01a08fa2-ffe1-7b90-a842-b2b22166b9fa`), deferred; it is a startup ordering, not the shutdown path this entry changes.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: [`ci-run-34474105011-rollback-job.log`](ci-run-34474105011-rollback-job.log) (the job log as downloaded through `gh api`); unmodified tree, ten checker-driven boots: [`transcript-1.log`](transcript-1.log), [`transcript-2.log`](transcript-2.log), [`transcript-3.log`](transcript-3.log), [`transcript-4.log`](transcript-4.log), [`transcript-5.log`](transcript-5.log), [`transcript-6.log`](transcript-6.log), [`transcript-7.log`](transcript-7.log), [`transcript-8.log`](transcript-8.log), [`transcript-9.log`](transcript-9.log), [`transcript-10.log`](transcript-10.log); widened window, unfixed: [`widened-checker-transcript-1.log`](widened-checker-transcript-1.log), [`widened-raw-boot-1.log`](widened-raw-boot-1.log), [`widened-raw-boot-2.log`](widened-raw-boot-2.log); widened window, fixed: [`fixed-widened-checker-transcript-1.log`](fixed-widened-checker-transcript-1.log), [`fixed-widened-raw-boot-1.log`](fixed-widened-raw-boot-1.log); checker stdout of the five-run campaign: [`campaign-run-1.log`](campaign-run-1.log), [`campaign-run-2.log`](campaign-run-2.log), [`campaign-run-3.log`](campaign-run-3.log), [`campaign-run-4.log`](campaign-run-4.log), [`campaign-run-5.log`](campaign-run-5.log).
- Serial/debugger/model output: [`campaign-summary.txt`](campaign-summary.txt), [`repro-summary.txt`](repro-summary.txt), [`fix-verify-summary.txt`](fix-verify-summary.txt), [`repro.py`](repro.py), [`repro_raw.py`](repro_raw.py).
- Related work item: B93, `.tasks/items/01a08f34-5e0f-7c61-90de-ce3268c872d2.md`.
