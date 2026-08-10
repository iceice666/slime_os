# B41 — why the root cannot yet have a second dispatcher

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Audit |
| Status | Root-caused |
| Scope | `deps/rust-sel4/crates/sel4/src/{state/mod.rs,state/token.rs,syscalls.rs}`, `deps/rust-sel4/crates/sel4-kernel-loader/{add-payload/src/utils.rs,payload-types/src/lib.rs}`, `deps/rust-sel4/support/targets/aarch64-sel4-roottask*.json`, `slime-root/src/main.rs` |
| Roadmap | B41, B43, B44, B45 |
| Gates | `just sel4_boot_check` |
| Trigger | B41, B43, B44, and B45 all require console/block/store traffic to leave the universal dispatcher, which requires something else to receive it. |
| Baseline | One root thread, one endpoint, `DebugWrite`/`BlockTransact`/`StoreTransact` as universal operation labels. |

## Summary

Four backlog items need a second receiver in the root task. This entry records
why there cannot be one yet, with the experiment run to the point of failure on
both candidate targets. The obstruction is in `deps/rust-sel4`, a pinned
vendored dependency, and it is different on each target. Everything on the
slime side works; the tree carries none of the experiment.

## Observable symptom

- Command: `just sel4_boot_check` with a console dispatcher thread wired in.
- Expected: the thread receives on the per-process console endpoint.
- Observed on `aarch64-sel4-roottask-minimal`:
  `panicked at deps/rust-sel4/crates/sel4/src/state/mod.rs:167` — the
  `BorrowMutError` arm of `set_ipc_buffer`.
- Observed on `aarch64-sel4-roottask` (thread-local):
  `SLIME_ROOT FATAL console thread: no PT_TLS segment among 5 headers`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | Thread starts, `SLIME_ROOT console dispatcher started`, graph continues behind it | Scheduling, stack, TCB, and priority are right |
| 2 | First invocation panics in `set_ipc_buffer` | Per-thread runtime state is the problem, not the IPC |
| 3 | `-minimal` sets no `has-thread-local`; the slot is one global | A second thread has nowhere of its own to register |
| 4 | `aarch64-sel4-roottask.json` differs from `-minimal` in exactly that key | Switching targets is a one-line, behaviour-neutral change |
| 5 | On the TLS target the panic is gone; `Caught cap fault` instead | Progress — the buffer registers |
| 6 | `tcb_configure` had a zero CSpace guard; computed it from `initThreadCNodeSizeBits` (`guard_bits=52`, endpoint `0x418`) | Both correct, still faults |
| 7 | `sel4-initialize-tls::TlsImage::with_initialize_on_stack` is public — an earlier note claiming no API existed was wrong | The TLS block is installable |
| 8 | Wired up, it reports `no PT_TLS segment among 5 headers`; the built ELF has six and `PT_TLS` is one of them | The loader loses it |
| 9 | `add-payload` copies only `PT_LOAD` (`utils.rs:29`) and `PayloadInfo::user_image` has no TLS fields | The payload format cannot carry it |
| 10 | Back on `-minimal`: `single-threaded` is *not* enabled, so the token is a `SyncToken` two threads could share | Retry justified |
| 11 | Still `BorrowMutError`: `recv_with_mrs` holds the slot borrowed *across* the blocking syscall (`syscalls.rs:151`) | Structural — a parked receiver never releases it |

## Root cause

Two distinct obstructions, one per target, both in the vendored runtime:

**Thread-local target.** Each thread would get its own IPC-buffer slot, which
is what a second dispatcher needs. But the slot is reached through `tpidr`, and
a thread needs a TLS block for that. `sel4-initialize-tls` can build one from
the ELF's `PT_TLS` header — and the root task's running image does not have
that header. `sel4-kernel-loader-add-payload` copies only `PT_LOAD` segments,
correctly, since a `PT_TLS` segment is not separately loadable; the gap is that
`PayloadInfo::user_image` carries region bounds, entry, and offset with no
field for the TLS image's vaddr, filesz, memsz, and align. The data is in the
image; the description of it is not.

**Non-thread-local target.** The slot is one global, guarded by a `SyncToken`
that two threads *can* take in turn — `single-threaded` is not enabled, so this
is not the `UnsyncToken` case. It fails anyway because `recv_with_mrs` takes
the borrow and holds it for the duration of `seL4_Recv`. The main dispatcher
spends nearly all its time parked in exactly that call, so the borrow is
effectively never available.

## Changes

None. The experiment was reverted in full, including a target pin that was
briefly committed and then reverted: `has-thread-local` is precisely what
creates the TLS requirement, so keeping it would have left a dependency change
that helps nothing and moves the ground under the other route.

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_boot_check` after full revert | Pass | Direct |
| `just sel4_spawn_check`, `sel4_supervision_check`, `sel4_dango_check`, `sel4_input_check`, `sel4_transfer_check` | Pass | Direct |
| `just test_sel4_root` | 142/142 | Direct |
| `just test_host` | 7 suites | Direct |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just devlog_check` | Pass | Direct |

## Decisions

- **Decision:** stop at the vendor boundary rather than patch `deps/rust-sel4`.
  **Rationale:** both fixes are real and neither is large in isolation — a
  payload-format field, or a narrower borrow around the receive syscall — but
  both change a pinned dependency's behaviour or wire format, and the pin
  exists so that the kernel, toolchain, and target are reproducible. Changing
  it is a decision about the project's dependency posture, not a coding step.

- **Decision:** record the whole experiment rather than only its conclusion.
  **Rationale:** three separate walls were hit, and two of my own earlier notes
  about this blocker were wrong — first that eleven statics made it hard, then
  that no TLS API existed. Both were corrected by looking. The failing
  addresses and line numbers are here so the next attempt starts from evidence
  rather than from a summary of a summary.

## Open risks and follow-ups

- [ ] B41: `DebugWrite` and `InputRead` remain universal labels. The console
      endpoint *is* provisioned per process, write-only, and covered by the
      B40 CSpace audit — only the receiver is missing.
- [ ] B43: `BlockTransact` and `StoreTransact` likewise. All six of its named
      gates pass, but its first clause needs the dedicated endpoints.
- [ ] B44, B45: same second-dispatcher dependency.
- [ ] Either vendor change would unblock all four at once, which is the
      argument for doing it deliberately rather than incidentally.

## Artifacts and provenance

- Vendored sources read: `sel4/src/state/mod.rs` (slot declaration, feature
  cfg), `sel4/src/state/token.rs` (`SyncToken`, `UnsyncToken`),
  `sel4/src/syscalls.rs:151` (`recv_with_mrs` borrow),
  `sel4-kernel-loader/add-payload/src/utils.rs:29` (segment filter),
  `sel4-kernel-loader/payload-types/src/lib.rs:56` (`UserImageInfo`).
- Related roadmap items: `roadmap/00-backlog.md` B41, B43, B44, B45.
