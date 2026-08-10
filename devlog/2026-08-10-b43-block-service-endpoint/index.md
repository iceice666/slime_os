# B43 — block requests answered where the devices live

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{console,device,ipc,main,transfer_window}.rs`, `components/runtime/src/syscall{,.rs}/sel4_transport.rs`, `components/bins/`, `scripts/check/check-sel4-{component-graph,transfer-plane}.py` |
| Roadmap | B43 |
| Gates | `just sel4_device_check`, `just sel4_storage_check`, `just sel4_store_check`, `just sel4_rollback_check`, `just sel4_recovery_plane_check`, `just sel4_transfer_check`, `just sel4_component_graph_check` |
| Trigger | B43's first exit clause was false: `BlockTransact` and `StoreTransact` were still labels on the universal root dispatcher, so a block request needed no declared service capability. |
| Baseline | All six named gates green since 2026-08-10, but passing against the universal dispatcher rather than a direct service path. |

## Summary

`BlockTransact` and `StoreTransact` are gone from `slime-root/src/ipc.rs::Operation`.
Block traffic is answered by the console thread on the per-process console
endpoint, labelled `ConsoleKind::BlockTransact`, and the block device tables
moved there with it. A component without a console capability has no path to a
device at all, which is what B43's first clause asks for. The two labels left
in opposite directions and for different reasons, described below. All six
named gates pass, plus ten more planes.

## Changes

- **`BlockTransact` moved to the console thread.** `serve_block_transact`, the
  `RIGHT_BLOCK_READ`/`RIGHT_BLOCK_WRITE` constants, and the `SLIME_GRAPH block
  served` marker now live in `slime-root/src/console.rs`. `BlockDevices` and
  `MAX_BLOCK_DEVICES` moved from the binary into `slime-root/src/device.rs`,
  because a lib module cannot import from `main.rs` and because that is where
  they belonged.
- **The service loop no longer sees the devices.** `serve_instance_graph`'s
  `block_devices` parameter is now `#[cfg(slime_boot_selector)]`: only the
  selector variant's promotion path still needs it, and that variant launches
  no components and never constructs the console thread.
- **`StoreTransact` deleted outright**, along with `slime_rt::store_transact`,
  its transport, and its only two clients (`storage-store-probe`,
  `filesystem-service`).
- **The staging helpers collapsed onto one chokepoint.**
  `transfer_window::with_window_mapped_in` takes an optional `&mut IpcBuffer`
  and every reader and writer routes through it. `read_staged_array`,
  `read_staged_array_with`, `write_staged_region`, and the new
  `write_staged_region_with` are now four thin wrappers over two bodies.

## Regression guards

- `ipc::tests::no_console_operation_is_reachable_on_the_universal_abi` refuses
  a restored fallback: it checks all three retired labels by number and every
  reachable operation's variant name for `Debug`, `Input`, `Block`, or
  `Store`.
- `check-sel4-transfer-plane.py` now asserts exact multi-device selection —
  that requests reached both device 0 and device 1, that source reads
  succeeded, and that no write was ever served on the read-only source.
- `check-sel4-component-graph.py`'s `UNMEDIATED_OPERATIONS` list dropped
  `StoreTransact`, so a reintroduction fails that gate.

## Verification

| Check | Result |
|---|---|
| `just sel4_device_check` | pass |
| `just sel4_storage_check` | pass — "a userspace component read, wrote, flushed, and verified sectors on a real device through a capability its generation granted" |
| `just sel4_store_check` | pass |
| `just sel4_rollback_check` | pass |
| `just sel4_recovery_plane_check` | pass |
| `just sel4_transfer_check` | pass — "both devices came up from one shared granule and each answered under its own index" |
| `just sel4_component_graph_check` | pass |
| `sel4_boot_check`, `sel4_root_boot_check`, `sel4_input_check`, `sel4_dango_check`, `sel4_capability_layout_check`, `sel4_spawn_check`, `sel4_supervision_check`, `sel4_reclamation_check`, `sel4_directory_check` | pass |
| `cargo test -p slime-root --lib` | 143 passed |
| `just test_host` | 7 suites ok |
| `just contracts_check`, `just generation_check` | pass |
| `just lint_all`, `just fmt_check_all`, `just ruff`, `just typos`, `just machete` | clean |

Two mutations were run rather than assumed:

- Pinning the device index to `0` in `serve_block_transact` — the transfer
  plane failed to complete, so the new selection assertion is load-bearing.
- Reintroducing `DebugWrite = 34` with its `from_label` arm — the control test
  refused it.

## Decisions

**The device tables moved with the handler, not a reference to them.**
Whoever answers block requests *is* the driver. Leaving `BlockDevices` with the
main dispatcher and passing a borrow would have split the authority across two
threads for no benefit, and would have needed a lock the rest of the root does
not have. The console thread owns them outright.

**`BlockTransact` shares the console endpoint rather than getting its own.**
A separate block endpoint would need a second blocking receive and therefore a
third thread. The console endpoint already carries a Call kind (`InputRead`)
with reply authority, and the badge already identifies the sender per process,
so a third label costs nothing. The name "console" is now slightly narrow for
what the thread does — it is the root's *second dispatcher* — but renaming it
would churn every file B41 touched for no behavioural gain.

**`StoreTransact` was deleted, not migrated.** It never had a handler: it
answered `UnsupportedOperation` from `Mediation::Unavailable`, so it was ABI
surface for an operation the root does not perform. A durable store is
userspace policy built over block authority — `sel4-store-probe` and
`sel4-filesystem-service` already do exactly that — so there was nothing for it
to become. Its two remaining clients predated the seL4 cutover, appeared in no
seL4 manifest, and called an operation that could only fail; deleting them is
the clean cutover rather than a scope increase.

**Retired labels stay holes.** Labels 6, 7, and 17 are refused rather than
renumbered, as label 5 established with B41. A component built against the old
ABI is refused; renumbering would have it silently invoke whichever operation
moved into the slot.

**One chokepoint instead of parallel `_with` twins.** B41 added
`read_staged_array_with` beside `read_staged_array` by copying the body. Doing
that again for the region writer would have made three copies of the same
map/copy/unmap. Threading `Option<&mut IpcBuffer>` through
`with_window_mapped` removes the duplication instead of extending it.

## Open risks and follow-ups

- The transfer plane's source-read assertion is "at least two succeeded"
  rather than an exact count, because the probe's capacity search reads past
  each device's end on purpose. An exact count would be a stronger claim but
  would pin the search's iteration count, which is a QEMU disk-size accident.
- The second dispatcher is still named `console` while serving three unrelated
  kinds. If B44/B45 add more, renaming it to something like `service2` or
  splitting by concern becomes worth the churn.

## Artifacts and provenance

- Commits: `47c3811` (the cutover), `054084c` (the selection assertion).
- Sixteen plane gates observed green in this session; `sel4_powerbox_check`
  remains red and was verified inherited by stashing this work and re-running
  it.
