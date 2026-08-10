# Backlog (defects and unmasked debt)

**Purpose:** Track concrete defects, regressions, and latent bugs found in
implemented code that must be resolved before starting new roadmap-track
milestones. Backlog items are not new capability; they restore an already
claimed exit condition or remove debt that would compound under new work.

**Priority:** Backlog items are handled before roadmap-track milestones. A green
verification suite is a precondition for milestone work, not a milestone itself.
Clear or explicitly defer every open item here before opening a new track gate.

**Entry shape:** Each item states the problem, the evidence (how it was
observed), the proposed fix, and the exit condition that closes it. Close an
item only when its exit condition is observed, then move it to the resolved log
at the bottom rather than deleting it.

## Open

The native-capability-model cutover is tracked as ordered unmasked-debt work,
not as a new roadmap track. B39 establishes the authenticated contract; B40
establishes the CSpace substrate; B41–B45 remove the universal root dispatcher
one service slice at a time; B46–B49 replace the remaining compatibility
mechanism; and B50 deletes the dual-model residue. Each item is a clean cutover:
its old ABI and fallback are removed in the same change that makes its exit
condition observable.

### B41 — console and debug traffic still enters the universal root dispatcher

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B40.

**Problem:** `DebugWrite` and console/input-adjacent control share the same
badged root endpoint and dispatcher as lifecycle, storage, and fabric traffic.
A noisy client therefore consumes the highest-priority root service loop and a
console defect shares the system-wide dispatcher fault domain.

**Evidence:** `slime-root/src/ipc.rs::Operation` includes `DebugWrite` and
`InputRead`; components call the matching wrappers in
`components/runtime/src/syscall.rs` rather than a declared console/input
service endpoint.

**Fix:** Provision dedicated console/debug and input service endpoints through
generation v5 and cut clients to direct `Call`/`ReplyRecv` or one-way endpoint
traffic as appropriate. Remove the migrated operation labels, root handlers,
and runtime fallback in the same change.

**Exit condition:** `DebugWrite` and `InputRead` are absent from the universal
root operation ABI; capability denial is enforced by missing service CPtrs;
console output, refused input, scripted Dango input, and root boot diagnostics
pass `just sel4_root_boot_check`, `just sel4_input_check`, and `just
sel4_dango_check`. A gate-control mutation that restores a root fallback fails.

**Half landed 2026-08-10.** Every process now has a console/debug endpoint
object of its own: the plan declares it as a `SERVICE_CONSOLE` service binding,
the decoder resolves it into `ChildSlotPlan`, and the root mints it into each
child at construction. Write-only on both sides of the trust boundary, since
every process shares one console dispatcher and a receiver could dequeue
another's output. The slot is 32, above every slot a generation grant can name
— grant slots are the component's own numbering and start at 0, so a low fixed
slot collided with declared authority in every migrated fixture, which the B40
CSpace audit caught immediately. Child CNodes are six bits.

**Remaining, and why it is not landed:** nothing receives on the new endpoint.
The root has one blocking dispatcher (`ipc::recv_request(endpoint)` at
`slime-root/src/main.rs`), so routing `debug_write` there was tried and hangs
every component on its first line — reverted. Closing this needs a second
dispatcher. Two of the three obstacles turn out to be small; the third is a
real design decision:

- **Scratch page — solved by construction.** `with_window_mapped` maps a
  caller's frame into the root's own VSpace at `ScratchPage::addr()`, so two
  threads sharing one scratch address would collide. But scratch pages are just
  claimed root-image pages and the root already claims four (`FREE_PAGE`, two
  `FOUNDATION_PAGES`, `DEVICE_PAGE`). A console thread claims its own.
- **The statics — mostly not shared.** Of the eleven in `main.rs`, the console
  path touches none: it needs its endpoint, its scratch page, and the window
  table. The blanket "root task is single-threaded" comment overstates the
  coupling.
- **`WindowTable` — the actual blocker.** `DebugWrite` resolves the caller's
  window through `WindowTable::bound` (`&self`), while the main dispatcher
  mutates the same table at five sites (`declare`, `release`). One concurrent
  reader against five writers needs a stated synchronization contract — whether
  the console thread gets its own view, the table becomes lock-protected, or
  window binding moves behind the console endpoint too. That choice is the work,
  and it should be made deliberately rather than improvised.

A bound notification does not substitute: it signals, and the console still
needs its own receive to carry the payload.

Until then `DebugWrite` and `InputRead` remain on the universal ABI, so the
exit condition is unmet — the endpoint exists and is enforced, but nothing
uses it yet.

**Gates unblocked 2026-08-10.** Both gates this exit condition names now pass, along
with `just sel4_directory_check` and `just sel4_storage_check`, which shared
one of the same causes. B41's own work — moving `DebugWrite` and `InputRead`
off the universal root dispatcher — has not started.

The three causes, none in the behaviour the gates test:

- **The run token was undeclared.** Every probe plane's claim is that
  generation-declared device authority alone does not run a probe; only the
  instance `init` hands a run token proceeds. That token crosses a spawn
  boundary, so it is a `MintedBinding`, and it was not declared — preflight
  refused the spawn outright.
- **The idle instance was undeclared.** The claim needs two instances of one
  executable: the token-holding one and a root-launched copy that parks. Only
  the first was declared, so `[<probe>] idle without a run token` could never
  appear. `SLIME_GRAPH declared placed`, asserted by three gates, had no
  emitter anywhere in the tree; it now comes from the root's self-loop install
  path, the only point at which a child's own declared authority is placed.
- **A received capability could land in slot 0.** The receive path allocated
  from `free_slot_from(0)`, that slot number is reported to the receiver, and
  every protocol carrying one reads 0 as "no capability". A forwarded
  capability landing there was invisible to its new holder, which is what made
  dango's composed launch fail validation with both capabilities transferred.

Records: [`devlog/2026-08-10-b41-dango-plane-declarations/`](../devlog/2026-08-10-b41-dango-plane-declarations/index.md)
and [`devlog/2026-08-10-probe-plane-run-tokens/`](../devlog/2026-08-10-probe-plane-run-tokens/index.md).

**Historical — the two reds as first observed:**

- `just sel4_dango_check` dies in `components/bins/build.rs:272`,
  `expect("command RPC binding")` — `related_binding_slot` finds no send/recv
  grant between the command launcher and its profile instance in
  `contracts/generation/v1/fixtures/valid.zti`. Confirmed inherited by running
  the gate with `valid.zti` restored from `3228eb6`: identical failure. The
  file's only session change is two additive fields (`bootAction`,
  `mintedBindings = []`), and `build.rs` last changed in `c489edf`.
- `just sel4_input_check` exceeds its 180s bound without completing the plane.

`sel4-dango.zti` and `sel4-input.zti` are also among the fixtures B39 left
unmigrated. Resolve both reds before starting B41, so its own gate results mean
something; a green suite is a precondition for milestone work.

**Dango migration, partially landed 2026-08-10.** The fixture now declares what
`init` actually hands each child, and the plane gets four components spawned
and running where it previously could not build at all:

- The `dango`↔`spawn-service` RPC edge was never declared, though `dango`
  declares a `commandProfile` that can only be served over it. It is now a
  grant bound by both ends, and `init` no longer mints a second channel that
  would shadow it.
- The console channel stays runtime-minted, declared as two `mintedBindings`.
- `init` passed `spawn-service` its factories from a hardcoded slot 4, which
  this plane's layout assigns to the endpoint factory; both now come from the
  generated boot layout.
- Slots are ordered by provenance: what the parent passes occupies the lowest
  declared slots, since a spawn grant array is positional and the root ranks
  requests against declarations by destination slot.

**Build-script derivation, fixed 2026-08-10.** `components/bins/build.rs`
derived `RPC_SLOT` and `SHARED_BUFFER_FACTORY_SLOT` from the instance owning
the command profile, while `spawn-service.rs` — the only consumer of the
generated `command_profile.rs` — resolves both in its own CSpace. Those
coincide only while launcher and client share one slot numbering. The consumer
is now identified by which instance runs the spawn service, giving `RPC_SLOT`
2 and `SHARED_BUFFER_FACTORY_SLOT` 1 on the dango manifest, which is what that
fixture declares.

**Landed 2026-08-10.** The plane now runs end to end for the first launch:
the shell reaches its prompt, `$(sysinfo)` resolves through the profile,
`spawn-service` launches it, the child reports `result:exit:0`, and `init`
prints `dango plane complete` with every component exiting 0. What was
undeclared: `spawn-service`'s executables resolved in the wrong CSpace, the
dango RPC channel pre-created twice (declared edge shadowing init's minted
halves), the per-launch context endpoint, and the composed launch's forwarded
working directory and stdin. `sysinfo` and `echo-agent` are now owned by
`spawn-service`, which is what admits its exec bindings.

The gate asserted `spawn-request:accepted` before the child's own marker. The
child runs first by construction — `spawn` starts the thread, then the service
sends the launch context and only afterwards replies — so that ordering
asserted a race; the child's marker is required for presence, not position.

**`just sel4_dango_check` passes (2026-08-10).** The composed second launch was
refused because a received capability could land in slot 0: the receive path
allocated with `free_slot_from(0)`, that slot number is reported to the
receiver, and every protocol carrying one reads 0 as "no capability" — the
spawn request's `received_caps` among them. The forwarded working directory or
stdin endpoint was therefore invisible to the component it had just been given
to. Every other runtime slot allocation in the root already searched from 1.

**Superseded note — `init` holds two of the five
capabilities `spawn-service` declares — its factories — while the RPC end and
the two executables are root-installed, so spawn preflight's declared-count
rule refuses the spawn. The working `sel4` fixture avoids this by sourcing
those executables from `init`, which then holds and passes all five. Copying
that shape into `sel4-dango.zti` is refused by the decoder at
`grant_applies_to_instance`: `init` is root-owned, so it may not bind an `exec`
grant targeting an instance it does not own, and the `sel4` fixture's
equivalent grants target *executables* rather than instances. Closing this
means restating dango's executable grants against executables, which changes
which slots `spawn-service` and `dango` resolve and so needs their layouts
re-derived together rather than edited slot by slot.

Attempts to relax the preflight count rule instead were all refused by a
sibling plane: excluding pre-created channels breaks `sel4_component_graph_check`
(its `init` legitimately passes its own end of one), and excluding executables
breaks it the other way (its `spawn-service` receives five). The rule is right;
the dango fixture is what disagrees with it.

### B43 — block and durable-store clients still transact through root operation labels

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B40.

**Problem:** `BlockTransact`, `StoreTransact`, and recovery storage operations
share the universal dispatcher even though generation data can provision a
specific device or storage service. Root remains both IPC broker and driver
dispatcher, so unrelated clients share latency and failure scope.

**Evidence:** The labels remain in `slime-root/src/ipc.rs::Operation`; block,
store, rollback, recovery, and transfer components reach storage through
`components/runtime/src/syscall.rs` wrappers rather than a dedicated service
Endpoint installed in their CSpaces.

**Fix:** Provision explicit block-driver and durable-store service endpoints,
move each client to direct typed request/reply IPC, and preserve device/DMA
authority only in the owning service. Remove the root labels and handlers as
each storage slice cuts over; do not leave a universal-dispatch fallback.

**Exit condition:** Block/store requests cannot be issued without the declared
service capability; unrelated root operations make no progress on their behalf;
read-only device authority remains read-only; multi-device selection remains
exact. `just sel4_device_check`, `just sel4_storage_check`, `just
sel4_store_check`, `just sel4_rollback_check`, `just
sel4_recovery_plane_check`, and `just sel4_transfer_check` pass against the
direct service path.

**All six gates green (2026-08-10), but the exit condition is not met.** The
gates pass "against the direct service path" only in the sense that the paths
they exercise are correct; `BlockTransact` and `StoreTransact` are still labels
on the universal dispatcher (`slime-root/src/ipc.rs:99-100`), so the first
clause — a request cannot be issued without the declared service capability —
is still false. Moving them off shares B41's blocker: nothing receives on a
second endpoint until the `WindowTable` contract is settled.

What the gate work did establish, and what it found:

- **Block devices were not renumbered on the spawn path.** `declared_resource`
  answers `Block { device: 0 }` for every block grant because only the
  installer knows how many it has placed; the boot-graph path renumbers per
  binding and the spawn path's self-loop install did not. A component holding
  two device capabilities saw both resolve to device 0. The transfer plane is
  the only one holding two, and it read its manifest off the receiver instead
  of the source — sixteen sectors served `status=0`, every byte zero.
- **`SLIME_GRAPH block served` now carries the device index**, without which
  the record cannot say which of a plane's devices answered. That is precisely
  what multi-device selection claims, and it is what made the above
  diagnosable.
- Read-only device authority is verified byte-identical from the host
  (`source_before` in the transfer gate), and multi-device selection is now
  exact.

**Gate repairs (2026-08-10).** `sel4_device_check` and
`sel4_storage_check` already passed; `sel4_store_check`,
`sel4_rollback_check`, and `sel4_recovery_plane_check` were red for reasons
outside B43's scope and now pass. Each storage plane makes the same claim every
probe plane does — generation-declared device authority alone does not run a
probe, only the instance `init` hands a run token proceeds — and neither the
token nor the idle instance the claim compares against was declared. The
recovery probe additionally did not compile: `&mut [u8]` reaches the
`&mut [u8; N]` conversion only through a mutable borrow.

**`sel4_transfer_check` remains red, and not on declarations.** The probe reads
all sixteen manifest sectors successfully (`block served task=1 op=1 lba=1070
status=0 sectors=1`, sixteen times) and then decodes `declared=0` where the
same bytes read `1030` from the host. The manifest on disk is well-formed —
magic `SLIMETR\0`, `total_len` 1030 at offset 232, metadata and payload offsets
consistent — and the probe's slot layout, LBA constants, and `read_sector`
implementation all match the recovery probe's, which now works. What differs is
that this is the only plane reading through a *second* device capability, so
the sector payload is not reaching the caller's buffer on the source device's
path. The `SLIME_GRAPH block served` record carries no device index, which is
the first thing to fix in diagnosing it.

**B43's own work has not started:** `BlockTransact` and `StoreTransact` remain
labels on the universal dispatcher, and moving them off shares B41's blocker —
nothing receives on a second endpoint until the `WindowTable` contract is
settled.

### B44 — generation and recovery policy still crosses the universal root dispatcher

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B43.

**Problem:** Generation management and recovery are userspace policy, but
`GenerationTransact`, `GenerationReceive`, health, and reconstruction requests
still enter the universal root dispatcher. This leaves policy clients coupled
to root's global request ABI after B35 made the durable boot selector
authoritative.

**Evidence:** `slime-root/src/ipc.rs::Operation` retains generation and recovery
labels; the `sel4-generation-*`, rollback, recovery, and transfer components use
the root syscall transport even when an owning userspace manager exists.

**Fix:** Give the generation manager and recovery service dedicated endpoints
and the minimum block/BootState capabilities they need. Move typed requests to
those services, keep only irreducible boot-selector mechanism outside them, and
remove the universal operation labels and runtime wrappers.

**Exit condition:** Stage, inspect, select, rollback, recovery reconstruction,
transfer, and health promotion traverse declared service endpoints; a client
without those caps is denied by seL4 lookup, not a root-side resource table.
`just sel4_generation_check`, `just sel4_boot_selection_check`, `just
sel4_rollback_check`, `just sel4_recovery_plane_check`, and `just
sel4_transfer_check` pass with no dispatcher fallback.

### B45 — directory, filesystem, and store services still depend on universal root IPC

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B43.

**Problem:** Directory inspection/derivation/commit and filesystem/store
requests are service policy, yet their public path remains operation labels on
the root endpoint. Capability provenance is therefore checked in a global
software table rather than expressed by a client holding the service endpoint
and an attenuated directory or store capability.

**Evidence:** `DirectoryInspect`, `DirectoryDerive`, `DirectoryCommit`, and
`StoreTransact` remain in `slime-root/src/ipc.rs::Operation`; the directory,
filesystem, powerbox, Dango, and store components use the shared syscall ABI.

**Fix:** Provision dedicated directory, filesystem, and store endpoints with
Zutai request/reply contracts. Pass attenuated directory/store capabilities
through real CSpace bindings and seL4 transfer. Remove each root operation label
and wrapper as its service becomes direct.

**Exit condition:** Directory derivation and filesystem/store access succeed
only through declared service capabilities; attenuation, provenance, malformed
requests, and service death remain observable. `just sel4_directory_check`,
`just sel4_filesystem_check`, `just sel4_store_check`, `just
sel4_powerbox_check`, and `just sel4_dango_check` pass with the corresponding
root labels absent.

### B46 — logical ChannelTable, Transit, ParkedReplies, and WaitSet duplicate seL4 IPC

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:**
B39–B45.

**Problem:** Slime channels are root-owned queues with userspace-managed
blocking, wait sets, reply slots, peer death, and up-to-four-cap transit. Every
message crosses root twice and `slime-root` re-proves atomicity and lifetime
properties already supplied by Endpoints, Reply objects, and Notifications.

**Evidence:** `slime-root/src/channel.rs`, `transit.rs`, and `parked.rs` own the
compatibility mechanism; `Send`, `Recv`, `Wait`, `EndpointCreate`,
`CapTransfer`, and `TransferWindowBind` remain universal root operations.

**Fix:** Cut synchronous RPC to Endpoint `Call`/`ReplyRecv`, rendezvous messages
to Endpoint send/receive, and buffered asynchronous streams to a new
`contracts/fabric-stream/v2/` shared-ring contract with Notification badge bits
for availability and credit. Use real seL4 cap transfer, at most one capability
per IPC message; make bundle provisioning an explicit typed transaction. Delete
the logical channel, transit, parked-reply, and wait-set implementations in the
same cutover.

**Exit condition:** `channel.rs`, `transit.rs`, `parked.rs`, `WaitSet`, and the
migrated universal labels no longer exist. Backpressure, bounded queues,
timeouts, peer death, cap-transfer attenuation, unrelated-route progress, and
buffered-stream recovery pass `just sel4_channel_check`, `just
sel4_crossing_check`, `just sel4_stream_check`, `just sel4_qos_check`, `just
sel4_call_check`, `just sel4_operation_check`, and `just
sel4_visibility_check` on native Endpoint/Notification paths.

### B47 — package, process, thread, service instance, and lifecycle are one Task model

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B42,
B46.

**Problem:** One `Task` currently means image instance, CSpace/VSpace owner,
single TCB, service identity, scheduling unit, and lifecycle identity. This
forces single-threaded components and makes lifecycle and scheduling policy
ambient task-ID concerns rather than capability-owned process/thread state.

**Evidence:** `components/runtime/src/runtime.rs` assumes one thread;
`slime-root/src/task.rs::create` allocates one CNode and one TCB and returns a
`TaskId`; the public spawn ABI mirrors that identity.

**Fix:** Split package/image, service template, process, thread, service
instance, and lifecycle handle in generation v5 and root mechanism. A process
owns CSpace/VSpace; each thread owns TCB, IPC buffer, fault endpoint, and
scheduling state; service endpoints and lifecycle capabilities are separately
delegable. Remove `TaskId` from every cross-process contract.

**Exit condition:** A fixture can declare two threads in one process without
duplicating its CSpace/VSpace; one thread fault is reported under the declared
fault policy; lifecycle authority remains capability-based; single-threaded
graphs retain behavior. `just test_sel4_root`, `just sel4_spawn_check`, `just
sel4_supervision_check`, `just sel4_reclamation_check`, and `just
sel4_boot_check` pass.

### B48 — all child execution shares one fixed priority and no scheduling authority

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** B47.

**Problem:** Every child uses `CHILD_PRIORITY = 254`, root runs above it, and
`KernelIsMCS` is disabled. Generation policy cannot bound CPU budget or period,
differentiate service priorities, donate scheduling context to passive servers,
or bind timeout faults per thread.

**Evidence:** `slime-root/src/task.rs` defines one child priority and
`sel4/config/qemu-arm-virt.cmake` disables MCS. Generation v4 has no schedule
record.

**Fix:** First remove the all-services-one-priority fallback by applying v5's
declared per-thread priorities. Then resolve the assurance gate and enable MCS,
install per-thread scheduling contexts and timeout endpoints, and use scheduling
context donation for passive RPC servers. If MCS cannot be admitted, explicitly
defer only the MCS half with recorded assurance evidence; do not restore one
maximal child priority.

**Exit condition:** Priority, budget, and period are authenticated generation
data and observed in the running graph; one budget-exhausting client cannot
starve an unrelated higher-criticality service; timeout faults reach the
declared handler. `just sel4_qos_check`, the platform-timer assertions in `just
sel4_root_boot_check`, and the full direct-IPC graph pass under the selected
scheduling configuration, with a recorded MCS assurance decision.

### B49 — resource ceilings are reactive tables rather than an admitted object budget

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:**
B39–B48.

**Problem:** Static table constants bound tasks, capabilities, channels,
transit, scopes, and graph iterations according to the largest graph seen so
far. The generation cannot prove before activation that its TCBs, CNodes,
CSlots, endpoints, notifications, frames, mappings, IRQs, untyped size classes,
and dynamic reserves fit.

**Evidence:** `MAX_TASKS`, `MAX_TASK_CAPS`, `MAX_CHANNELS`, `MAX_TRANSIT`, and
related comments describe ceilings raised for prior test graphs; generation v4
contains partial fabric/shared-buffer limits but no complete seL4 object plan.

**Fix:** Compute exact static requirements and bounded dynamic reserves from
generation v5 during construction. Admission fails closed before any task
activates when the plan is unsatisfiable. Dynamic factories consume and release
delegated quota capabilities; remove compatibility-table watermarks that no
longer own mechanism.

**Exit condition:** A QEMU stress graph at the admitted ceiling boots and stays
bounded; the same graph one object, slot, mapping, IRQ, or untyped size class
over is rejected before activation. Observed live and reclaimed counts match the
plan through clean exit, fault, and construction unwind. `just
contracts_check`, `just generation_check`, `just sel4_reclamation_check`, and
`just sel4_boot_check` pass.

### B50 — the logical capability and universal syscall compatibility model remains deletable residue

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:**
B39–B49.

**Problem:** Even after native replacements land, leaving `GraphTables` as an
authority database, the universal `Operation` dispatcher, public task IDs,
generic cross-kind `u64` rights, name-only grants, or compile-time plane flags
would preserve two competing authority/IPC models and invite fallback drift.

**Evidence:** The handoff identifies these as the retained custom-kernel model
implemented above seL4. B39–B49 each have a narrower removal boundary; this
item is the final repository-wide proof that no compatibility path survived.

**Fix:** Delete the global logical authority database, universal operation ABI,
public task identity, generic rights vocabulary where seL4 cap rights or typed
policy now apply, name-only grants, fixed-slot constants, and all product graph
selection flags. Remove obsolete tests, fixtures, comments, and generated
bindings rather than aliasing them.

**Exit condition:** Exact-source guards find no deleted model symbols or build
flags; every surviving syscall is either a direct seL4 primitive or a narrowly
owned root mechanism with a declared v5 capability; every fixture uses v5.
`just test_sel4_root`, `just contracts_check`, `just generation_check`, all
affected `just sel4_*_check` targets, `just sel4_gate_control_check`, `just
fmt_check_all`, and `just lint_all` pass after the deletion.



## Resolved
### B42 — spawn and lifecycle control use ambient task IDs and the universal dispatcher

**Status:** Resolved 2026-08-10.

**Problem:** `spawn` returned both a numeric `task_id` and a supervision slot,
and the spawn protocol sent that number across a process boundary to wait for
termination. A numeric task id is not authority — it is a name anyone can forge
by counting — so lifecycle identity was ambient.

**Exit condition observed:** no Zutai wire record or public runtime type
exposes a bare task id. `task_id` is gone from `contracts/spawn/v1/schema.zt`,
from the generated `WireSpawnReply`, and from `slime_rt::Spawned`, with no
compatibility shim. The spawn service keys its live table on the supervision
slot it handed back, and dango waits on the handle it holds, so spawn, wait,
and health all work through the capability alone.

Handle coverage: derived (attenuated), transferred, parked in transit across a
sweep, retained across the crossing, and — added here — stale. Collecting an
outcome consumes the record, and the same handle then refuses rather than
answering twice from a stale table, which is the distinction a reusable number
could not make.

`just sel4_spawn_check`, `just sel4_supervision_check`, `just
sel4_reclamation_check`, and `just sel4_dango_check` all pass.
`scripts/check/check-lifecycle-identity.py`, wired into `just contracts_check`,
refuses the reintroduction: it matches task-id-shaped *declarations* in
schemas, generated protocol Rust, and the runtime's public surface. A comment
explaining the ban is not a breach, and the root's own in-memory `TaskId` is
out of scope because it crosses no boundary. Verified by reinstating `task_id`
in the spawn schema and observing the refusal.

**Gate repairs this required.** B34 renamed the markers reporting the
executable/instance split and ten plane gates still asserted `components=N`,
failing on the first marker so everything behind it went untested. Three
assertions could never have matched — spliced prose in the marker text, a
field the staged record never had, and a frozen `activated` count that only
ever covered root-launched instances. Two markers had no emitter at all:
`factory placed` now comes from the boot graph's binding install, and
`channel copied` from the parent's channel-end copy. `spawned … channels=N`
counted only generation-declared re-installs and now counts minted ends too.

Closure record: [`devlog/2026-08-10-b42-lifecycle-identity/`](../devlog/2026-08-10-b42-lifecycle-identity/index.md).

### B40 — child CSpaces are fixed four-slot shells rather than admitted authority

**Status:** Resolved 2026-08-10.

**Problem:** Every child CNode had four slots — null, root service endpoint,
own TCB, and fault endpoint — with those slots compiled in, while actual
authority stayed in a root-side `CapabilityTable`. The v5 plan already declared
each process's CNode size, its own TCB and fault bindings, and its service
binding, and the root ignored all of it, so the kernel could not enforce the
declared layout.

**Exit condition observed:** `just sel4_capability_layout_check` boots the
twenty-instance graph and requires every child's CSpace to match the admitted
plan, then rebuilds the root once per injected mutation and requires each to be
refused — missing, extra, wrong type, wrong slot, aliased, and wrong rights. A
mutation that still boots is the gate's failure condition. `just
sel4_boot_check`, `just sel4_root_boot_check`, `just sel4_component_graph_check`,
`just sel4_reclamation_check`, `just contracts_check`, `just generation_check`,
and `just test_sel4_root` (140) all pass on the same layout.

**What the kernel can be asked.** seL4 exposes no "read this slot", so each
property needed its own probe and one could not be answered at the slot at all.
Occupancy is a self-`Move`: `ensureEmptySlot` runs before the source lookup
(`deps/seL4/src/object/cnode.c:93`), so occupied answers `DeleteFirst`, empty
answers `FailedLookup`, and neither mutates. Type is a `tcb_suspend` on a
root-side copy, refused with `InvalidCapability` for any non-TCB. Rights and
identity are *not* observable — `maskCapRights` masks silently and never
reports back — so both are checked at `InstallLedger::record`, the single
chokepoint every child install passes through.

**Service-slot pin.** Making the slot plan-driven newly created drift against
`ROOT_SERVICE_SLOT`, the constant every component's runtime resolves the root
endpoint from: a plan naming another slot would build clean, admit clean, pass
an audit that validates against that same plan, and produce children whose
first syscall invokes an empty slot. The root (`ChildSlots::validate`) and the
host checker both pin it until the runtime reads the slot from the boot layout.

**Not covered.** The P5.1 fixture paths construct tasks outside any plan and
keep the four-slot shell, now passed explicitly as `ChildSlots::SHELL` rather
than inherited.

Closure record:
[`devlog/2026-08-10-b40-native-child-cspaces/`](../devlog/2026-08-10-b40-native-child-cspaces/index.md).

### B39 — Generation v5 must describe the exact seL4 object and authority plan

**Status:** Resolved 2026-08-10.

**Problem:** Generation v4 declared logical objects and grants that
`slime-root` reinterpreted, so it could not prove the process/thread topology,
kernel objects, mappings, CSpace bindings, scheduling policy, fault policy,
spawn templates, or dynamic reserve the admitted graph would consume. `init`
also selected its scenario graph through `SLIME_GENERATION_NUMBER` and
`SLIME_*_CHECK` build flags.

**Exit condition observed:** `just contracts_check` and `just generation_check`
pass, proving every binding and object reference resolves, every
authority-bearing grant maps to a planned capability or is explicitly deferred,
and two isolated builds are byte-identical. `just sel4_boot_check` passes: the
full graph — twenty declared instances across five routes, split into three
bounded route workers — comes to rest at the supervisor's terminal record with
every required instance parked and none completed or failed, selected only by
the generation's authenticated `bootAction`. No product code admits generation
v4: `MAGIC_V4` survives solely to reject it, with
`rejects_v4_product_generations` proving so.

**What the format gained.** Ten plan record types (`Process`, `Thread`,
`KernelObject`, `Mapping`, `CapBinding`, `ServiceBinding`, `Schedule`,
`FaultPolicy`, `SpawnTemplate`, `ResourceQuota`) plus two deferral records for
authority whose object does not exist until runtime: a `MintedBinding`, and a
`CapabilityGrant` marked `minted`. Both fix the edge, its endpoints, the
destination slot, and an exact rights ceiling before activation, deferring only
object identity — which is intrinsic, since the object's creator runs after
admission. A relationship needing identity pinned uses an ordinary grant
against a concrete object.

**Boot-graph selection.** The root delivers the authenticated `bootAction` in
the bootstrap thread's first C parameter and `init` composes from it before any
build flag is read. Every `SLIME_SEL4_*_CHECK` branch is gone; an unimplemented
action is a boot failure rather than a fallthrough. `init`'s copy of the action
numbering is pinned to the contract by a const-assert per variant.

Audit and closure record:
[`devlog/2026-08-10-b39-generation-v5-checker-cutover/`](../devlog/2026-08-10-b39-generation-v5-checker-cutover/index.md).

### B34 — generation component records conflate executable catalogue entries with initial instances

**Status:** Resolved 2026-08-10.

**Problem:** `slime-root` constructs and activates every loadable component in
the generation, while `init` also receives those executable capabilities and
spawns the graph it owns. The full C8.10 image therefore runs a root-launched
copy and an init-spawned copy of the same fabric, workers, and participants.
The first copy has no matching spawn-time composition: its fabric service is
refused when it tries to spawn its route workers, and that graph exits nonzero.
The generation format has one `Component` record for two different concepts —
an executable available to spawn and an initial instance that must exist at
boot — and has no launch-owner or autostart field with which to distinguish
them.

**Evidence:** `just sel4_boot_check` failed on 2026-08-09. Continuing the same
image past the checker's early terminal showed root-launched fabric task 16
report `spawn refused ... ungranted`, then the root-launched graph exited with
status 1; init task 19 subsequently transferred supervision and continued a
second graph. `slime-root/src/main.rs::launch_component_graph` walks every
`Admission::loadable_plans()` entry and activates them all.

**Fix:** Introduce a clean generation-format cutover separating
`Executable` records from `Instance` records. Initial instances explicitly
declare their executable, launch owner (`root` or another instance), autostart
state, dependency barrier, health policy, quota, and capability bindings. Root
launches only root-owned autostart instances; executable catalogue entries are
inert until an authorized spawn. Do not retain a runtime v1 compatibility shim.

**Exit condition observed:** A fixture can carry executable-only images without creating
tasks; every declared initial instance is constructed exactly once by its
declared owner; the full graph contains no duplicate component identities or
unintended nonzero exits; and `just sel4_boot_check` observes the single graph's
complete healthy-idle chain. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B35 — BootState does not select the generation the seL4 product boots

**Status:** Resolved 2026-08-10.

**Problem:** The generation admitted by `slime-root` is selected at build time
with `SLIME_GENERATION` and compiled into the root ELF through `include_bytes!`.
The generation-management, rollback, and recovery planes can correctly mutate
durable BootState sectors, but the next seL4 boot never reads those sectors to
choose which generation to launch. The generation also retains a required
`kernelObject` whose seL4 payload is an inert placeholder that is validated but
never loaded.

**Evidence:** `slime-root/build.rs` states that the root task admits generation
bytes compiled into it; `scripts/build/build-sel4.py::build_application` builds
a distinct root ELF per manifest. `just sel4_generation_check` proves authority
and disk transitions but boots an image that already embeds generation 27, so
it cannot prove that the committed selection controls a later boot.

**Fix:** Add a minimal immutable seL4 boot selector that reads the
explicitly granted boot device, selects and updates the two BootState slots,
verifies release/target/generation/object closure, and launches the selected
runtime generation. Move seL4 kernel, loader, and boot-selector identity into
the signed boot bundle or release record; remove the unused generation
`kernelObject` in the same format cutover.

**Exit condition observed:** One QEMU campaign stages a pending generation, reboots into
that exact generation, durably consumes failed attempts across fresh boots,
returns to known-good when exhausted, and promotes only after health
confirmation. Changing only the root build's embedded bytes cannot satisfy the
gate. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B36 — the full-graph gate stops at a non-unique component idle marker

**Status:** Resolved 2026-08-10.

**Problem:** `check-sel4-boot-plane.py` treats the generic fabric line
`[fabric] idle: parked on control endpoints` as the whole system's terminal
marker and terminates QEMU immediately. Any fabric instance can emit it. With
B34's duplicate graph, the checker stops on the wrong instance before init's
supervision transfer, later component exits, and the actual graph outcome.

**Evidence:** Both `just sel4_boot_check` and
`python3 scripts/check/check-sel4-boot-plane.py --no-build` exited 1 with the
same missing init marker immediately after the first fabric-idle line. Manually
continuing the identical image produced the missing init marker only after the
first graph had reported multiple status-1 exits.

**Fix:** Define one supervisor-emitted terminal record binding the
generation identity or instance-set digest, required/live/idle counts, and zero
failed instances. Collect serial until that record or a failure marker; treat
every required component's nonzero exit as failure. Extend gate-control
mutations with an early duplicate fabric-idle line and a later failed instance.

**Exit condition observed:** `just sel4_boot_check` reaches the unique supervisor terminal
only after every causal chain, fails on any required nonzero exit, and the gate
control proves that an injected early component-idle line cannot truncate or
pass the check. Closing B36 by hiding B34's duplicate graph is forbidden. Audit
record: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md).

### B37 — dependency activation and non-bootstrap slot ABI are implicit contracts

**Status:** Resolved 2026-08-10.

**Problem:** Generation dependencies are decoded and structurally validated but
the seL4 launch path does not consult them; root stages component-table order and
activates every task. Actual dependency barriers live as imperative spawn/yield
sequences in `init`. Bootstrap slots have an authenticated layout resource, but
other component slots are inferred from grant iteration order, making manifest
ordering an undocumented ABI shared by the builder, root, and binaries.

**Evidence:** `boot-contracts/src/generation.rs` validates dependency bounds and
self-reference, while `launch_component_graph` uses only
`Admission::loadable_plans()`. `slime-root/src/channel.rs` documents that
non-bootstrap channels and executables take positional slots; prior Dango and
powerbox fixes already found boot/spawn and multi-kind ordering disagreements.

**Fix:** Bind dependencies and capabilities to explicit instance
records. The builder rejects cycles and unsatisfied dependency barriers, emits a
fixture-checked per-instance capability layout, and generates each component's
startup bindings from that same data. Root activates the declared DAG rather
than component-table order; grant order grants no ABI meaning.

**Exit condition observed:** Cyclic, missing, and impossible dependencies fail the build;
permuting grant declarations leaves every component's local bindings unchanged;
boot and spawn use the same generated layout; and a QEMU graph proves activation
occurs only after each declared dependency barrier. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B38 — task reclamation cannot reuse root CSlots or untyped memory

**Status:** Resolved 2026-08-10.

**Problem:** `ObjectAllocator` advances root CSlots and ordinary-untyped
watermarks monotonically. Task cleanup revokes and deletes each task's
capabilities but records those slots only as reclaimed; it returns neither slot
indices nor the task's TCB, CNode, page tables, and frames to allocatable pools.
A long-running component manager can therefore exhaust boot-lifetime resources
through repeated bounded spawn/exit cycles even when simultaneous live usage
never exceeds its generation budget.

**Evidence:** `slime-root/src/object_allocator.rs` explicitly states that slots
are never reused, and `CleanupRecord::revoke` states root CSlots are not returned
to the allocator. seL4 resets an untyped cap's free index when it has no children,
but the root does not allocate tasks from reclaimable per-task untyped subtrees.

**Fix:** Give each task or task group a derived untyped arena that owns
its CNode, TCB, VSpace objects, and ordinary frames; revoke the arena on death so
the parent can be retyped again. Add a free-list or bitmap for emptied root
CSlots. Keep device untyped and DMA ownership on their separate monotonic path.

**Exit condition observed:** A live QEMU stress graph completes more spawn/exit cycles
than the current root CSlot and untyped watermarks permit, with bounded and
stable live slot/object/byte counts, no capability alias surviving reclamation,
and successful reuse after clean exit, fault, and construction unwind. Audit
record: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md).

### B33 — seL4 cutover review findings

**Status:** Resolved 2026-08-09.

**Was:** The post-cutover static review recorded CUT-001 through CUT-077 across
capability isolation, lifecycle cleanup, shared-memory aliases, storage,
userspace services, gate integrity, CI/profile policy, and project records.
Several defects were merge blockers, and several gates could pass without the
current artifact or without the evidence named by their target.

**Fix:** Every finding was re-grounded and repaired. Final integration also
separated the capability-subset proof from the fabric control protocol and
dropped init's retained endpoint copies after spawn so peer-death retirement
can drain the QoS graph.

**Exit condition observed:** focused root and host tests pass; the supervision,
QoS, root-boot, gate-control, and layout-resource checks pass; formatting,
Clippy, Python lint, and dependency policy pass. See
[`devlog/2026-08-09-b33-cutover-review-remediation/`](../devlog/2026-08-09-b33-cutover-review-remediation/index.md).

### B31 — six oracle properties blocked `kernel/` deletion

**Status:** Resolved 2026-08-09.

**Was:** Two deletion audits found six acceptance properties that would have
disappeared with the frozen custom-kernel oracle, plus orchestration coupling in
the workspace, Justfile, check scripts, component transport, CI, and generation
builder.

**Resolution:** P5.4.final records each disposition. Complete component-wrapper
admission moved to `boot-contracts`; the seL4 root boot gate now observes
independent frame accounting, exact task and shared-buffer reclamation, clean
exit beside deliberate fault isolation, and panic/fault failure markers; the
global gate control proves missing, reordered, or contradictory evidence turns
every seL4 plane red. Free-frame reuse, custom EL1 mechanism, and
PMM/VMM/heap/APIC internals were reclassified where seL4 changes the mechanism.
The retired NVMe QEMU path was not promoted into false product evidence:
`storage_nvme_read_check` fails closed and M5.7 remains blocked on a seL4 NVMe
driver plus physical Framework observation.

**Exit condition observed:** `kernel/`, its workspace membership, custom-kernel
build and check orchestration, legacy component syscall transport, and custom
generation-builder path are removed together. The surviving repository gates
exercise the seL4 product or portable host contracts. See
[`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md).

### B32 — three scenario receive spins were invisible to the root

**Status:** Resolved 2026-08-09.

**Was:** The call plane's terminal receiver and two operation-plane receive
paths used `yield_now()` on `ERR_WOULDBLOCK`. `seL4_Yield` kept the components
runnable, so the root could neither name their endpoint wait nor distinguish a
real dependency from an iteration-budget spin.

**Fix:** All three now call `wait(&[WaitSource::Endpoint(...)])`. This is valid
for timeout and peer-death terminals: the brokers publish those records on the
same route endpoints. Parking exposed a pre-existing operation teardown race,
so client B now records its terminal before client A closes the backup route and
lets the broker exit. The backup probe likewise waits on the route it receives
from.

**Exit condition observed:** `just sel4_call_check` and
`just sel4_operation_check` both pass with every affected timeout, peer-death,
and unrelated-route marker present. See
[`devlog/2026-08-09-b32-parked-scenario-receivers/`](../devlog/2026-08-09-b32-parked-scenario-receivers/index.md).

### B29 — one block device per granule

**Status:** Resolved 2026-08-08.

**Was:** `slime-root` brought up at most one virtio block device. QEMU packs
eight virtio-mmio transports into one 4 KiB granule, so two attached disks land
at `0xa003e00` and `0xa003c00` — the same page — and `DeviceRegion::remap` maps
the frame to a driver's standing window, leaving nothing for the second.

**Fix:** `device::MappedGranule`, a borrowed view carrying the virtual base and
no capability. One owner maps the page; a second driver reads and writes its
registers at its own offset through the borrow, and can neither remap nor
unmap. `probe_devices` keeps a standing-granule table and `bring_up_shared_block`
brings up a transport in a page another driver already stands in.

**Exit condition observed:** `just sel4_transfer_check` boots with two disks and
records `SLIME_ROOT block ready` for both, with a component holding one
capability over each and the read-only one byte-identical afterwards.

Two further defects surfaced on the way, both now fixed and gated:

* declared placement hardcoded `Block { device: 0 }`, so a component holding two
  devices reached the same one twice — successive block grants now name
  successive devices;
* placement intersected the component's *union* of rights rather than the
  grant's own, so a read-only source came out writable and accepted a write.
  Both paths now use the grant's rights, which is what "this grant declares this
  much" means.

See
[`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md).


### B30 — the dango plane launched no commands

**Status:** Resolved 2026-08-08.

**Was:** Dango booted, read its scripted keystrokes, and resolved commands, but
no launch reached the spawn service.

**Three causes, none of them the hypothesis recorded when this was opened.**

1. `construct_child` never placed a child's declared **executables**. A spawned
   `spawn-service` found slots 1 and 2 empty and refused every request with
   `slot=1 ungranted`. The same defect class as P5.4.2c's missing declared
   authority, in the one resource kind that slice did not cover.
2. Declared authority was placed in a **fixed kind order**, and two components
   disagreed about it: `powerbox-chooser.rs` reads a directory then input,
   `dango.rs` reads input then a cwd root. Both placement paths now walk the
   generation's own grant order, which is what the oracle does.
3. `Resource::is_transferable` refused **endpoints** by kind, so a shell could
   not give a child its stdin. The reasoning was wrong rather than narrow: what
   bounds every move on that path is the sender holding `RIGHT_TRANSFER`, and
   the oracle's `sys_send` gates on exactly that bit with no kind predicate.

**Exit condition observed:** `just sel4_dango_check` — 14 markers, 2 profile
resolutions, 2 accepted spawn requests, `resolve-denied`, `parse-error`, and
`[dango] interactive session closed`. See
[`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md).

### B25 — a spawn-granted endpoint moves on seL4 and copies on x86, so a parent cannot broker a later introduction

**Resolved 2026-08-08.** Devlog:
[`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md).
Endpoint authority now carries `Side`, so a spawn grant is the same non-consuming
narrowing copy as every other grant and `ChannelTable` no longer records a
single task holder per end. Capability transit binds to the receiving *side*,
not a task selected at send time; whichever co-holder dequeues the message may
collect it, while task-naming loan creation refuses an ambiguous receiver.
Observed exit condition: `just sel4_call_check` passes 50 markers across ten
causal chains, including three parent-vouched post-spawn supervision transfers,
all C8.6 outcomes, and clean exits for the five spawned tasks plus init.
All twelve seL4 plane gates were re-run, not only the call gate: the change
rewrote marker text four of them read. `sel4_channel_check`, `sel4_loan_check`,
and `sel4_crossing_check` were red and are fixed, and
`sel4_gate_control_check`'s spawn-plane pin correctly caught a deleted
distribution assertion that had not been replaced. The crossing gate also
surfaced a root defect: `ChannelTable::live_queues` counted entries no
capability table names, which the retired per-end task cache had masked.

**Problem:** `slime-root`'s `distribute_channel_ends` (`slime-root/src/main.rs`)
treats an endpoint named by a spawn grant as a **move**: it reassigns the
channel's holder to the child and calls `table.drop_slot` on the parent's slot.
The retired kernel copies: `preflight_spawn_grant`
(`kernel/src/task/mod.rs:286`) performs `cap.derive(grant.rights)` at `:320`
into a fresh vector that `spawn_with_caps_for` (`:402`) installs into the
child, and neither reads nor mutates the parent's table — so the parent keeps
its end.

That difference is invisible to every component that hands an end away and
never touches it again — which is every component in the nine passing planes —
and fatal to any composition where a parent grants one end at spawn and then
*uses* that channel itself. The x86 call plane is exactly such a composition:
`init.rs::launch_fabric_calls` spawns `fabric-service` with all four service
halves, keeps them, and afterwards moves each participant's supervision handle
to the broker with `cap_transfer` over the matching half.

**Not a slot-numbering defect.** Two earlier versions of this entry blamed
`SlotCursors::take`'s `used_slot_zero`, first as a slot *collision* and then as
a slot *gap*. The gap is real, but it was a consequence of declaring the
control channels as **generation grants** — the root then numbers a launched
component's ends from its own cursor, which resumes above the factory grants
staging installed. Having `init` mint the pairs and hand them out at spawn
removes it, because `construct_child` installs a child's grants at `0..count`
in the requested order. Observed with the pairs minted: the fabric's four
controls arrive as `channel handed parent=5 child=6 … slot=2,3,4,5`,
contiguous above the two factory grants at the head of its grant array.

The grants themselves stay in the manifest, which the first attempt at this got
wrong by deleting them. `_control_sources`
(`scripts/build/build-generation.py:833`) derives `FABRIC_CALL_CLIENTS` — the
table the broker maps a control slot to a caller identity with — from exactly
those four grant *names*, and in `FABRIC_CALL_CONTROL_GRANTS` order rather than
the builder's `(name, source, target)` sort. Removing them emptied the table
and tripped `request_response_controls`' four-control assert before the broker
read a slot. They are the naming source; the minted endpoints are the
authority.

**Evidence:** `devlog/2026-08-07-p5-4-6-call-spawn-semantics/`. With the plane
rebuilt to mint its control pairs, the boot reaches
`SLIME_GRAPH channel handed parent=5 child=6 key=4 slot=2` — the fabric's end
arriving *and* init's slot being dropped in one step. Every participant's role
request then reaches the broker (`SLIME_GRAPH received task=4 channel=2`) and
is never answered: `Broker::provision` blocks in `consume_supervision` awaiting
a handle no one on this plane can send, and the graph ends `live=10`,
`parked=8`, `transfers served=0`.

The obvious alternative — each participant sending a handle naming itself — is
not constructible. `serve_spawn` installs a supervision handle only into the
**parent's** table, and only after `construct_child` has built the child's
(`slime-root/src/main.rs:3586-3603`), so no component ever holds a handle
naming itself.

**Narrowed by experiment, 2026-08-07.** Inverting the call plane's spawn order
*does* carry the supervision handoff, so the endpoint-move semantics alone are
not the whole blocker. Spawning the participants first with the *participant*
half of each control pair, keeping the *service* half in init, transferring each
participant's handle over it, and spawning the fabric last with the service
halves reached `[init] call supervision delegated` — the step this entry was
filed for. Both halves of a pair are still granted exactly once, so no
`drop_slot` takes anything init needs later.

What that order cannot then deliver is the **fabric's own** handle, and for a
second, independent reason. Two participants lend to the broker
(`fabric_call_scenario`'s `send_large_request` and `send_large_reply`), so both
need a `RIGHT_SUPERVISE` capability naming the fabric at their
`FABRIC_SUPERVISION_SLOT`. A *spawn grant* copies (`preflight_spawn_grants`
installs `held.resource` and leaves the parent's slot), which is how
`drive_sample_plane` hands one handle to a lender — but it requires the fabric
to exist first, which is the order this experiment inverted. A *transfer* moves
(`serve_cap_transfer` calls `table.drop_slot` on the source), and
`FLAG_RETAIN_TRANSFER` keeps the delegation bit at the destination without
making the move a copy — so one handle reaches one receiver. Init cannot obtain
a second, because `bootstrap_executable_slot` resolves an executable by
component identity to exactly one slot and each spawn returns one handle.

So the two requirements are order-incompatible as the components are written:
the control ends want the fabric spawned last, the fabric handle wants it
spawned first. That is a sharper statement than "the grant moves", and it means
the fix is still a model decision rather than a composition detail. Observed
directly; the experiment was reverted and the tree is back to the committed
plane.

**Severity:** Latent for every current plane, and a hard blocker for any plane
whose parent must broker an introduction after spawning. It is a genuine
*semantic* divergence from the frozen oracle, not a numbering accident, so it
cannot be resolved by re-blessing a fixture.

**Proposed fix:** Decide which semantics the model wants and make both
implementations agree, rather than working around it per plane. A copy matches
the oracle and keeps `init.rs` portable, but it means two tasks name one
channel end and `ChannelTable` resolves queues by holder — so the copy needs a
holder model that admits more than one. A move is the cheaper invariant and is
arguably the more capability-honest one, but then the oracle's own call plane
is not portable as written and `launch_fabric_calls` needs restructuring.

The experiment above adds a third option, cheaper than either and worth
weighing first: let a component obtain a **second** handle naming a task it
already supervises, so a broker's handle can reach both of its lenders without
the fabric having to be spawned before the participants. The narrow form is a
`supervision_derive`-style operation returning a fresh capability naming the
same task, which is a copy of authority the caller already holds and widens
nothing. That would make the inverted order carry the whole plane, leaving the
endpoint move/copy question a real but no longer blocking difference.

**Third option implemented, 2026-08-07.** `supervision_derive` (operation 32)
exists and is gated. A caller holding `RIGHT_SUPERVISE` on a supervision handle
receives a second capability naming the same task at the same rights, in a fresh
slot, keeping the source. Root side is `serve_supervision_derive`
(`slime-root/src/main.rs`); the ABI is mirrored in both component transports.

It widens nothing by construction — same task, same rights, `RIGHT_SUPERVISE`
required to ask — so it cannot mint authority the caller could not already have
transferred. `graph::holds_supervision` already scanned every live table for *any*
holder, because a handle has always been movable, so reclamation needed no change.

**Observed on the supervision plane**, which is the one plane where init holds a
handle it has not yet given away:
`SLIME_GRAPH supervision derived task=0 child=3 slot=5`, then the *derived* handle
answers the child's outcome, then the source is still intact for the existing
transit transfer. Both markers are gated in
`check-sel4-supervision-plane.py`. Two fault injections confirmed: returning the
source slot instead of a new one, and installing the derived handle with no
rights, each trip a distinct component assertion. A third — dropping the
`RIGHT_SUPERVISE` gate — is **not** covered, because every caller on this plane
holds that right; recorded rather than claimed.

**This does not yet close B25**, and investigating the call plane afterwards found
that B25 is no longer what stops it.

**The supervision grant already works.** `launch_fabric_calls` grants
`service.supervision_slot` to *both* `fabric-call-client` and
`fabric-call-server` (`init.rs:841` and `:860` — the same slot, twice), and the
boot shows all five components spawning. So a *supervision* spawn grant copies:
`distribute_channel_ends`' move applies to channel ends only. B25's two blocking
reasons were both about supervision handles, and neither is what the plane hits.

**What the plane actually hits is a missing component, not a missing operation.**
The boot reaches `[init] call participants spawned` and then dies with
`[fabric-call] fail: time phase receive`. `fabric-call-time` waits on
`recv(1, …)` for a phase byte, and *nothing on this plane sends one*: only
`fabric_operation_scenario.rs` has a time-phase publisher (`PHASE_TIME_SLOT = 2`,
`send` at `:648`), while `fabric_call_scenario.rs` has only a **client** phase
channel (`CLIENT_PHASE_SLOT = 1`). There is no time-phase sender in the call
scenario at all.

A contributing defect was found and fixed-then-reverted along the way:
`init.rs` grants `FABRIC_CALL_PHASE_TIME_SLOT` to `fabric-call-time`, and that
constant is `SLOT_ABSENT` (`u32::MAX`) because `sel4-call.zti` declares no phase
grants. So the component was handed a slot naming nothing. Minting the pair in
`init` and granting the service half to the fabric was written, built, and booted
— and the plane *still* fails identically, because plumbing a channel does not
create the publisher that was never written. The change was reverted rather than
committed as a partial fix that changes no observable outcome.

**Fixed, and the failure is gone.** `fabric-call-time`'s own comment already said
"no phase channel in the boot layout" and it already had a `park_only` path for
exactly this — but the guard was `fabric_boot::active()`, which keys on
`SLIME_FABRIC_BOOT_CHECK`. The x86 boot generation sets that; the seL4 call plane
does not, so the component took the phase path on a plane with no phase publisher.

The component now also parks when `FABRIC_CALL_PHASE_TIME_SLOT == SLOT_ABSENT`,
read from the generated boot layout it already includes. Testing the *slot* rather
than adding a second flag is deliberate: the condition that matters is whether a
phase channel exists, the layout already answers that, and a flag would have to be
kept in step with every future generation.

Observed: `[fabric-call] fail: time phase receive` is gone and replaced by
`[fabric-call-time] boot idle without a role`. All eight other plane gates re-run
green.

**The plane still does not complete, and the remaining gap is now located exactly.**
It wedged with no component failure — `graph iterations exhausted live=11 parked=9`.
Tracing it:

* The broker is task 6. It received once on channel 4 and then went silent — it
  never replied and never parked, because
  `call_broker.rs::consume_supervision`'s `ERR_WOULDBLOCK` arm was `yield_now()`,
  which is `seL4_Yield` and invisible to the root. **Fixed:** that arm now parks
  with `wait(&[WaitSource::Endpoint(control)])`, matching `consume_request` in the
  same file and `operation_broker.rs::consume_supervision`, both of which already
  parked — this was the one arm that did not. The plane now reports
  `parked task=6 reason=wait` and reaches a genuine all-parked deadlock instead of
  burning the root's iteration budget, which is a strictly better failure: the
  root's accounting can name the waiter. All eight other plane gates re-run green.
* `consume_supervision` waits for a descriptor carrying a `RIGHT_SUPERVISE`
  handle naming the *participant*, on that participant's control channel.
* **Nothing on this plane sends one.** `drive_call_plane` (the seL4 path;
  `launch_fabric_calls` is the x86 one, keyed on a different flag) never calls
  `transfer_supervision` at all. Its own comment at `init.rs:1901` describes the
  intended cut — "each participant delivers its **own** handle over its own
  control channel, as its first act" — but the grants below it hand each
  participant a handle naming the **fabric** (`init.rs:1917`), never one naming
  itself. So the plan in the comment was never implemented, and it *cannot* be as
  written: `serve_spawn` installs a supervision handle only into the **parent's**
  table, so no component ever holds one naming itself.

**And `supervision_derive` does *not* close it**, which is worth stating plainly
after adding the operation: the derive copies a handle the **caller** holds, and
what is missing here is a handle naming the **participant itself**, held by that
participant. Init holds one naming each participant, but init has no channel left
to the fabric — the endpoint grant moved every service half away at spawn.

Traced to the exact shape the broker expects: `consume_request` then
`consume_supervision` on the **same** control channel
(`call_broker.rs:273-275`). The participant already holds that channel
bidirectionally (`RIGHT_SEND | RIGHT_RECV`, `init.rs:1915`) and sends the request
over it, so the channel is not the obstacle. The obstacle is that no component can
obtain a supervision capability naming *itself*: `serve_spawn` installs one only
into the parent's table.

**So the options narrow to two, and both are real design choices rather than
plumbing:**

1. Let a spawn place a self-naming supervision handle in the *child*, so a
   participant can present its own identity. That is a new authority shape —
   a component holding a handle to itself — and needs its own argument about what
   it permits (`supervision_status` on oneself, notably).
2. Keep an endpoint grant from moving for this one case, so init retains a service
   half and can deliver the derived handles itself. That is the original move/copy
   divergence, and it is where B25 started.

The derive is still the right operation to have — it is what makes option 2 a
two-line change once the endpoint question is settled, because init can then hand
the same participant handle to the fabric *and* keep its own for the termination
wait. But B25's core question is unavoidable, and it is a model decision the way
the entry always said.

**A third route was looked for and does not exist.** The obvious workaround is for
init to mint a *fifth* pair as a private delegation channel — grant one half to the
fabric, keep the other, and deliver the derived handles over it. That fails on a
stated constraint rather than a mechanism: the broker reads each participant's
supervision handle from `client_control[index]`
(`call_broker.rs:273-275`), the participant's *own* control channel, and
`init.rs:1907` records that `consume_supervision` "cannot tell the two paths
apart, which is what keeps the broker unmodified". Routing delegation over a
different channel means changing the broker, and an altered broker is no longer
the same composition the oracle's gate asserts — which is the property P5.4 exists
to preserve.

**Sizing the two real options, so the decision is informed. Re-examined
2026-08-08, and both earlier estimates were wrong in the same direction — they
priced the shallow form of option 1 and the shallow objection to option 2.**

* *Copying endpoint grant.* The earlier sizing — "change `producer`/`consumer`
  and every path that reads it, the widest change of the three" — prices only the
  **shallow** form, where `Entry` grows a holder list. That form is as bad as
  stated and worse: `mark_dead` (`channel.rs:378`) would have to become a
  refcount, matching what the oracle gets for free from
  `Arc::strong_count(&owner_alive) == 2` (`kernel/src/ipc/mod.rs:166`), and
  `peer` (`channel.rs:362-369`) would return a *set*, which `Transit`'s
  send-time receiver binding (`transit.rs:62-65`) cannot consume.

  There is a **deeper** form that is cheaper than either option, and it is the
  one to weigh: put the *side* in the capability —
  `Resource::Endpoint { channel, side }` — and resolve queues by side rather
  than by task. Then `distribute_channel_ends` is deleted outright rather than
  inverted, because a granted endpoint becomes an ordinary copy alongside every
  other kind (`preflight_spawn_grants:3207` already copies; endpoints are the
  one kind singled out for a move), and `Entry::producer`/`consumer` are deleted
  with it. That is not a new representation but the *removal* of one: the field
  doc at `channel.rs:552-556` already states these are "a cache of who holds
  each end, maintained by `ChannelTable::reassign` with no capability check of
  its own", and the only reason the cache exists is that the capability does not
  say which end it is. Holder questions then become table scans of the shape
  `channel::sweep` (`channel.rs:574`) already performs, bounded by
  `MAX_TASKS * MAX_TASK_CAPS` = 32 × 64 on cold paths.

  Two things this form must still answer, neither of them the representation:
  `Transit` binds an in-flight capability to a receiver *task* at send time and
  would bind to a *side* instead; and the declared self-edge
  (`check-sel4-channel-plane.py:113-116` — `queues=1`, "init holds both
  directions at one slot") needs a side that means *both*, because `materialize`
  installs exactly one slot for a loopback (`channel.rs:806`).

* *Self-naming supervision handle at spawn.* Mechanically small, as recorded.
  But the semantic objection recorded here — that it "makes `supervision_status`
  on oneself reachable" — is the weak one, and it is not what should decide.
  Asking one's own status can only ever answer `WouldBlock`, a task can already
  deadlock itself on a loopback channel, and `serve_buffer_loan` already refuses
  a self-loan at `main.rs:4853`.

  **The real objection is that it moves who vouches for an identity, and it
  degrades the oracle.** `consume_supervision` (`call_broker.rs:1146-1157`)
  checks magic, version, `object_kind`, `direction`, `rights_mask`, and
  `route_identity` — it cannot check *which task* the handle names. Today the
  broker is trusting the parent's introduction, because init is the sender. Under
  option 1 that stays true. Under option 2 the participant vouches for itself,
  and it is holding a second `RIGHT_SUPERVISE` handle naming the **fabric**
  (`init.rs:1917`), which satisfies every field the broker checks. A participant
  sending *that* one makes the broker treat the fabric as the loan receiver.
  `slime-root` happens to catch the result at `main.rs:4853` (`peer == id`);
  **the oracle does not** — neither `sys_shared_buffer_loan`
  (`kernel/src/syscall/mod.rs:786-799`) nor `SharedBufferTable::loan`
  (`kernel/src/memory/shared_buffer.rs:296-342`) compares lender against
  receiver. So option 2 opens a hole on the frozen side to unblock a plane on
  this one.

**And there is no "parent keeps a third end" route, for an arithmetic reason
worth stating so it is not re-attempted.** `endpoint_create` installs exactly
two slots (`main.rs:1790-1806`), and the first grant of a minted loopback always
moves the *consumer* side whichever slot named it (`channel.rs:426-435`). Three
holders of a two-slot pair is therefore unconstructible, and the x86 plane's
shape is exactly three: init's layout carries both
`fabric-call-client-control` and `...-control-service`
(`contracts/boot-layout/v1/fixtures/fabric-call.layout:53-56`), init grants the
client half at `init.rs:839`, sends the descriptor over the *same* slot at
`init.rs:883`, and only then drops it at `:884`. Whatever closes this must let
one end have two holders, or let a participant name itself — the two options
above and nothing else.

One further constraint on any re-transfer variant: the participants' executable
grants are declared `transferable = false` (`sel4-call.zti:95,102,109`), so the
supervision handle a participant spawn returns carries no `RIGHT_TRANSFER`
(`main.rs:3762`) and cannot be `cap_transfer`ed at all without a fixture change.

**Option 1 was built as a spike, 2026-08-08, and it gets further than the entry
expected before hitting one wall that is not a plumbing detail.** Reverted; the
tree is back to the committed planes. What it established, all observed:

* *`Side` in the capability works, and the deletion is real.*
  `Resource::Endpoint { channel, side }` with `Side::{Producer,Consumer,Loopback}`
  let `distribute_channel_ends`, `recall_channel_ends`, `ChannelTable::reassign`,
  and `Entry::{producer,consumer}` all be **deleted**, and
  `restore_transferred`'s `reassigned` rollback argument with them. An endpoint
  grant became an ordinary copy beside every other kind. Total footprint was five
  files: `slime-root/src/{channel,graph,main,transit}.rs` plus the two spawn-plane
  assertions below.
* *Holder questions are answerable from the graph.* `mark_dead` became a per-*side*
  abandonment query (`holds_endpoint_side(key, side, except)`), and the
  `peer death channels=N` marker's count became `CapabilityTable::endpoints_held`.
  No refcount was needed, contradicting this entry's earlier sizing.
* *`just sel4_channel_check` and `just sel4_spawn_check` pass*, as do
  `sel4_root_boot_check`, `sel4_component_graph_check`, and `sel4_loan_check`.
  Host tests went 109 → 112.
* *Two spawn-plane assertions had to be inverted, and they are the honest cost.*
  `init.rs:2085` asserted `send` on a granted end answers `ERR_BAD_CAP`, and
  `:2159` asserted it for all six B15 grants; `check-sel4-spawn-plane.py` asserted
  the `channel handed` marker. All three assert the *move*, so option 1 makes them
  false by construction — they encode the divergence rather than a property the
  oracle shares, and `devlog/2026-08-05-p5-3-3-spawn-plane/index.md:283` lists
  "make the endpoint grant a copy" as an intended fault injection.
* *A new test pins B25's actual property* — an end with two holders survives one
  holder dying — and fault-injecting the pre-B25 `mark_dead` fails exactly that
  test and nothing else.

**The wall: `Transit` binds an in-flight capability to a receiver *task*, and with
two holders per end there is no longer a unique one.** `just sel4_sample_check`
wedges: `parked task=0 reason=wait` / `parked task=3 reason=wait`, boot exceeds
180s. `drive_sample_plane` mints a pair, grants the consumer half to
`sample-receiver` and the producer half to `sample-lender`, and — now that a grant
copies — init keeps **both**. `serve_send` resolves the loan's destination through
`channels.peer(channel, id)`, which became "the first live holder of the opposite
side", and init is enumerated before the child. So `transit.depart` binds the loan
to init, `land_caps` calls `transit.arrive(token, receiver)`, that returns `None`,
and the receiver parks forever on a capability delivered to the wrong task.

This is not a bug in the spike; it is the model question the spike surfaces, and it
was written into `channel::peer_of`'s doc before the plane was run. A capability
naming a *queue* cannot name a *recipient*, and message-carried capability transfer
needs a recipient. Two ways out, neither attempted:

1. *Bind transit to a side rather than a task*, and have `arrive` admit any holder
   of that side. Then delivery is first-come between co-holders — fine for the
   x86 call plane, where init sends and only the broker receives, but it makes
   "who gets this capability" depend on scheduling wherever an end really is
   shared.
2. *Make the capability name its recipient*, the way `Resource::Loan`'s
   `LoanHandle` already does (`main.rs:4616` refuses a loan sent to anyone but its
   declared receiver). That is the principled answer and the larger change.

So option 1's cost is now known concretely: the deletions are as clean as hoped,
the passing planes survive, and what remains is *one* genuine design decision about
transit binding — not the wide representation change this entry originally priced.
Neither option is written, because both change what a capability *means* on every
plane and that is a decision to take deliberately rather than as a side effect of
unblocking one gate.

**Exit condition:** A parent grants one end of a minted pair at spawn, uses the
other end afterwards to deliver a capability to that child, and the child
observes it — asserted on a plane that declares such a composition, with a
fault injection showing the parent's end going missing is caught. The call
plane's `[init] call supervision delegated` marker is that composition, already
observed; what remains is for the plane to get past it.

### B28 — a `retained` second route on one publisher stops a *different* publisher's parked role reply from ever being taken

**Resolved 2026-08-07.** Devlog: [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md).
The cause was `MAX_GRAPH_ITERATIONS = 512`: the QoS plane needs more than 512 and
fewer than 768 root round-trips, measured by bisection. No wake was lost, no
capability was stale, no scheduler was inconsistent, and every component was
correct. Bound raised to 2048 with the measurement recorded. Observed exit
condition: `just sel4_qos_check` passes with fourteen markers across nine causal
chains, and restoring 512 makes it red on its own `wedged waiter` signature.

**Problem:** On the P5.4.5 QoS plane, `fabric-publisher` parks once in `recv`
waiting for its role reply and never runs again, although the fabric delivers
both role capabilities to it — the transcript carries
`SLIME_GRAPH capability transferred task=9 channel=5 to=10 kind=endpoint` twice,
and `serve_cap_transfer` calls `deliver_wake` for each. It produces *zero*
further log lines and is still live at teardown, so the plane never reaches
`[init] fabric stream complete`.

**Bisected to one fixture field.** The trigger is `fabric-publisher-b`'s
*diagnostics* participant being `durability = retained` with
`retainedDepth = 2`. Flipping that one participant back to `volatile`/`0` and
rebuilding, with nothing else changed, makes `fabric-publisher` wake and print
`publish role received`. Flipping it to `retained` makes it park forever. The
affected task is a different component on a different route, which is what makes
this a defect rather than a scenario limitation.

**Two earlier readings, both ruled out by experiment.**

* *Starvation behind the clock driver.* `fabric-publisher-b` performs seven
  `advance_time`/`await_time_credit` round-trips, each re-waking the fabric, so
  the obvious reading was that task 10 is woken and never selected. Reducing the
  advance to a **single** step changes nothing — both transfers still land, the
  task still parks once, and it still never runs. Clock volume is not the
  variable.
* *Slow progress.* Extending the boot window from 200s to 700s changes nothing.

**Evidence:** `devlog/2026-08-07-p5-4-5-qos-clock/boot.log` for the retained
case. The stream plane, which is the same graph without the clock or the retained
diagnostics route, runs a byte-comparable transfer sequence and wakes the same
task at the same point.

**Not diagnosed to a line**, but the search is narrowed. What a second retained
route changes inside `fabric-service` is the untraced step: it adds a retained
history the broker maintains, and `create_late_subscriber` now finds a satisfying
publisher where it previously failed — so the broker takes a path it did not take
before, between the transfer and the point where the parked task would be served.

Two resource-exhaustion readings are also ruled out, so a bound is not the cause:

* *`retainedSamples` too small.* The graph declares `2` while two publishers now
  retain depth 2 each, which looks like the obvious ceiling. Raising it to `4` and
  rebuilding changes nothing — the task still parks forever.
* *Frame-table exhaustion.* `FABRIC_FRAME_CAPACITY` is 32 against a retained
  demand of 4, and the transcript carries no frame-exhaustion marker.

A fifth reading is ruled out too, and it moves the suspicion off the broker: the
late-subscriber path **works**. With the diagnostics route retained the transcript
carries `retained history offered to late subscriber`,
`retained history replayed to late subscriber`, and
`retained history expired for late subscriber` in order — and that replay is what
produces the `QoS lifespan expired` arm. The fabric's capability slots peak at 23
of 32, so it is not out of slots either. The broker is healthy and simply parks on
its stream sources with `fabric-publisher`'s request never served.

**The reply is not lost.** `ParkedReplies` is now instrumented: the root emits
`SLIME_GRAPH replies owed count=` and one `reply owed task=` per still-parked task
at teardown, and only when the set is non-empty, so every healthy plane gains no
line. On this boot the answer is a single owed reply belonging to task **6**,
which is `init` waiting on its children — expected and correct. `fabric-publisher`
is **not** in the list, although the transcript shows `parked task=10 reason=wait`
and no later activity from it.

So its wake *was* delivered: 33 park events across the boot, one owed at
teardown. The task resumed, consumed its reply, and then blocked inside seL4
without issuing another root call — which is why it emits nothing further and why
no root-side accounting shows it as outstanding. That excludes every lost-wake and
lost-reply reading, including the two this entry previously carried.

**The precise state, from the root's own accounting.** `parked=1` at teardown and
the owed list names task 6 alone, so task 10 left the parked table — it was woken.
It then issued **no further root call at all**: the transcript carries zero
`received task=10` lines after the wake, and `recv` is the only thing
`receive_role` does between waking and returning.

That is the contradiction to resolve. `receive_role` loops `recv` then
`wait(Endpoint(CONTROL_SLOT))`, both root operations, so a woken task must either
call `recv` again or park again. Task 10 does neither, and it prints nothing — not
even `publish role received`, which is the next statement after the two-capability
loop completes. A task that returns from `wait` and then makes no syscall is
either faulting silently or looping in userspace on a path with no root call in
it.

**The fault check is done and the path is found.** No fault marker appears — the
root reports them (`SLIME_GRAPH component fault`) — so task 10 did not fault. And
there *is* a userspace loop with no root-visible call in it, in the runtime rather
than the component:

`sel4_transport::wait` (`components/runtime/src/syscall/sel4_transport.rs:264`)
stages its source set through the transfer window, and on a staging failure it
calls `yield_now()` and **returns silently** — no `SYS_WAIT`, no error to the
caller, because `wait` returns `()`. `yield_now` is `sel4::r#yield()`, a kernel
primitive that never reaches the root. `receive_role` then loops back to `recv`,
and a caller that keeps failing to stage spins between the two forever while the
root sees nothing at all. That is exactly task 10's signature: woken, no further
root call, no fault, no output.

The comment there says "the caller re-polls either way", which is true only if the
next poll can succeed. When it cannot, the silent return converts a bounded error
into an invisible hang — and `wait`'s `()` return type is what makes it
unreportable.

**That arm is not the cause — refuted by instrumentation.** A temporary
`debug_write` on the staging-failure branch, rebuilt and booted, produces **zero**
lines. `wait` stages successfully every time on this plane, so the silent-yield
path is never taken and task 10 is not spinning there.

The park accounting is also self-consistent, which removes the last root-side
suspicion: 33 park events, one owed reply at teardown (task 6), and task 10 never
appears in a reclaim or peer-death line. Its park entry was therefore *consumed by
a wake* rather than abandoned. It resumed, and then made no root call by any path
the root or the runtime can report.

Seven readings are now excluded: lost wake, lost reply, starvation, clock volume,
boot duration, the `retainedSamples` bound, the frame table, a component fault, and
the runtime's silent-yield arm.

**Localized to the first `receive_role` iteration.** A marker compiled into
`fabric-publisher` between `role requested` and its two-capability loop prints
`awaiting role cap` and then nothing: the task blocks inside the *first* iteration,
never reaching `role cap arrived`. It is not stuck on the second capability, and it
is not past the loop.

The wiring is right, which is what makes this narrow. Task 10 holds the control
channel at the slot it reads — `channel handed parent=6 child=10 key=5 slot=0`
against `CONTROL_SLOT = 0` — and the fabric transferred both capabilities to that
exact channel (`capability transferred task=9 channel=5 to=10`, twice, rights
`0x1` then `0x2`). So the transfers targeted the queue the receiver polls, the
receiver polled it, and it saw nothing.

That points at `serve_cap_transfer`'s enqueue-plus-wake against a receiver that is
parked *at that moment*: the fabric's two transfers land back to back while task 10
is parked from its `wait`, and the second finds `deliver_wake` a no-op because the
first already un-parked it — but the first wake races the enqueue of the second
capability. On the stream plane the same pair lands and the receiver drains both,
so the ordering that breaks is graph-dependent, which is consistent with the
retained bisect.

That ordering was then read, and it is **correct**, so this reading is refuted too.
`Channel::commit_send` enqueues and `take()`s `recv_waiter`, so the first transfer
carries the wake and the second correctly returns `None`; the receiver is expected
to drain both messages once awake. The transcript confirms the order is favourable:
`parked task=10 reason=wait` precedes both transfers, so `deliver_wake`'s
`parked.reason(task).is_none()` guard cannot have skipped the first wake.

So: the receiver parks, two messages are enqueued on the queue it polls, the wake
is delivered to a task the root agrees is parked, its park entry is consumed, and
it never runs again. Every step is individually correct and the composition
deadlocks.

Two further readings were tested and refuted, which is worth recording because both
look compelling from the transcript:

* *Both ends of the loopback given away.* Init mints `key=5` as a loopback and the
  log shows it handed to child 9 *and* child 10, so init keeps neither end — which
  would leave the queue's `producer`/`consumer` naming only the two children. That
  is exactly what `reassign`'s loopback split is for, and the **stream plane does
  the identical thing** (`channel handed parent=6 child=9 key=5` then
  `… child=10 key=5`, same line shape, same order) and drains both messages. Not
  the cause.
* *Round-robin starvation.* On the stream plane task 10 runs only after the fabric
  parks and every other task blocks, so it is plainly last in the queue — and on
  the QoS plane the clock keeps the fabric busy. But the QoS fabric still parks
  **eight** times after the transfers, so task 10 has scheduling opportunities and
  does not take them. Not the cause either.

Ten readings excluded from the boot log. Both planes are byte-comparable through the
park and the two transfers, the rights on the control end are `send|recv` so
`WAIT_KIND_ENDPOINT` resolves, and every root-side structure reports consistent
state.

**The debugger settles it.** Booting under `-gdb tcp::1234`, letting the plane reach
its deadlock, and attaching `lldb` shows the CPU parked at `0x8060011190`, inside a
`b .` self-loop. Resolving that against the kernel's symbol table puts it in
`idle_thread` (`0x806001118c`, the symbol immediately below). **seL4 has no runnable
thread at all** — so task 10 is not spinning in userspace and not starving behind a
peer: it is blocked in the kernel on an endpoint nothing will signal.

That inverts the remaining suspicion back onto the root, with one specific
candidate. `parked::send_reply` ends with `slot.cap().send(info)` and **discards the
result**. Every accounting structure the root keeps is updated as though the reply
was delivered — the entry is removed from `ParkedReplies`, `recycled` is bumped, the
`reply owed` list is correspondingly empty for task 10 — while an `seL4_Send` that
failed would leave the child blocked forever with no trace anywhere. That is exactly
the observed combination: consistent root bookkeeping, an idle CPU, and a child that
never resumes.

**The send is not the loss either.** `sel4::cap::Unspecified::send` returns `()` —
seL4's `Send` reports nothing, so there was no discarded error to find. Bracketing
the call with markers shows `SLIME_DBG wake replying task=10` followed by
`wake replied task=10`: the root reaches the send, performs it, and returns. The
same bracket fires and *works* for tasks 4, 5, 7, 8, and 9 in the same boot, so the
save/park/wake/reply path is sound in general.

**So the defect is narrower than any structure the root can inspect.** Task 10's
reply is sent over its saved capability, the send returns, the CPU then goes idle
with no runnable thread, and the child never resumes. Every layer reports success
and the thread stays blocked. Eleven readings are now excluded, including the
debugger-motivated one.

**The reply capability is live — measured, not assumed.** `KernelDebugBuild` is
already `ON` in `sel4/config/qemu-arm-virt.cmake`, so `seL4_DebugCapIdentify` is
available; `Cap::debug_identify` was called on the saved slot immediately before the
send. Task 10's slot reports `kind=8`, which is `cap_reply_cap` in
`build/sel4-qemu/generated/arch/object/structures_gen.h:635` — and it is the *same*
kind reported for tasks 4, 5, 7, 8, 9, 11, and 12, every one of which wakes
correctly in the same boot.

So every root-side link is now measured and sound: the task parks, its reply is
saved as a genuine `cap_reply_cap`, two messages are enqueued on the queue it polls,
`deliver_wake` fires while the root agrees it is parked, the send is performed over a
capability the kernel confirms is a live reply cap, and the send returns. The CPU
then idles with no runnable thread and the child never resumes. **Twelve readings
excluded.**

**The kernel's own scheduler state confirms a true deadlock, not starvation.**
Reading it through the gdbstub with the kernel ELF loaded as a symbol target:

* `ksCurThread = 0x8060030c00`, the idle TCB — matching the `idle_thread` PC.
* `ksSchedulerAction = 0`, i.e. `SchedulerAction_ResumeCurrentThread`: the kernel
  has decided there is nothing to switch to.
* `ksReadyQueues[0]`, `[1]`, `[254]`, and `[255]` all have `head = NULL`. Priority
  254 is `CHILD_PRIORITY` and 255 is the root's, so **no thread at any priority is
  runnable**.

That closes the starvation question for good: every thread in the graph is blocked,
including the root. It also means the missing wake is not a scheduling artifact — a
runnable-but-never-selected thread would sit in `ksReadyQueues[254]`, and it does
not.

So the state is fully characterized and internally contradictory at the seL4
boundary: the root sent a reply over a capability the kernel identifies as a live
`cap_reply_cap` naming a blocked thread, and that thread did not become runnable.
Thirteen readings excluded.

**A TCB state read deepens the contradiction rather than resolving it.**
`ksDebugTCBs` (the kernel's debug thread list, available because
`KernelDebugBuild` is on) heads at `0x80604f6c00`, and `tcbState` is the first field
of `tcb_t`. Reading it there gives word 0 = `0x1`, which is
`ThreadState_Running` in `deps/sel4/include/object/structures.h:160`.

So at the deadlock there is a thread the kernel considers **Running** while
`ksReadyQueues` is empty at every priority and `ksCurThread` is the idle TCB. Those
three facts cannot all be consistent with a healthy scheduler: a Running thread
belongs in a ready queue or is current, and this one is neither.

That is the sharpest statement available and it is worth stopping on rather than
guessing past. Fourteen readings excluded. Two candidates remain, and they are
different bugs:

* the thread was made Running and then never enqueued — a missing
  `SCHED_ENQUEUE`, which on this path would be inside seL4's own reply handling;
* or the TCB at the head of `ksDebugTCBs` is not the thread this concerns, and the
  Running state belongs to something else entirely — in which case walking
  `tcbDebugNext` to identify each thread is the remaining read.

**The second candidate is now settled: it is a real child thread.** This build has
no `tcbDebugNext` field, so `ksDebugTCBs` is not a walkable list here — but the TCB it
points at can be identified directly. At `0x80604f6c00`, `tcbPriority` (offset 920)
reads **254**, which is `task::CHILD_PRIORITY`. The idle thread is a different object
at `0x8060270000`-ish with `tcbPriority = 0`. So the Running TCB is one of the
graph's own components, not the idle thread and not the root.

**The inconsistency is therefore confirmed at the kernel level:** a child thread at
priority 254 in `ThreadState_Running`, absent from `ksReadyQueues[254]` (and from
every other priority's queue), while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. A Running thread that is neither
current nor enqueued cannot be scheduled again, which is exactly the observed hang.

Fifteen readings excluded, and the defect is now located to one transition rather
than a subsystem: something set a child's state to Running without enqueuing it, on
the path a reply to a parked task takes. That is either seL4's own
`setThreadState`/`possibleSwitchTo` sequence for a reply-send to a thread blocked in
`Recv`, or a root-side invocation that leaves the thread in Running without the
kernel completing the switch.

**Thread naming was tried and does not help on this build.** `seL4_DebugNameThread`
is exposed by `rust-sel4` as `cap::Tcb::debug_name`, and calling it at spawn compiles
and boots cleanly — but this kernel has no `tcbName` field at all, so nothing stores
the label and no dump can report it. `KernelDebugBuild ON` gives `DebugCapIdentify`
and the `ksDebugTCBs` pointer without the naming storage that
`CONFIG_DEBUG_BUILD`'s thread-name support would add. The change was reverted rather
than left as a call whose effect is unobservable.

**The thread is identified: it is task 10, `fabric-publisher` itself.** Matching was
done through the IPC buffer rather than the VSpace, because the root already prints
the derived address. The Running TCB reports
`tcbIPCBuffer = 0x237000`; `child_vspace.rs` sets `ipc_buffer_addr = footprint.end`
and places the transfer window one page above it, so that TCB's window is
`0x238000` — and the transcript's `window bound task=10 base=0x238000` names exactly
one spawned task with that address. Task 10 is the `fabric-publisher` instance init
spawned, which is the thread that never wakes.

Its saved context is consistent with a live component rather than a fresh one:
`registers[31]` (PC) is `0x2366f0`, far above the `entry=0x211e78`
`fabric-publisher` was started at.

**So the defect is now stated exactly.** `fabric-publisher`'s thread is in
`ThreadState_Running` with a plausible mid-execution PC, absent from
`ksReadyQueues` at every priority, while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. The root has sent it a reply over a
capability the kernel identifies as a live `cap_reply_cap`. Sixteen readings
excluded; every layer above the scheduler checks out.

**The kernel's reply path was read and it explains the state without being wrong.**
Non-MCS `doReplyTransfer` (`deps/sel4/src/kernel/thread.c:133`) opens with
`assert(thread_state_get_tsType(receiver->tcbState) == ThreadState_BlockedOnReply)`.
On success it does `cteDeleteOne(slot)`, `setThreadState(receiver, Running)`, then
`possibleSwitchTo(receiver)` — so the enqueue is not missing from the kernel.

`possibleSwitchTo` is where a Running thread can legitimately end up in no queue:
when the target shares the current domain and `ksSchedulerAction` is
`ResumeCurrentThread`, it takes neither `SCHED_ENQUEUE` branch and instead sets
`ksSchedulerAction = target` — a *pending switch* held outside the ready queues.
`schedule()` consumes that correctly, so the design is sound; but the measured state
at the deadlock is `ksSchedulerAction = 0` (`ResumeCurrentThread`) with the target
Running and unqueued, which is that pending switch having been **cleared without
being honoured**.

**So the shape of the bug is now pinned even though the culprit is not.** Something
between `possibleSwitchTo` recording the switch and `schedule()` acting on it reset
`ksSchedulerAction` to `ResumeCurrentThread` — plausibly a second
`possibleSwitchTo`/`rescheduleRequired` interleaving from another root operation in
the same kernel entry, which the root's single-threaded dispatch makes possible when
one syscall replies to two different tasks. That is consistent with B28 appearing
only when the retained diagnostics route adds a second reply-bearing path.

**The multi-reply interleaving exists and is observable.** `reclaim_dead_task`
(`slime-root/src/main.rs:4281`) loops over `DeathWakes` and calls `deliver_wake` — so
`send_reply` — once per wake, all inside one kernel entry. The QoS transcript records
`peer death task=3 channels=5 woken=2`: two tasks replied to in a single root
operation. Each `seL4_Send` on a reply cap runs `possibleSwitchTo` for its receiver,
and the second one's call sees `ksSchedulerAction` already holding the first target
rather than `ResumeCurrentThread` — the branch that then fires is
`rescheduleRequired()` plus `SCHED_ENQUEUE`, which enqueues the *first* target and
requests a reschedule.

**But the timing refutes it as task 10's cause.** The only `woken=2` line in the
transcript is at boot-log line 184, and task 10 does not park until line 283. A
pending switch that never existed cannot have been cleared, so this interleaving —
real as it is — is not what strands `fabric-publisher`. Every later wake in that boot
is `woken=0` or `woken=1`, i.e. one reply per kernel entry.

Seventeen readings excluded. That leaves the contradiction fully measured and
unexplained by any mechanism inspected so far: a `Running` child at priority 254,
absent from every ready queue, `ksSchedulerAction = ResumeCurrentThread`,
`ksCurThread` idle, reached after a single reply send over a live `cap_reply_cap`.

**A kernel breakpoint identifies the branch and the caller.** Booting with `-S`,
setting `breakpoint set -n possibleSwitchTo -c "(unsigned long)target ==
0x80604f6c00"`, and continuing stops with:

```
frame #0: possibleSwitchTo(target=0x80604f6c00) at thread.c:562
frame #1: restart(target=…) at thread.h:99 [inlined]
frame #2: invokeTCB_Resume(thread=…) at tcb.c:1698 [inlined]
```

and, at that moment, `ksSchedulerAction == 0`, `ksCurDomain == 0`,
`target->tcbDomain == 0`.

So the call reaching task 10 is its **activation** — `TCB_Resume`, which is
`tasks.activate(id)` from the root's launch loop — not a reply at all. And with the
domains equal and `ksSchedulerAction` at `ResumeCurrentThread`,
`possibleSwitchTo` takes its third branch: `NODE_STATE(ksSchedulerAction) = target`.
The thread is left `Running`, **deliberately unqueued**, with the switch pending in
`ksSchedulerAction` — exactly the state observed at the deadlock, and the reason
`ksReadyQueues[254]` is empty while a child is Running.

**Two corrections, both refuting the paragraphs above.** They are recorded rather
than deleted because each looked conclusive and each is a trap the next reader would
fall into.

*The TCB was not identified.* The IPC-buffer match is ambiguous: **three** tasks in
this boot bind window `0x238000` — task 6 (`init`), task 1 (the root-launched
`fabric-publisher`), and task 10 (the one init spawned). `child_vspace` lays every
component out identically, so `tcbIPCBuffer` cannot distinguish them, and the claim
that the Running TCB is task 10 does not follow. The `tcbPriority = 254` reading
still holds, so it is *a* child rather than the root or the idle thread — nothing
finer.

*No activation switch is dropped.* `rescheduleRequired`
(`deps/sel4/src/kernel/thread.c`) **enqueues** the pending target before overwriting
the action:
`if (action != ResumeCurrentThread && action != ChooseNewThread) SCHED_ENQUEUE(action)`.
So the second `possibleSwitchTo` in the root's activation loop takes exactly that
branch and the first target is enqueued, not lost. The transcript agrees:
`activated components=7`, and six of the seven root-launched instances go on to print
their own failure line, so they demonstrably ran. The breakpoint that fired was one
of those early activations, before task 10 existed at all.

**So the state is measured and the mechanism is still unknown.** Nineteen readings
excluded. What remains true: some child thread is `Running` at priority 254, absent
from every ready queue, with `ksCurThread` idle and `ksSchedulerAction` at
`ResumeCurrentThread`; and `fabric-publisher`'s spawned instance never resumes after
its role reply.

**The VSpace route was tried and does not close the gap either.** The stranded TCB's
`tcbVTable` entry reads a VSpace root of `0x80604b8000` — a kernel object address.
Logging `task.vspace.vspace.bits()` per task gives `0x2c7`, `0x305`, `0x345`, `0x386`,
`0x3cb`, `0x40c`, `0x44d`: distinct per task, and therefore a genuine discriminator,
but they are *root CSpace slot numbers*, not the kernel addresses the TCB stores. The
two namespaces cannot be compared without resolving each cap to its object, which is
what a kernel with thread-name support would have made unnecessary. The
instrumentation was reverted.

**So B28 stops here, root-caused only as far as the evidence allows.** Nineteen
readings excluded. Established beyond doubt:

* `fabric-publisher`'s spawned instance parks once for its role reply and never
  resumes; the plane cannot reach `[init] fabric stream complete`.
* Every root-side layer is correct and measured: park entry, a live `cap_reply_cap`
  (`debug_identify` = 8), `deliver_wake` firing against a task the root agrees is
  parked, the send performed and returning.
* At the deadlock the kernel holds *a* child thread (priority 254) in
  `ThreadState_Running`, absent from `ksReadyQueues` at every priority, with
  `ksCurThread` idle and `ksSchedulerAction` at `ResumeCurrentThread`.
* It is triggered by one fixture field — `retained` on the diagnostics participant —
  and that same field buys two of the five observed C8.5 arms.

**The kernel state was misread, and reading it correctly changes the diagnosis.**
`CONFIG_DEBUG_BUILD` *is* set in `build/sel4-qemu/gen_config/kernel/gen_config.h`, so
`tcbDebugNext`/`tcbName` do exist; lldb could not see them because they live in a
separate `debug_tcb` struct placed inside the TCB's CTE array
(`TCB_PTR_DEBUG_PTR(p) = TCB_PTR_CTE_PTR(p, tcbArchCNodeEntries)`), not in `tcb_t`.
The list is walkable at `(tcb & ~0x7ff) + 0xa0`, and walking it finds seven threads.

An earlier hand-rolled `state` read reported all of them `Running`, which is
impossible on one core and was the tell that the offset arithmetic was wrong. Reading
`tcbState.words[0] & 0xf` through the debug info instead — the same expression
`thread_state_get_tsType` uses — gives:

|TCB|`tsType`|Meaning|
|---|---|---|
|`0x80604f6c00`|4|`BlockedOnSend`|
|`0x80604b7c00`|4|`BlockedOnSend`|
|`0x8060473c00`|4|`BlockedOnSend`|
|`0x8060433c00`|4|`BlockedOnSend`|
|`0x80603f2c00`|5|`BlockedOnReply`|
|`0x8060030c00`|7|`IdleThreadState` (prio 0)|
|`0x807fd8a400`|0|**`Inactive`** — the root task|

The idle thread reading `IdleThreadState` rather than `Inactive` is the control that
confirms the typed read is right and the raw one was not. **So there is no Running
unqueued thread and no scheduler inconsistency.** Every prior paragraph resting on
that — the "kernel-level inconsistency", the dropped pending switch, the
`possibleSwitchTo` third-branch theory — is void. The ready queues are empty because
every thread is legitimately blocked.

**The root task is `Inactive`: it returned.** And the transcript shows why that is
fatal — the last lines are the root's own accounting, printed *after* the serve loop
fell out, including `replies owed count=1` / `reply owed task=6`. The serve loop
(`slime-root/src/main.rs:1522`) is `for _ in 0..MAX_GRAPH_ITERATIONS { if live == 0
{ break } … }`. With `sends=41 receives=37 parks=33`, roughly 111 operations ran
against a bound of 512, so the loop did **not** exhaust its iteration budget — it left
by `live == 0`.

**The `live == 0` reading was wrong too, and the loop's own marker says so.** A guard
was added making an owed reply at `live == 0` fatal; it did **not** fire. The
post-loop line then gave the answer directly: `served live=5`. The loop leaves with
**five tasks still live** and one reply owed, so it exits by exhausting
`MAX_GRAPH_ITERATIONS` — the root spins 512 times without any arrival advancing the
graph, then returns, which is what marks it `Inactive`.

**So the defect is a genuine wedge, and it was silent.** Falling out of the bound was
indistinguishable from settling: the root printed its ordinary accounting summary and
the boot looked healthy apart from a missing final marker. That is exactly how B28
stayed invisible through nineteen readings aimed at the reply path — `fabric-publisher`
never resuming is a *symptom* of the root going away, not an independent fault, and the
four `BlockedOnSend` children are blocked sending to a root that is gone.

**Fixed to that extent: the wedge is now reported.** `serve_component_graph` counts its
iterations and, on reaching the bound with tasks still live, fails with
`SLIME_GRAPH FAIL graph iterations exhausted live=5 parked=1` — observed on the QoS
plane. All nine passing planes were re-run and stay green, so the detector
distinguishes a wedged graph from a settled one rather than tripping on both. The
`live == 0` guard is kept beside it: unreachable today, but it is the other way a
graph can end owing a reply.

**It is a livelock in `fabric-service`, not a deadlock anywhere.** Logging the
operation label on the loop's final iterations shows task 9 — `fabric-service` — in a
fixed cycle: five `Recv` then one `Wait`, repeating to the bound. A `wait` that
returns immediately is a park on a source that is permanently ready, so the broker
burns the root's iteration budget instead of blocking.

**Two always-ready sources found and fixed** (`d69cd8e`). `park_on_streams` already
skips finished publishers and its own comment states the rule — a dead source is
always ready, so leaving one in the set turns the park into a spin — but never applied
it to:

* a subscriber whose peer is gone (`ended` is set both on a clean end event and on
  `ERR_PEER_DEAD`);
* the QoS clock (`TIME_SLOT` was pushed on the flag alone, though the worker already
  probes `time_peer_dead()` before asking it to advance).

With both exclusions the cycle widens from five `Recv` per park to about eleven, so
the always-ready wake is gone. All nine passing planes re-run green, so neither
exclusion changes a settled graph.

**The plane still wedges, and the remaining cause is now located to one condition.**
The stream worker returns only when *every* subscriber has `ended` **and** the clock
peer is dead (`components/bins/src/bin/fabric-service.rs`, the
`all(|subscriber| subscriber.ended)` block). The clock's client half is granted to
`fabric-publisher-b` (`init.rs:1748`), and that component reaches
`[fabric-publisher-b] done`; the root then reports
`peer death task=11 channels=6 woken=1` at transcript line 394, *before* the spin
window opens at 396. So the clock peer does die, the fabric is woken for it, and the
broker still does not take its exit — which places the defect in the broker's handling
of that wake rather than in the wake's delivery.

**Instrumented, and the causal chain is now complete.** Printing the exit block's
conjuncts on every pass gives `subs ended=3/0` then `3/1`, held forever: the broker
carries **three** subscribers and only **one** ever reaches `ended`. The clock probe is
never even attempted, because the first conjunct never becomes true — so the earlier
suspicion about `time_peer_dead()` racing an advance is void.

Instrumenting `announce_end`'s guard shows why. It refuses to end a subscriber that
still holds history or unacknowledged samples
(`if !subscriber.terminal && (!subscriber.history.is_empty() || subscriber.in_flight
!= 0)`), and the two stuck subscribers sit at `hist/inflight=1/1` permanently: one
sample delivered, never acknowledged.

**And the reason they never acknowledge is already on the transcript, upstream of
everything QoS.** `[fabric-subscriber] fail: role reply` and
`[fabric-subscriber-b] fail: role reply` — both subscribers die at role
provisioning, before they can ack anything, and the root duly reports
`peer death task=4` / `peer death task=5`. Their samples stay in flight forever, so
`announce_end` never fires for them, so the broker's exit condition is unreachable, so
it spins to the iteration bound.

**Attributing this to B25 was wrong, and the passing plane disproves it.** Booting the
**stream** plane — which reaches `[init] fabric stream complete` — shows the *same two*
`fail: role reply` lines from `fabric-subscriber` and `fabric-subscriber-b`. They are
expected negative-control assertions on both planes, not the defect, and B28 is not a
B25 symptom.

**The real differentiator is the retire path.** The stream plane logs
`[fabric] QoS peer dead` **twice** — the broker observes both dead subscribers on their
ack channels and calls `retire_subscriber`, which is what lets its exit condition
become true. The QoS plane logs it **zero** times, both before and after the park-set
fixes, so the exclusion in `d69cd8e` is not the cause. Counts: `QoS matched` 7 on the
stream plane vs 6 on QoS.

So the two subscribers stuck at `hist/inflight=1/1` are stuck because the broker never
takes `drain_acks`' `ERR_PEER_DEAD` arm for them, not because they died at role
provisioning — which they do on both planes.

**Traced further, and the ack channels are a red herring too.** `drain_acks` is called
unconditionally for every present subscriber — no flag gates it — so the flag-split
suspicion is void. Instrumenting the publisher sweep shows the broker pumps exactly one
publisher, index 2 / slot 20, which is `fabric-publisher`'s own route: publishers 0 and
1 are already `finished` when `broker` starts. Its `recv` returns `WOULDBLOCK` on every
pass because **`fabric-publisher` (task 10) never resumes to publish anything**. The
subscribers are stuck at `hist/inflight=1/1` waiting on samples that task 10 would have
sent. So the whole QoS-side chain reduces back to the original symptom.

**The root's side of that wake is now fully instrumented and is correct.** Four
measurements, each reverted after being taken:

* `deliver_wake`'s silent `parked.reason(task).is_none()` early return — added a marker;
  **zero** lines. No wake is ever dropped for being unparked.
* `send_atomic`'s wake for the transfer that carries the role — `xfer wake
  present=true target=10`. The wake *is* generated.
* `deliver_wake` reaching its answer — `wake answering task=10` fires. The task is
  answered.
* `ParkedReplies::wake` ordering — `send_reply(held.slot, …)` completes before
  `release_slot` deletes the slot, so the B29 fix does not invalidate the reply it just
  sent.

**And the plane comparison rules out composition.** Byte-for-byte against the *passing*
stream plane, task 10 parks at the same point, receives exactly the same two
`capability transferred … to=10` records on the same channel (key 5 = its
`CONTROL_SLOT = 0`), and the same `sent … queued=1` / `QoS matched` pairs follow. The
two planes are indistinguishable through the entire role handoff; the stream plane then
shows `received task=10` twice and QoS shows it **zero** times.

**So the defect is isolated to one transition with everything around it verified:** the
root sends a reply over a live `cap_reply_cap` to a task the kernel has in
`BlockedOnReply`, on a plane whose every preceding step matches a plane where the same
send works, and the task does not run. Twenty-one readings excluded.

**The baseline was taken, and it retires the kernel-state line of inquiry.** Walking
`ksDebugTCBs` on the *passing* stream plane at `[init] fabric stream complete` gives
`idle=0x8060030c00 cur=0x8060030c00 action=0x0` and only **two** threads on the list:
the idle thread and the root. Every one of the six children is gone, properly reclaimed.

So the healthy terminal state has *no* children, and the QoS plane's five survivors are
the anomaly — which is exactly what a livelocked broker produces and needs no kernel
explanation at all. `ksCurThread == ksIdleThread` with `ksSchedulerAction ==
ResumeCurrentThread` and empty ready queues is *also* the healthy end state, so none of
those three readings ever indicated a fault. This is the control that should have been
taken before any of the seL4 work.

**The component-side marker was taken and it settles what task 10 is doing.**
Bracketing `receive_role`'s wait arm in `fabric-publisher` prints
`[dbg] role: parking` and **never** `[dbg] role: wait returned`. So task 10 is not
looping between `recv` and `wait`, and it is not mis-decoding an answer: it is blocked
inside one `slime_rt::wait` that never returns. The staging-failure arm above it
(`[rt] wait source set could not be staged`) does not fire either, so the wait set did
cross intact.

**And the root's reply is provably aimed at that exact wait.** Instrumenting
`ParkedReplies::commit`/`wake` to print the CSlot index gives, in order:

```
[dbg] role: parking                        <- component enters wait
SLIME_DBG park task=10 slot=1667           <- root saves the reply cap
SLIME_GRAPH parked task=10 reason=wait     <- committed as a Wait, not a Recv
SLIME_DBG wake task=10 slot=1667           <- answered on the same slot
```

`slot=1667` appears exactly twice in the whole boot, so the index is neither reused nor
recycled between the park and the send, and the park is committed with
`ParkReason::Wait` — the operation the component is actually blocked in. Combined with
the earlier readings, every link is now individually verified: the wake is generated
(`xfer wake present=true target=10`), it is not dropped as unparked, `deliver_wake`
reaches `wake answering task=10`, the slot identity matches, `send_reply` runs before
`release_slot`, and the kernel calls the cap a live `cap_reply_cap`.

**So B28 is isolated to a single unexplained transition:** the root invokes
`slot.cap().send(info)` on a live reply capability naming a task the kernel has parked
in `SYS_WAIT`, on the correct CSlot, and that task never resumes — while the *same*
code path answers tasks 4, 5, 7, 8, 9, 11 and 12 correctly in the same boot, and
answers task 10 itself correctly on the stream plane. Twenty-two readings excluded.

**The wait registration was checked too, and it is correct.** Instrumenting
`serve_wait`'s registration loop for task 10 prints `w10 registering
target=Receive(5)` — the same channel key the fabric's two
`capability transferred task=9 channel=5 to=10` records land on, and the same key the
root hands the component at `channel handed parent=6 child=10 key=5 slot=0`. The
registration also passes through `ChannelTable::recv_queue_mut(key, task)`, which
resolves `forward` for the consumer and `reverse` for the producer, so a
holder/direction mismatch would have registered on the wrong queue — it does not.

`serve_wait` additionally re-probes readiness *after* registering, specifically to close
the lost-wakeup window between the first probe and the registration, so a send landing
in that gap cannot be missed.

**Every layer of the root is now individually verified against this one hang**, and the
list is worth keeping because it is what makes the residue small: wait set stages, wait
target resolves to the right key, registration lands on the right queue, readiness is
re-probed after registering, the wake is generated by `send_atomic`, it is not dropped
as unparked, `deliver_wake` reaches its answer, the park was committed as
`ParkReason::Wait`, the reply CSlot index matches the park exactly and is used twice in
the whole boot, `send_reply` runs before `release_slot`, and the kernel identifies the
capability as a live `cap_reply_cap`.

**A note on one earlier kernel reading, so it is not trusted later.** Re-walking
`ksDebugTCBs` on the wedged QoS plane with *typed* reads now gives two children
`BlockedOnSend`, three `BlockedOnReply`, idle at `IdleThreadState`, and the root
`Inactive`. The root being `Inactive` is an artifact of this entry's own wedge
detector — `fatal!` fires and the root exits — so that snapshot describes the
post-mortem, not the hang. Any future kernel reading of this defect must be taken with
the detector disabled, or it measures the detector.

**Two more candidates checked and both refuted, by reading rather than by running.**

`root_service()` cannot differ per task: it is
`cap::Endpoint::from_bits(ROOT_SERVICE_SLOT)` with `ROOT_SERVICE_SLOT = 1`, a
compile-time constant every component shares. Task 10 calls the same endpoint as the
tasks that are answered correctly in the same boot.

The endpoint's *rights* looked more promising, because
`task.rs:348` gates the `grant` right on the generation declaring the grant
transferable — and `init-fabric-publisher` in `sel4-qos.zti` is
`rights = ["exec"; "spawn"]` with `transferable = false`, so task 10's service endpoint
carries `grant_reply` but not `grant`. That would plausibly stop a reply that conveys a
capability. But the *stream* fixture declares that grant identically —
same rights, same `transferable = false` — and delivers the same two transfers to the
same task successfully. So it is not the difference either.

**The fixture diff was done and it is remarkably small.** `sel4-stream.zti` and
`sel4-qos.zti` differ in exactly **three** fields: `generation` (1 vs 19), and on
`fabric-publisher-b`'s *diagnostics* participant `durability`
(`volatile` → `retained`) and `retainedDepth` (`0` → `2`). Nothing else — same
components, same grants, same telemetry route, same capacities
(`FABRIC_FRAME_CAPACITY = 32` on both). The wedged task is `fabric-publisher` on the
*telemetry* route, a different component and a different route from the one field that
changed.

**And the observable consequence is in the loan table, not the frame table.** Both
planes create five loans. The stream plane maps and returns **three**; the QoS plane maps
and returns **one**. Per-loan:

|Plane|`id=1`|`id=2`|`id=3`|
|---|---|---|---|
|stream|mapped by task 9|mapped by task 7|mapped by task 8|
|QoS|mapped by task 9|**never mapped**|**never mapped**|

Loans 2 and 3 are created by the broker and never taken by the subscribers — which is
exactly the `hist/inflight=1/1` state the subscribers are stuck in, seen from the other
side.

**Two tempting explanations for that are already excluded.** It is not frame
exhaustion: instrumenting `pump_publisher`'s `!frames.iter().any(|f| f.refs == 0)` guard
produced **zero** lines. And it is not queue backpressure: the *stream* plane runs
deeper queues (`queued=` up to 11) than QoS (up to 6), so a full channel cannot be what
stops the QoS delivery.

What the transcripts do show at the divergence is a scheduling difference in the broker
itself. After the same `capability transfer task=9 … to=8` and `[fabric] downstream loan
created`, the stream plane keeps serving (`received task=9 channel=21`) while the QoS
plane immediately emits `[fabric] idle: parked on stream sources` and
`parked task=9 reason=wait`. The broker parks with two loans outstanding and undelivered.

**`deliver`'s decline was instrumented, and it is correct behaviour, not the bug.**
Every refusal comes from one arm: `history.entry_at(subscriber.in_flight)` returning
`None`. That is by design — `entry_at` documents `offset >= len => None`, and the
stuck subscribers sit at `in_flight = 1` with `len = 1`, meaning everything the ring
holds has already been sent and is awaiting an ack. `deliver` is right to stop, and the
`in_flight >= history.depth()` gate above it is not involved either (the subscribers
declare `historyDepth = 8`).

**The eviction bookkeeping is also correct**: `history.push` returning an evicted entry
decrements `in_flight` and releases the frame, so a stalled subscriber cannot ratchet
`in_flight` past its depth.

**The subscribers are alive, not dead.** This corrects an assumption three earlier
paragraphs shared. `peer death task=4` / `task=5` name the *root-launched* subscribers,
which hold one channel each and are never provisioned into the graph. The broker's real
subscribers are tasks **7 and 8**, and on the QoS plane they never die at all — they are
`parked`, waiting for samples. On the stream plane they do eventually die
(`channels=3`, `channels=5`). So no `ERR_PEER_DEAD` is owed on their ack channels, and
`drain_acks`' peer-death arm is right not to fire.

**Which puts the whole chain back on one fact:** `fabric-publisher` (task 10) sends
**2** messages on the QoS plane and then blocks in the `SYS_WAIT` that never returns.
Task 11 sends 10, so the broker keeps working; the subscribers ack twice
(against 18 on the stream plane) and then park because no further sample arrives. Every
downstream symptom — the two unmapped loans, `hist/inflight=1/1`, `announce_end`
refusing, the broker spinning — follows from that single hang, and none of them is an
independent defect.

**One candidate fix was written, verified not to fire, and reverted.** `deliver`
collapsed `ERR_WOULDBLOCK | ERR_PEER_DEAD => false` on both send paths, so a dead peer
was retried like a busy one; splitting the arms to retire the subscriber built clean and
kept all nine planes green, but the new arm was never reached on *any* plane — the QoS
plane returns earlier at `entry_at`, and the stream plane's two `[fabric] QoS peer dead`
lines both come from the pre-existing `drain_acks` path. Unobserved code is not a fix,
so it was reverted rather than committed.

**The root now names its wedged waiters, and the answer is not task 10.** The wedge
`fatal!` fired *before* the owed-reply accounting further down the function — and
`fatal!` does not return — so the one path that most needed the diagnosis printed
only counts. Fixed: the exhaustion arm iterates `parked.tasks()` first and emits
`SLIME_GRAPH wedged waiter task=N` per entry.

On the QoS plane that gives, in order: **7, 8, 9, 6** — `fabric-subscriber`,
`fabric-subscriber-b`, `fabric-service`, and `init`. **Task 10 is absent.**

That overturns the reading this entry was built on. `fabric-publisher` (task 10)
sends twice, parks once, is never reclaimed, and is *not* among the tasks the root
is holding a reply for. So its `SYS_WAIT` was **answered** — the root is not owing
it anything — and it still did not resume. The four tasks actually stuck are the
broker and its two subscribers, which is a different shape entirely: the broker is
waiting on sources the subscribers would make ready, and the subscribers are waiting
on samples the broker would deliver.

**The root now prints the whole deadlock.** `ChannelTable::registered_waits` and
`Channel::waits_for` were added — diagnostic-only scans — so the exhaustion arm
emits each waiter's park reason and every channel it is registered on:

```
wedged waiter task=7 reason=Some(Wait)   channel=16 receive=true
wedged waiter task=8 reason=Some(Wait)   channel=12 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=13 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=17 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=22 receive=true
wedged waiter task=6 reason=Some(Wait)   (no channel — a supervision wait)
```

All five channels are broker-minted role endpoints. Channel 22 is the interesting
one: it is minted at transcript line 285 and *is* transferred to task 10 at line
286 (`capability transferred task=9 channel=5 to=10`), so the broker holds one half
and `fabric-publisher` the other. The broker is waiting to receive a sample on it;
task 10 parked without ever sending one.

So the cycle is: the broker waits on the publisher's route (22) and both
subscribers' acks (13, 17); the subscribers wait on their data channels (16, 12)
which only the broker fills; `init` waits on a supervision handle. Nothing can
move because the one task that would break the cycle — task 10 — is parked and
**is not owed a reply by the root**, having been answered already.

**Narrowed once more, to a two-line difference between the two transcripts.**
`fabric-publisher`'s `receive_role` loop runs **twice** — a publisher role is two
capabilities, a send-only data endpoint and a receive-only credit endpoint
(`fabric-publisher.rs:126-143`). Both planes deliver exactly two transfers to task
10. The difference is what happens next:

| | stream (passes) | QoS (wedges) |
|---|---|---|
| `parked task=10 reason=wait` | line 280 | line 283 |
| `capability transferred … to=10` ×2 | 283, 285 | 286, 288 |
| **`received task=10 channel=5`** | **299, 300** | **never** |
| `[fabric-publisher] publish role received` | printed | never |

So the transfers land identically and the *receives* do not happen on the QoS
plane. Task 10 is parked, is owed nothing by the root (it is absent from the
`wedged waiter` list), and never performs the `recv` that would collect the
capabilities already delivered to it.

**The mechanism is now confirmed, and it is starvation rather than a lost wake.**
Three measurements settle it:

1. **Task 10 is not parked.** `live=5 parked=4`, and task 10 is the one live,
   unparked, unreclaimed task. Its `SYS_WAIT` *was* answered.
2. **The kernel says it is blocked sending.** Walking `ksDebugTCBs` with typed reads
   gives one child in `BlockedOnSend` and four in `BlockedOnReply`. The four are
   normal parked-awaiting-root; the one is a task whose call into the root never
   got received.
3. **The root never reaches it.** Logging the operation on the loop's final twelve
   passes gives `task=9 op=Recv` for eleven of them. The broker consumes the entire
   512-iteration budget, so task 10's send is still queued when the budget ends.

The broker's own loop is *not* spinning — instrumenting its `progressed` branch past
400 passes produced **zero** lines, and it emits
`[fabric] idle: parked on stream sources` **ten** times. So it parks, is woken,
serves a handful of `Recv` calls, and parks again — repeatedly. Each cycle costs
root iterations, and ten cycles at ~50 operations each is the budget.

So B28 is: **an always-ready wake source makes the broker cycle park→wake→park, and
those cycles starve the root's iteration budget before `fabric-publisher`'s queued
send is served.** That is the same class as the two park-set spins fixed in
`d69cd8e` — a third source is still permanently ready — and it explains why the
plane depends on one fixture field: `retained` diagnostics add the route whose
source never quiesces.

**The park set was printed, and it narrows the candidate to one slot without yet
convicting it.** Instrumenting `park_on_streams` to dump its contents gives, across
the ten cycles:

```
20 11 13 15 09  <- park set   (×5)
20 11 15 09     <- park set   (×3)
20 11 15        <- park set   (×1)
```

Slots 09 and 13 correctly drop out as their peers retire. **Slot 20 is present in
every set including the last.** It is the broker's half of channel key 22
(`endpoint minted task=9 key=22 slots=20,21`) — `fabric-publisher`'s route, the same
channel the wedge diagnostic reports the broker waiting to receive on.

**But that does not by itself explain the wake**, and the distinction matters:
`ChannelTable::is_ready` for a `Receive` target is `len != 0 || !peer_alive`, and
task 10 is alive and has sent nothing, so key 22 should be *not* ready. Being
present in the park set is not the same as being ready.

So the remaining question is one predicate on one channel: what makes the root
answer `wait` immediately when key 22 is in the set. Either `receive_ready` sees a
queued message that the broker's `recv` then fails to take, or the set contains a
second target on key 22 whose readiness differs — the broker pushes a publisher's
data slot and a subscriber's *ack* slot, and slot 21 is key 22's other half.

Thirty-one readings excluded. Every layer above this predicate is now measured:
task 10 answered and unparked, the kernel's per-thread states, the root's iteration
accounting, the broker's own loop not spinning, and the park set's exact contents. B28 stays open; the two park-set spins fixed under it
(`d69cd8e`) were real and are kept. B28 stays open; the two park-set spins fixed under it (`d69cd8e`) were real
and are kept.
Re-check this entry after B25 lands rather than investigating it further on its own.

**One wider finding stands regardless of B28**, and it is worth its own slice:
`sel4_transport::wait` returns `()`, so its staging-failure branch can only
`yield_now()` and return silently. It is unreachable on every current plane — hence
the zero lines above — but if it were ever reached it would convert a bounded error
into an invisible hang, exactly the signature that made this defect take seven
attempts to characterize. It should either report or be made impossible by
construction.

**Severity:** Blocks P5.4.5's exit condition and nothing else. Latent for every
other plane: no other seL4 graph declares two retained routes on one publisher.
The tradeoff is quantified — `retained` yields five observed C8.5 arms with
`fabric-publisher` parked, `volatile` yields three with it running, and neither
reaches the final marker — so the committed fixture keeps `retained` as strictly
more coverage.

**Exit condition:** With the diagnostics route `retained`, `fabric-publisher`
takes its role reply and the plane reaches `[init] fabric stream complete`,
asserted by a gate, with a fault injection showing the parked case caught.


### B12 — the component build's `--remap-path-prefix` names a path that does not exist

**Resolved 2026-08-07.** Devlog:
[`devlog/2026-08-07-b12-component-remap/`](../devlog/2026-08-07-b12-component-remap/index.md).
The hardcoded literal is gone from `components/.cargo/config.toml`;
`build-rust-components` now appends `--remap-path-prefix={ROOT}=.` for triple
targets through `--config`, mirroring what the JSON-target branch already did
through `RUSTFLAGS`.

**`--config` and not `RUSTFLAGS`, which is the whole difficulty.** Setting
`RUSTFLAGS` *replaces* the config's rustflags rather than adding to them, so it
would have silently dropped `relocation-model`, `code-model`, and three link args
the x86 link depends on. The JSON branch can set `RUSTFLAGS` freely only because a
JSON target inherits none of those to begin with.

**Two corrections to this entry, both material.** First, the checkout is now
`/Users/iceice666/code/slime_os`, so the stale literal is not even a *prefix* of
the real path — the mangling this entry describes stopped happening at some point
and the flag became an outright no-op. Second, and more importantly, **the
severity was overstated**: these are release builds, and the x86 component ELFs
embed *zero* absolute source paths (`strings … | grep -c '/Users/iceice666'` is 0
for every component). So the flag had nothing to remap either way.

That is why the deferral's central fear — that fixing this would alter every
component ELF and therefore every generation identity the oracle's gates assert
against — turned out to be empty. Measured directly: the generation identities
before and after the fix are **byte-identical**
(`df40ce7a…13e5`, `ebdf06d0…b092`), `just generation_check` passes, and the seL4
channel, stream, and component-graph plane gates are unaffected.

**Exit condition partially met, and the remainder is now argued rather than
observed.** Two builds from two different checkout directories were *not* run —
that needs a second clone, which this environment cannot usefully provide. What
was established instead is stronger than the original worry and weaker than the
original exit condition: the flag is no longer wrong, it is computed from the
actual root, and the artifacts it guards contain no paths for it to affect. If a
future build turns on debug info for components, the flag becomes load-bearing and
the two-checkout comparison becomes worth running for real.

**Problem:** `components/.cargo/config.toml` passes
`--remap-path-prefix /home/iceice666/projects/slime_os=.` for both the
`x86_64-unknown-none` and `aarch64-unknown-none` targets. The current checkout is
`/home/iceice666/projects/slime_os-sel4-cutover`. Because the stale literal is a
*prefix* of the real path, the flag does not simply miss: it rewrites the leading
portion and leaves `-sel4-cutover/...` behind, so recorded paths are mangled
rather than normalized, and a checkout at a different directory still produces
different bytes.

The determinism claim this flag exists to support is therefore weaker than it
reads. `just generation_check` still passes, because it builds twice from *one*
checkout — the property it verifies is reproducibility across runs, not across
source paths. `build-sel4.py` closes the same leak properly for the kernel with
`-ffile-prefix-map` onto fixed logical roots (`/slime/sel4`, `/slime/build`), and
P5.1's devlog records two builds from different source paths as byte-identical
on that path.

**Evidence:** `components/.cargo/config.toml:11` and `:21` against `pwd`. Noted
while adding the seL4 target in P5.2; see
`devlog/2026-08-04-p5-2-native-component-images/`.

**Proposed fix:** remap from the repository root as computed at build time rather
than from a hardcoded literal — the builder already knows it (`ROOT` in
`scripts/build/build-generation.py`), and the seL4 path passes
`--remap-path-prefix={ROOT}=.` explicitly for exactly this reason. Deciding
whether the mapped-to token should match `build-sel4.py`'s `/slime/...`
convention is part of the fix.

**Why deferred rather than fixed in P5.2:** changing the frozen x86 oracle's
build inputs alters every component ELF it produces, and therefore the
authenticated identity of every generation the oracle's gates assert against.
That is a larger blast radius than the defect, and it is orthogonal to native
seL4 component images. The seL4 target is unaffected: it inherits none of these
rustflags (they are keyed by triple) and passes its own.

**Exit condition:** two builds of the same generation from two different
checkout directories produce byte-identical component images and the same
generation identity, with `just generation_check`, `just product_boot_check`,
and `just test` unchanged.

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate**, on the same
reasoning: that slice replaces the seventh seL4 generation through the same
build path, whose rustflags are keyed by triple and match none of the stale
literal's. See `devlog/2026-08-05-p5-5-2-stream-plane/`.

**Deferral re-reviewed 2026-08-05, before opening P5.5.1's gate**, on the same
reasoning: that slice adds a seventh seL4 generation through the same build
path. See `devlog/2026-08-05-p5-5-1-typed-fabric/`.

**Deferral re-reviewed 2026-08-05, before opening P5.3.4's gate**, on the same
reasoning: that slice adds a sixth seL4 generation through the same build path,
whose rustflags are keyed by triple and match none of the stale literal's. See
`devlog/2026-08-05-p5-3-4-sample-plane/`.

**Deferral re-reviewed 2026-08-05, before opening P5.3.3's gate**, on the
reasoning recorded below: that slice adds a fifth seL4 generation through the
same build path, whose rustflags are keyed by triple and match none of the stale
literal's, so it neither touches the defect nor extends its reach. See
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Deferral re-reviewed 2026-08-04, before opening P5.3.2's gate** on the same
reasoning: that slice adds a fourth seL4 generation through the same build path,
so it neither touches the defect nor extends its reach. See
`devlog/2026-08-04-p5-3-2-loan-plane/`.

**Deferral reviewed 2026-08-04, before opening P5.3.1's gate.** Still deferred,
on the reason recorded above rather than by omission. B12's own analysis
establishes that the seL4 target is unaffected: `components/.cargo/config.toml`
keys its rustflags by triple, the seL4 component build matches none of them
(it uses a JSON target specification), and `build-generation.py` passes
`--remap-path-prefix={ROOT}=.` explicitly on that path for exactly this reason.
P5.3.1 adds a second seL4 generation built through that same path, so it neither
touches the defect nor extends its reach. Fixing it still means rebuilding every
frozen x86 component image and re-authenticating every generation identity the
x86 gates assert against — a blast radius larger than the defect, and orthogonal
to the seL4 cutover. It should be scheduled against the x86 oracle deliberately,
not folded into a portability slice.

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's gate.** Still
deferred, on the same reasoning. B16's fix adds an eighth seL4 generation and a
new component binary built through the same JSON-target path, which the
rustflags this defect concerns do not match, so the reach is unchanged once
again. `just generation_check` and `just contracts_check` were run to confirm
the new binary perturbed neither contract validation nor generation identity.
See `devlog/2026-08-07-b16-supervision-records/`.

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's own gate.** Still
deferred, on the same reasoning once more. B22's fix adds a ninth seL4
generation and a new component binary through the same JSON-target path, whose
rustflags this defect does not match, so the reach is unchanged.
`just generation_check` and `just contracts_check` were run to confirm the new
binary perturbed neither contract validation nor generation identity. See
`devlog/2026-08-07-p5-4-1-oracle-inventory/`.

### B30 — `release_trust_check` was red, unregistered, and its rotation refusals never reached Rust

**Resolved 2026-08-07.** Devlog:
[`devlog/2026-08-07-b30-release-trust-gate/`](../devlog/2026-08-07-b30-release-trust-gate/index.md).
Observed exit condition: `just release_trust_check` passes, is listed in
`AGENTS.md`'s gate index, and each rotation continuity branch is guarded by its
own fixture — removing the replacement check fails with
`apply_rotation accepted version-skip`, removing the previous check fails with
`apply_rotation accepted stale-previous`.

**Problem:** three separate defects in one gate, found by running it.

1. **It could not run at all.** `scripts/lib/release_trust.py` re-exports generated
   constants from `boot_contracts`, but never imported `ROTATION_BYTES`,
   `ROTATION_HEADER_BYTES`, `ROTATION_MAGIC`, `ROTATION_VERSION`, or
   `MAX_TRUST_KEYS`. `just release_trust_check` died with
   `AttributeError: module 'release_trust' has no attribute 'ROTATION_BYTES'`
   before asserting anything, so all thirteen of its `expect_error` cases were
   dead code.
2. **It was not in the gate index.** `AGENTS.md:61-77` is canonical, and this
   target was absent — which is why a red gate went unnoticed.
3. **Its rotation refusals tested Python, not the kernel's decoder.**
   `verify_rotation` (`check-release-trust.py:181`) is a pure-Python
   reimplementation of the same rules. Only the *valid* rotation was ever handed
   to `apply_rotation` through the `verify_release` example, so all three
   continuity assertions proved the fixture was malformed, never that
   `release.rs` refuses it.

**Evidence:** with the import fixed, deleting the
`replacement_version != current.version + 1` branch from `apply_rotation` left
the entire gate **green**. So did deleting `previous_version != current.version`.

**Fixed.** The four rotation constants and `MAX_TRUST_KEYS` are now loaded from
`boot_contracts` directly in the check (`CONTRACTS`), rather than widening
`release_trust`'s imports to names its own body does not use — which ruff
correctly flags as F401. Every rotation refusal now goes through
`apply_rotation` as well as the Python mirror, via
`expect_rust_rotation_refused`. A fixture for stale `previous_version` was added,
because the two existing continuity cases vary the *signature counts* and never
reach that branch.

The new fixture is `(previous=2, replacement=2)` and not `(2, 3)`: the
replacement version must stay at `current.version + 1` or the replacement branch
fires first and masks the branch under test. Getting that wrong is why the first
attempt at this fixture still passed under injection.

**Exit condition met.** `just release_trust_check` passes, is registered in
`AGENTS.md`, and each continuity branch is now guarded by its own fixture:
removing the replacement check fails with `apply_rotation accepted version-skip`,
removing the previous check fails with `apply_rotation accepted stale-previous`.
Both observed, then reverted.

**One guard attempted and deliberately not shipped.** `apply_rotation`'s
`replacement.validate()?` still has no fixture that isolates it: deleting the call
leaves the gate green. Two candidate fixtures were built and both failed to
discriminate, because the signature loop rejects them first —
`verify_signature_entries` resolves each key-id by `sha256(key)` against
`root.keys[..key_count]`, so any replacement root malformed enough to fail
`validate` also fails to match a signature. Shipping a fixture that passes with
and without the call would have looked like coverage while proving nothing, so it
was reverted rather than committed.

**A third attempt established *why*, and the answer is that the call is
redundant on this path rather than untested.** `build_rotation` was
parameterised to take the replacement threshold, and a fixture built with a
correct two-key set and `threshold = 3` — signature-valid, `validate`-invalid.
It is still refused with the call deleted, because
`verify_signature_entries` independently returns `MissingSignatures` when
`count < root.threshold`, and the replacement root is passed to it immediately
after. Every malformation `TrustRoot::validate` catches is therefore also caught
downstream on this path:

* threshold above key count → `count < threshold` in `verify_signature_entries`
* zero, duplicate, or trailing keys → no `sha256(key)` matches the entry's key-id

So `replacement.validate()?` is defence in depth, not a live guard, and no
black-box fixture can distinguish its presence. Three candidate fixtures were
built and all three were reverted rather than committed, because a test that
passes with and without the code it names looks like coverage while proving
nothing.

**Recorded as accepted, not open.** The honest statement is that `validate` is
directly covered by the fifteen `TrustRoot::validate` unit tests in
`boot-contracts/src/release.rs`, and its use inside `apply_rotation` is
unreachable-by-construction given the checks that follow it. If that ordering ever
changes — if a future `apply_rotation` uses the replacement root before signing —
the call becomes load-bearing and will need the fixture this note describes.

### B29 — `ParkedReplies::wake` never deleted the reply CSlot it counted as recycled — **resolved 2026-08-07**

**Problem:** `slime-root/src/parked.rs` has three paths that finish with a saved
reply capability, and only two released it. `answer_saved` and `discard` both go
through `release_slot`, which calls `delete_slot` *and* bumps `recycled`. `wake`
— the path every parked task takes — called `send_reply` and then bumped
`recycled` directly, with no `delete_slot`. So each parked wake left a root CSlot
holding a spent reply capability while reporting it as recycled.

**Found by** reading the three paths side by side while chasing B28. Not by a
failure: the boot's own counters cannot see it. `recycled` was already
incremented, so the terminal `replies=` figure is identical before and after the
fix (323 on the QoS plane both ways), and `tasks reclaimed … slots=` is unchanged
too (517). That is exactly what makes it worth recording — the accounting said
"recycled" and the CSlot was still occupied, so the number that exists to prove
the save path is not a leak was the number hiding one.

**Severity:** Latent, and bounded per boot rather than per operation only because
the graphs are short-lived. A long-running graph that parks and wakes repeatedly
consumes one root CSlot per wake with nothing reclaiming it; the QoS plane alone
parks 33 times. It is the same shape as B22, B23, and B24 — a table with no free
path — one level down, in the allocator rather than a table.

**Resolved by** `wake` calling `release_slot(held.slot)` after `send_reply`,
which is the path the other two already took. `recycled` is bumped by
`release_slot`, so the counter's meaning is now uniform across all three.

**Exit condition observed.** All nine seL4 plane gates, `sel4_boot_layout_check`,
and `test_sel4_root` (109/109) pass with the fix; the five C8.5 arms on the QoS
plane are unchanged. The counters are identical by construction, so the guard
against regression is that all three paths now call one function — a future
fourth path leaks only by not calling it.

### B27 — the manifest→flag table set and scrubbed in one pass, so two manifests could not share a flag — **resolved 2026-08-07**

**Problem:** `build_sel4_generation`'s manifest→flag loop
(`scripts/build/build-generation.py`) set the selected manifest's flag and
popped every other manifest's in the same iteration. With one flag per manifest
that is correct. The moment two manifests declare the same flag it is not: a row
later in the table pops what an earlier row set, and which one wins depends on
table order rather than on the selection.

**Found by** P5.4.5's QoS plane, which is the stream driver plus a clock and so
declares `SLIME_SEL4_STREAM_CHECK` alongside the oracle's
`SLIME_FABRIC_QOS_CHECK`. Adding the `sel4-qos` row *after* `sel4-stream`
cleared the stream plane's own flag, and `just sel4_stream_check` failed with
`boot exceeded 180s without reaching the final marker` — init fell through to
`[init] launching component graph` and spawned nothing. Observed directly, and
worth recording because the failure is a timeout rather than an error: nothing
said "flag missing", and the plane simply ran a different composition.

**Resolved by** collecting the selected manifest's flags into one set and every
flag the table declares into another, then setting the first and removing the
rest. A flag two manifests share now survives for whichever asked for it,
independent of row order.

**Exit condition observed.** `just sel4_stream_check` passes with the
`sel4-qos` row present, and the QoS plane's own boot shows both flags in effect
— it runs `drive_stream_plane` and its components take the QoS path. All nine
seL4 plane gates pass with every image rebuilt. See
`devlog/2026-08-07-p5-4-5-qos-clock/`.

### B26 — the `[layout]` dump reported the grant's rights, so a too-permissive layout row was unobservable — **resolved 2026-08-07**

**Problem:** `slime-root/src/main.rs` printed each layout row's rights from the
*installed capability*, which `launch_component_graph` fills from the
**generation grant**, rather than from the boot-layout entry the row exists to
freeze. `bootstrap_executable_slot` and `bootstrap_slot` test *containment*
(`rights & !entry.rights != 0`) rather than equality, deliberately and
correctly — a layout marks a channel half `RIGHT_TRANSFER` because init hands
it on, while the grant is not about delegation at all, and requiring equality
rejected a well-formed graph once already. So the two legitimately differ, and
a dump carrying only one of them could not show a layout declaring strictly
more authority than anything uses. B10 exists to keep the table that declares a
slot and the table that fills it in agreement; this was the one direction of
disagreement the gate was blind to.

**Found by** fault-injecting P5.4.6's call plane: changing
`SEL4_CALL_LAYOUT`'s `fabric-call-server` row from `0x10008` to `0x1000c`
rebuilt the generation to different bytes (verified by md5) and the gate still
passed, while swapping two slot *numbers* in the same table was caught
immediately. That contrast is what localized the gap to rights.

**Resolved by** `declared_layout_rights`, which resolves the layout entry
behind a bootstrap row — by identity for an executable, by role for the two
singular factories — and appends `declared=0x…` when it differs from the
installed value. Appended and only on disagreement, so every row that agrees
keeps the retired kernel's exact four fields and stays comparable to
`dump_boot_layout`'s output slot for slot. `check-sel4-boot-layout.py`'s
`ENTRY` pattern admits the optional tail.

A channel end is deliberately not covered: it is named by its *grant*, and one
capability can be reached by more than one grant name, so reporting a declared
value would mean picking one. Executables and the two factories are where a
layout row's rights are unambiguous, and they are the rows a layout edit
touches.

**Exit condition observed.** The previously-invisible `0x10008`→`0x1000c`
injection now fails the gate, reporting
`now: [layout] 5 executable fabric-call-server 0x10008 declared=0x1000c`
against the frozen row. Restored and re-verified green.

The fix immediately earned itself: re-blessing surfaced three *pre-existing*
disagreements nothing had ever reported — `sel4-loan`, `sel4-sample`, and
`sel4-stream` each declare `0x1000004` on their shared-buffer-factory row while
the root installs `0x1000000`. Those are legitimate containment differences,
now recorded rather than invisible. See
`devlog/2026-08-07-b26-layout-declared-rights/`.

### B24 — `SharedBufferTable::quotas` never reclaimed, so `MAX_CHARGE_HOLDERS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's and B22's defect shape in a third table, and the one B16's
sweep implicitly cleared. `slime-root/src/shared_buffer.rs:502` declares
`quotas` one line below `charges`, which B16 named among the correct tables.
`charges` **is** correct — `uncharge` frees it at `:1782-1784`. `quotas` had no
free path anywhere: `declare_quota` reuses a slot only for the same `HolderId`
and otherwise takes a fresh one, while `commit_teardown`, `reclaim_holder`, and
`advance_epoch` never mentioned it. Because `construct_child` keys it by task id
and `TaskTable::next_id` never rewinds, a spawn/reap graph presented a fresh
holder every time and the 96 slots bounded the holders a boot could **ever**
construct.

Found by P5.4.1's lifetime-vs-live class audit rather than one at a time, which
is the reason that audit was scoped as a class: `quotas` is *keyed* per-task but
*declared* per-component at boot, so it does not read as a per-task table at a
glance and B16's per-task sweep passed over it.

**Resolved by** `release_quota`, called from `reclaim_dead_task` after charge
settlement — the ceiling outlives every charge made against it and is dropped
only once nothing can be charged again. A **direct release rather than a derived
sweep**, unlike B16 and B22: a quota has exactly one holder and that holder is a
task, so "the task is gone" is complete information. Those two needed predicates
because a supervision handle or a channel end can be named by a capability that
outlives the task; a quota cannot.

**Exit condition amended, and why.** The condition recorded when this item was
opened asked for a graph constructing more than `MAX_CHARGE_HOLDERS` holders.
That is unreachable: root CSlots are deliberately never returned
(`task.rs:165-167`), and the supervision plane's 35 spawns consume 2321 of 3457,
so a boot exhausts CSlots near 52 tasks and cannot reach 97. Stretching the
evidence to fit the original wording would have been the wrong move; the
condition is restated to what the platform can carry.

**Exit condition (observed 2026-08-07):** every constructed holder releases its
declared ceiling when its task dies, observed under `just sel4_supervision_check`
— 38 holders constructed over one boot, 38 `SLIME_GRAPH quota released` lines,
and `quotas=0` on the terminal accounting — and fault-injected to show that
disabling the release leaves `quotas=38`. Asserted on that existing plane rather
than a tenth image, since it is already the deepest spawn/reap loop in the
corpus. See
[`devlog/2026-08-07-b24-shared-buffer-quotas/`](../devlog/2026-08-07-b24-shared-buffer-quotas/index.md).

**Follow-up recorded, not opened:** root CSlot non-reuse is now the binding
lifetime constraint on graph longevity, ahead of every table this class audit
examined. Deliberate and documented rather than a defect, but P5.4.1 classified
it as acceptable-monotonic without quantifying it.

### B23 — `slime-root`'s unit tests were run by no gate — **resolved 2026-08-07**

**Problem:** 102 `#[test]` functions across 13 modules were compiled by nothing
and run by nothing, while `slime-root/src/main.rs` described those modules as
"bounded, pure, and unit-tested in place". Two independent blockers: no Justfile
target named the crate, and it could not have run anyway — `main.rs` is
unconditionally `#![no_std]`/`#![no_main]`, the package declared no lib target,
and the crate built only for a seL4 JSON target with no `libtest`.

**Resolved by** splitting the mechanism modules into a `slime_root` library the
binary links, rather than a `cfg(test)` escape (which neither blocker admits) or
a separate test crate (whose passing tests would be evidence about a copy). The
`sel4` crate builds for a host target given `SEL4_PREFIX`, so nothing had to be
excluded: all 13 covered modules run, including the seL4-touching ones.
`sel4-root-task` is scoped to `cfg(target_os = "none")` because it pulls
`sel4-alloca`, whose inline ELF section directive will not assemble on Mach-O;
only the binary needs it and the seL4 build is unchanged.

**What the first run found, which is the point:** three latent defects, every
one a test silently wrong since something changed under it. Nine `push` call
sites had been stale since P5.3.2 added a `transferable` parameter. An
`elf_header` fixture was 20 bytes against `LEGACY_HEADER_LEN`'s 32, so it had
been asserting `Unrecognized` rather than the bare-ELF arm ever since
`component_image::target` gained its length guard. A `qualified` fixture sized
its tail with a literal that no longer matched. All three are test bugs rather
than production bugs — the good case, but not evidence that nothing was hiding.

**Exit condition (observed 2026-08-07):** `just test_sel4_root` runs 102 tests
across 13 modules and asserts the count, so a module that stops being covered is
visible. It is a gate of its own rather than a `test_host` arm, because it needs
the installed seL4 prefix that `test_host`'s CI runner does not build — the same
reason `lint_sel4_root` stands apart. Fault-injected by removing one `transit`
test: the gate fails with `ran 101 tests, expected 102`. The nine seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged, so the lib
split did not disturb the image. See
[`devlog/2026-08-07-b23-slime-root-host-tests/`](../devlog/2026-08-07-b23-slime-root-host-tests/index.md).

**Noted, not fixed:** `just test_host`'s `slime-proto` arm pins
`x86_64-unknown-linux-gnu` and therefore fails on an `aarch64-apple-darwin`
host, which was true before this change and is confirmed by stashing it.
`test_host` is left untouched — this fix adds no arm to it, and
`test_sel4_root` uses the host triple.

### B22 — `ChannelTable` never reclaimed, so `MAX_CHANNELS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's exact defect shape in a second table.
`slime-root/src/channel.rs` never freed an entry: `push` derived its key as
`self.len` (`:446`), `mark_dead` (`:339-354`) marked both queues of a dying
task's channels dead but freed nothing, and `reassign` only rewrote the holder
fields. So `MAX_CHANNELS` (32) bounded the channels a boot could **ever** mint,
not those live at once, and every channel a long-running graph minted was spent
permanently.

**How it differed from B16, and why that changed the fix's evidence:** B16
dropped a record *silently* and hung the parent, so converting the failure into
a reported one was part of its fix. B22's was already a bounded refusal —
`ChannelError::TableFull` becomes `IpcError::DestinationSlotsExhausted` — so
"the failure became reportable" proves nothing here. The gate could only be
satisfied by the graph *succeeding* past 32. The downstream symptom was the real
cost: a refused `mint` surfaces in the component, and at `MAX_CHANNELS = 16` the
stream plane's exhaustion "read as four broken components rather than one
exhausted table" (`channel.rs:107-111`). The bound had already been crossed once
and raised rather than fixed.

**Resolved by** `channel::sweep(&mut ChannelTable, &GraphTables, &Transit)`,
which frees every entry no live holder can name — derived from state that
already exists, exactly as `supervision::sweep` is. Two predicates, not one:
`GraphTables::holds_endpoint` for the live half and `Transit::holds_endpoint`
for the in-flight half, because `serve_cap_transfer` drops the capability from
the sender's table *before* parking it, so a sweep reading only the graph would
free the channel a transfer is mid-way through moving.

A precondition came with it: `key = self.len` had to become a monotonic
`next_key`. That derivation is unique only while `len` never decreases — once
the sweep frees an entry, the next `push` would reissue a key some live
capability already names, and `Resource::Endpoint { channel }` is the only
handle a component holds. That would have converted an exhaustion bug into
confused-deputy redirection, which is strictly worse.

The sweep is lazy, firing on `TableFull` and retrying, for B16's reason: one
trigger condition is one thing to keep correct, and a channel that stays is a
channel that still works.

**Exit condition (observed 2026-08-07):** `just sel4_crossing_check` boots a
graph that mints 33 pairs against a 32-entry table and still sends and receives
on every live channel, including a pair held across the crossing and an end
parked in `Transit` across it. The transcript records the first sweep as
`freed=28 live=4 minted=32` and the terminal line as `minted=37`; what the gate
*asserts* is looser and deliberately so — a nonzero `freed` on the sweep line
and a terminal `minted` in 33..=99, since pinning exact counts would break on
unrelated allocator changes while the loop-vs-bound arithmetic is enforced
separately from source. Three fault injections confirmed failing:
removing the sweep dies at the 33rd mint, removing the `Transit` half of the
predicate loses the in-flight end, and restoring `key = self.len` trips the
gate's key-derivation source check. The other eight seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged. See
[`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md).

**Follow-up opened:** [B24](#b24--sharedbuffertablequotas-never-reclaims-so-max_charge_holders-is-a-lifetime-bound),
a third table of the same shape found by the same class audit.

### B21 — the toolchain was pinned by name, so each host resolved a different binary — **resolved 2026-08-06**

**Problem:** `flake.nix` pinned the seL4 cross toolchain by *name*
(`CROSS_COMPILER_PREFIX = crossCC.targetPrefix`), and `build-sel4.py` passed
that bare prefix to CMake, which resolves `${prefix}gcc` through `PATH`. A name
is not an identity. `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *cross*
wrapper on `aarch64-darwin` and `x86_64-linux` but a *native* wrapper on
`aarch64-linux`, where `targetPrefix` is empty and `bin/` contains no
`aarch64-unknown-linux-gnu-`-prefixed entry. The prefixed lookup therefore
skipped that wrapper and found the **unwrapped** GCC its own `setup-hook` had
put on `PATH` — a different compiler driver *and* a different assembler,
selected by `PATH` order rather than by anything pinned.

**This corrects B20's recorded root cause.** B20 attributed the divergence to
Darwin's wrapper injecting `-fno-omit-frame-pointer` where `aarch64-linux`
"forces neither". Both wrappers ship a byte-identical
`nix-support/cc-cflags-before`; nixpkgs emits it for every non-x86-32,
non-s390 target. B20's two pre-fix hashes, `e8cbab4f…` and `f2d316e1…`, differ
by *driver*, not by host: both are reproducible on one machine by choosing the
wrapped or unwrapped compiler.

**Resolved by** exporting `CROSS_COMPILER_PREFIX` as an absolute
`"${crossCC}/bin/${crossCC.targetPrefix}"` store path, so every host runs the
same driver and assembler. This is the fix B20 proposed and rejected as
"larger, with a worse failure mode"; that rejection rested on a false premise.
`crossCC` is the same derivation each platform already evaluates and installs,
so nothing new is fetched and no pinned hash moves. `just sel4_pin_check` now
fails if the bare form returns — the prefix pin cannot catch this itself, since
it reports "toolchain drift" without naming which host is odd.

B20's `-fomit-frame-pointer -momit-leaf-frame-pointer` are **kept**. Fault
injection shows they close a *different* leak than the one B20 recorded: with
the toolchain pinned but the flags removed, the hosts still diverge in
`.debug_line` alone (`e8cbab4f…` vs `4c694979…`, both 982208 bytes, every ALLOC
section equal), because GAS's DWARF-5 view numbering for the extra prologue row
is not host-independent. That binutils behavior is masked, not fixed.

**Exit condition (observed 2026-08-06):** `kernel.elf` rebuilt from scratch on
`aarch64-darwin` and `aarch64-linux` is `97dcb029…`, 973184 bytes on both —
**unchanged** from the recorded pin, now depending on the toolchain rather than
on `PATH`. `CROSS_COMPILER_PREFIX` resolves to the wrapper on `aarch64-linux`
instead of being empty. `just sel4_qemu_image_check` passes on `aarch64-darwin`,
and the new guard is fault-injected: reverting to `crossCC.targetPrefix` fails
`just sel4_pin_check`. `x86_64-linux` was not re-observed; its prefix was
already the cross form, so the change is expected to be a no-op there
(**[INFERENCE]**). Both hosts are on one machine, one virtualized — the right
test for toolchain and `PATH` independence and no evidence about physical
boards. See `devlog/2026-08-06-b21-cross-toolchain-binary-selection/`.

### B16 — a supervision termination record was never reclaimed, so a long-lived graph exhausted the table — **resolved 2026-08-07**

**Problem:** `slime-root/src/supervision.rs::Terminations` records how each child
ended and never removes the record, because two parents may hold handles to one
child and each is owed the answer. `MAX_RECORDS` is `MAX_TASKS` (32), which
bounds the tasks *alive at once* — but `TaskTable::reclaim` frees its entries
while `TaskId`'s `next_id` keeps counting, so a graph that spawns and reaps
repeatedly creates far more than 32 tasks while never holding more than a few.

Past the bound, `record` drops silently and every later
`supervision_status` on that child answers `WouldBlock` forever: the
parent-waits-forever failure the module exists to prevent, arriving by the
module's own bookkeeping rather than by a missed wake. The retired kernel's
`sched.terminated` is an unbounded `Vec` and has no equivalent limit.

Not reachable by any declared seL4 generation — each creates a handful of tasks
and exits — so it is a latent bound rather than an observed defect.

**Evidence:** `supervision.rs::MAX_RECORDS` against `task.rs::TaskTable::reclaim`,
which decrements `len` but not `next_id`. Noted in the P5.3.3 review; see
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Proposed fix:** reclaim a record once every holder of a handle naming that
child has collected or dropped it, which needs a reference count incremented at
each `Supervision` capability install and decremented at each collect, drop, and
table release. Alternatively fail the *spawn* when the record table is full,
which turns a silent wrong answer into a bounded refusal at the point of
allocation — the same shape `construct_child` already uses for `MAX_GRAPH_TASKS`.

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate.** Still
deferred, on the same observation, and this is the largest graph the cutover
declares: P5.5.2's stream plane creates thirteen tasks — seven launched, six
spawned — against `MAX_RECORDS = 32`. The bound is approached more closely than
by any earlier slice and still not reached. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

Worth stating plainly, since the margin is now under 3×: this stays a latent
bound rather than a defect only because every declared generation runs to
completion and exits. A long-lived graph that spawns and reaps repeatedly is
what makes it bite, and P5.4 — which retires the oracle — is the point at which
"every declared generation" stops being a safe quantifier.

**Why deferred rather than fixed in P5.3.3:** the counting version touches every
path that installs or releases a capability, and the refusal version needs a
gate whose graph spawns past the record table to prove it. Neither is a line;
both want the multi-child graph P5.3.4 composes.

**Exit condition (observed 2026-08-07):** a graph that creates more than
`MAX_RECORDS` tasks over its lifetime still answers `supervision_status`
correctly for every live handle, observed under `just sel4_supervision_check`,
with the nine existing seL4 gates passing. (The entry said *five*; there were
nine by the time it was closed.)

**Fix: a derived sweep, which is neither option this entry proposed.** The
refusal was rejected on the entry's own terms — refusing the spawn makes the
graph the exit condition requires impossible to observe, so choosing it would
mean amending the condition in the same change that claimed to meet it. The
reference count was unnecessary: the live-holder set is already represented, so
`supervision::sweep` derives it, reclaiming every record no live holder can
name. Same choice, same reason, as `TaskTable::live_children`, and it fails
safe — a sweep that does not run leaves a record that still answers correctly,
whereas a missed decrement loses one forever.

The predicate reads **two** holders. A supervision handle in flight is held by
no capability table at all, so a sweep consulting only `GraphTables` would free
a record mid-transfer and leave the receiver waiting forever: this defect,
reintroduced by its own fix. `Transit::holds_supervision` is the second half,
and fault injection #2 below is what proves it is load-bearing.

The residual case is now reported rather than silent: if every record has a live
holder, `record_termination` emits
`SLIME_GRAPH FAIL termination lost task={} reason=records-full`, matching
`unland_caps`'s convention. That is what closes the *silent*-loss defect rather
than merely raising the bound.

**Observed:** 35 tasks created over one boot, `terminated=38` against
`MAX_RECORDS = 32`, with `freed=30 live=3` at the sweep — the retained handle,
the in-flight handle, and the current record all preserved. Two fault
injections, both confirmed failing: removing the sweep fails at
`termination lost task=33 reason=records-full`; removing only the `Transit` half
of the predicate fails at `a handle parked across the crossing lost its
outcome`, with every earlier marker still passing. See
`devlog/2026-08-07-b16-supervision-records/`.

### B20 — the prefix pin held for one platform at a time — **resolved 2026-08-06**

**Problem:** B19 made `kernel_sha256` independent of the dev *shell*; it was
still per-*platform*. `aarch64-darwin` produced `e8cbab4f…` and `aarch64-linux`
produced `f2d316e1…` from the same checkout, the same `flake.nix`, and the same
pinned seL4 source and config.

The cause was the toolchain, not a leak. `flake.nix` names
`pkgsCross.aarch64-multiplatform.stdenv.cc`, which resolves to a **cross**
`gcc-wrapper` on Darwin and a **native** `gcc` on `aarch64-linux` — the
empty-`targetPrefix` fact B19's analysis recorded, seen from the other side.
Darwin's `nix-support/cc-cflags-before` forces
`-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`, so every function
prologue differed. Because that file lives inside the wrapper derivation rather
than the environment, B19's scrub could not reach it.

**Resolved by** having the build state its own frame-pointer policy:
`-fomit-frame-pointer -momit-leaf-frame-pointer` joins the prefix maps and the
fixed seed in `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS`. This is a policy the build
**chooses**, not a compiler default it restores, and it moves *both* platforms:
GCC's aarch64 backend disables `-fomit-frame-pointer` at every `-O` level, so an
aarch64 kernel keeps its frame pointers at `-O2` unless the flag is explicit.
(`-Q --help=optimizers` claims otherwise at `-O2`; that is a reporting trap, and
it is what an earlier draft of this entry got wrong.) The choice is sound because
seL4 states no frame-pointer preference and nothing walks one — the AArch64 trap
path's `x29` uses are full register-context saves indexed off `sp`, and
`Arch_userStackTrace` scans `SP_EL0` linearly. `-momit-leaf-frame-pointer` is
belt and braces: under `-fomit-frame-pointer` it changes no emitted code, and it
is kept only because it names the second of the wrapper's two injections.

Darwin's two other injections need no counter-flag: `-march=armv8-a` is what seL4
passes itself and what both compilers default to, and the glibc/gcc
`-idirafter`/`-B` paths reach nothing in a `-nostdinc -ffreestanding -nostdlib`
build.

Naming one cross toolchain for every system — B20's own proposed fix — was
rejected as larger, with a worse failure mode, and moving the pin for a reason
unrelated to the defect. It remains the stronger fix and is now optional.

`kernel_sha256` is re-observed as `97dcb029…` on **all three platforms tested**.

**Exit condition (observed 2026-08-06):** `kernel.elf` built on
`aarch64-darwin`, `aarch64-linux`, and `x86_64-linux` are **byte-identical** by
`cmp`, each 973184 bytes at `97dcb029…`, from three different dev-shell seeds
(`r279wlb3cq`, `65gzz0x3v8`, `6ckb6q72lb`), with all nine `sel4_*` Justfile gates
passing. `x86_64-linux` is the case that matters most:
there `pkgsCross.aarch64-multiplatform.stdenv.cc` is a genuine *cross* wrapper as
on Darwin, rather than the native `gcc` `aarch64-linux` resolves, so both wrapper
shapes agree. B19's property still holds on each: a real-shell build and a
hostile-environment build are byte-identical. Fault-injected symmetrically —
replacing the flag string with `""` reverts Darwin to `e8cbab4f…` and
`aarch64-linux` to `f2d316e1…`, the exact pre-B20 divergence. Both Linux hosts
are containers under a macOS hypervisor, one of them emulated, not separate
hardware — the right test for toolchain independence and no evidence about
physical boards. See `devlog/2026-08-06-b20-cross-platform-kernel-identity/`.

**Root cause superseded by B21 (2026-08-06).** The mechanism recorded above is
wrong. Both wrappers ship a byte-identical `cc-cflags-before`; the divergence
was `PATH`-order *binary* selection, not a per-platform wrapper policy, and the
two pre-fix hashes differ by driver rather than by host. The "stronger fix …
now optional" is implemented and moved no hash. The frame-pointer flags are
kept, for a residual `.debug_line` leak this entry did not identify. See the
B21 entry above and
`devlog/2026-08-06-b20-cross-platform-kernel-identity/index.md`'s
`## Corrections`.

### B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain — **resolved 2026-08-06**

**Problem:** `sel4/pins.toml`'s `[observed_prefix]` is the gate that would
notice a change of seL4 compiler, and it pinned the **dev shell's own derivation
hash** instead. `configure_and_install_sel4` inherited `os.environ`, and nixpkgs
puts `-frandom-seed=<first 10 chars of the devShell derivation hash>` into
`NIX_CFLAGS_COMPILE`; GCC seeds symbol and section naming from it, so adding a
tool to `flake.nix` — or reordering the list — changed `kernel.elf` byte-for-byte
and was reported as toolchain drift. The same variable carried per-package
`-isystem` store paths, and `NIX_HARDENING_ENABLE` imposed
`-fstack-protector-strong`, `-fzero-call-used-regs`, and `_FORTIFY_SOURCE=3` on a
freestanding kernel whose own `CMakeLists.txt` asks for `-fno-stack-protector`.

**Resolved by** making the kernel build independent of the shell rather than by
re-pinning per host. `sel4_build_environment` builds the environment from
`os.environ` minus every flag-carrying `NIX_*` variable, the `CFLAGS`-family
names CMake seeds `CMAKE_<LANG>_FLAGS_INIT` from, the bintools wrapper's
`NIX_SET_BUILD_ID`/`NIX_BUILD_ID_STYLE` switches, and
`CMAKE_INCLUDE_PATH`/`CMAKE_LIBRARY_PATH`/`CMAKE_PREFIX_PATH`; a fixed
`-frandom-seed=slime-sel4-qemu-arm-virt` replaces the shell's seed. The scrub
matches by *prefix* because the cc-wrapper reads target- and role-mangled
spellings (`NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`, `_FOR_BUILD`,
`_FOR_TARGET`) rather than the base names.

Of the exact-name groups, only the search paths were a live route:
`CMAKE_INCLUDE_PATH` is prepended to `find_file` order, which no `-D` protects,
and seL4 resolves `KERNEL_HELPERS_PATH` that way. The rest are defense in depth
and are labelled so in the code rather than described as leaks.

`kernel_sha256` was re-observed as `e8cbab4f…` on `aarch64-darwin` — **since
superseded by B20's `97dcb029…`**, which is the same kernel built with the
frame-pointer policy stated rather than inherited. The other four
pinned artifacts were already reproducible and are unchanged. The hash still
binds `cmake`, `ninja`, and the host Python generators, which this file does not
pin — recorded as a residual in the devlog, not claimed as closed.

**Exit condition (observed 2026-08-06):** `just sel4_qemu_image_check` passes,
and adding `hexdump` to `flake.nix`'s `packages` moves the shell's seed from
`r279wlb3cq` to `rhl1f441df` while leaving `kernel_sha256` byte-identical. A
third build with a fabricated seed, fake `-isystem` store paths, a narrowed
hardening set, and an ambient `CFLAGS` is byte-identical too. Fault-injected:
one nibble changed in `kernel_sha256` makes the gate exit 1.

**A second host was then observed, on `aarch64-linux` under OrbStack** (shell
seed `65gzz0x3v8` against Darwin's `r279wlb3cq`). B19's property holds there —
a real-shell build and a hostile-environment build are byte-identical — but at
`f2d316e1…` rather than `e8cbab4f…`, because Darwin resolves a *cross*
`gcc-wrapper` that forces `-fno-omit-frame-pointer` while `aarch64-linux`
resolves a *native* `gcc` that does not. That is a genuine toolchain difference,
which is what the gate exists to catch, so the pin stands as recorded. It does
mean `[observed_prefix]` is **per-platform**; that was opened as B20 rather than
folded in here, and B20 is now resolved — both platforms produce a
byte-identical `97dcb029…`. See
`devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`.

### B18 — the seL4 stream gate was scheduling-dependent — **resolved 2026-08-06**

**Problem:** `just sel4_stream_check` passed roughly one run in three. Two
independent causes, both invisible on x86 because the retired kernel's
cooperative scheduler orders the events favourably every time.

**Cause 1 — a publisher writing to a route it had already retired.**
`fabric-publisher-b` sent its first `diagnostics` sample with `FLAG_LAST` and
then published on that route again after the large telemetry sample. That second
send was **dead code**: `FLAG_LAST` sets `publisher.finished`, and both the
broker loop and `park_on_streams` skip a finished publisher, so nothing ever
read it. Worse than inert — once `diagnostics` retired, only `telemetry` kept
the fabric alive, so after that drained the send answered `ERR_PEER_DEAD`, which
`publish` treats as fatal. Deleted.

**Cause 2 — `debug_write` was one syscall per byte.** Under `PRINTING` the
component-side implementation called `seL4_DebugPutChar` per character,
bypassing the root entirely. The root's own `debug_println!`, or another
component's line, could land mid-string: the transcript showed ` QoS matched`
where `[fabric] QoS matched` was written, and whichever gate required the
destroyed marker failed on a boot that was otherwise correct.

This was the larger cause, and it masqueraded as several different bugs — a
missing `re-delegation denied`, a missing `large sample published`, and (because
a corrupted `QoS matched` changes what the transcript appears to say about
matching) an apparent provisioning race. Diagnosing it as one defect rather than
three took reading full transcripts rather than the gate's 40-line tail.

`Operation::DebugWrite` is now served by the root's graph loop, which is
single-threaded and answers one request at a time, so a line printed inside that
arm cannot interleave with anything. Atomicity is structural rather than a
matter of timing. The cost is that printing now needs a bound transfer window;
every launched component binds one before it runs.

**Two fixes were tried and reverted**, both recorded because each looked
plausible and each made things worse:

- **Moving `FLAG_LAST` to the second diagnostics sample**, where the route
  genuinely ends. Wedges `just fabric_qos_check`, whose subscriber waits for the
  terminal event the early flag produces.
- **Making the stall stop acking.** `receive_large_sample` acks the inline
  samples it passes over, which does drain the ring the stall is supposed to
  overrun — but removing the ack wedges the fabric outright, because it waits
  for a delivery slot that never frees. The ack is load-bearing.
- Narrowing `fabric-subscriber-b`'s declared `historyDepth` from 4 to 2 also
  failed, and for the same underlying reason as everything else: the failures
  were marker corruption, not ring arithmetic.

**Exit condition (observed):** ten consecutive `just sel4_stream_check` runs
pass, with all six other seL4 gates, `just fabric_stream_check`,
`just fabric_qos_check`, `just fabric_visibility_check`, and
`just data_fabric_boot_check` unchanged. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B17 — the capability transfer's subset test had no coverage — **resolved 2026-08-05**

**Problem:** `slime-root/src/main.rs::serve_cap_transfer` enforces four rules,
and P5.5.1's gate observed three. The fourth — the **subset test**,
`rights & !source.rights != 0`, which is what makes the move narrow-only
against *what the holder actually has* — was not observed: deleting it left
every marker in that gate intact.

**The entry's stated reason was wrong, and that is the interesting part.** It
argued the property was unreachable from any graph this cutover could declare,
because reaching it needs a capability holding transfer authority while being
strictly narrower than its kind admits, and `cap_transfer` with
`FLAG_RETAIN_TRANSFER` was "the only thing that produces one" — which a
component cannot use on itself, since the two ends of a channel it holds alone
are a loopback the root refuses to split.

A plain **spawn grant** produces one. `preflight_spawn_grants` installs the
requested mask verbatim, so `grant(endpoint, RIGHT_SEND | RIGHT_TRANSFER)`
yields exactly send+transfer where `Endpoint` admits send+recv+transfer.
Init already does this on x86 for `DANGO_OUTPUT_SLOT` — the shape existed in the
tree the whole time; nobody had asked to widen one. The gap was a missing arm,
not an unreachable property, and the analysis that said otherwise was checking
`cap_transfer`'s own outputs rather than every path that installs a mask.

**Resolution:** `sel4-stream.zti` grants `fabric-publisher` a second endpoint
end at send+transfer, carrying no traffic and belonging to no route. It goes to
the publisher because that component already carries the other two
transfer-rule denials, so all three sit together and each states which rule it
proves. The component asks to move it with `recv` restored: that passes the transfer-authority rule,
passes the descriptor/kind rule, and computes zero against the per-kind mask, so
only the subset test can refuse it.

The arm is guarded on **holding** the subject rather than on a check flag,
because an empty slot answers the same `ERR_BAD_CAP` the subset test does — a
bare widening arm would pass identically in a graph that never granted the
endpoint, which is the "looks like coverage and is not" failure this item was
opened for. It establishes possession by *using* the granted end first, so a
graph without one skips silently and claims nothing.

**Exit condition (observed):** `just sel4_stream_check` observes the refusal,
and removing `rights & !source.rights` from `serve_cap_transfer` fails that gate
— the fault injection P5.5.1 ran and could not make fail. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four — **resolved 2026-08-05**

**Was:** `slime-root`'s spawn read its grant array through
`transfer_window::read_staged`, whose bound is `ipc::MAX_MESSAGE_BYTES` (64). At
`SPAWN_GRANT_RECORD_BYTES` = 16 that is **four** records, against the retired
kernel's sixty-four. Real x86 callers already exceeded it —
`init.rs::GENERATION_MANAGER_CAPS` and `dango_caps()` are six each, and
`launch_fabric_graph` hands the fabric nine — so a component that runs on the
retired kernel would have failed to launch its children on the cutover, which is
the one property P5.4 must be able to claim.

**Fixed by** a second staged bound rather than a wider message.
`transfer_window::MAX_STAGED_ARRAY_BYTES` (1024) bounds an *array* staged
through a window, where `MAX_STAGED_BYTES` bounds a *message*; the two stay
separate numbers because a `send` payload becomes an `ipc::Message` and is that
wide by construction, while a grant array becomes no message at all. The
component side needed no change: `sel4_transport::spawn` already encoded into a
`MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES` buffer and staged it into a 4096-byte
window, so the refusal was entirely root-side.

**Exit condition observed 2026-08-05** under `just sel4_spawn_check`: `init`
spawns `sysinfo` with **six** grants — B15's own number, and the size of this
repository's largest real grant lists — and all six ends move, each granted slot
leaving init's table while each retained half still sends. Fault-injected: with
the narrow reader restored the spawn is refused outright and the gate fails. See
`devlog/2026-08-05-p5-5-1-typed-fabric/`.

### B14 — `slime-root` ignores the generation's declared spawn budget

**Problem:** the generation declares `spawnBudget` per component, and
`slime-root/src/main.rs::serve_spawn` never reads it. A component with a
declared budget of 1 can spawn until `MAX_TASKS` fills. The retired kernel
checks it first thing in `spawn_from_cap`
(`kernel/src/task/mod.rs`: `if task.live_children >= task.spawn_budget`), and
refuses with `ERR_OUT_OF_MEMORY`.

This is the same shape B13 had, and it is why it is recorded rather than left
in a devlog note: the generation declares a bound and the root does not enforce
it, so the only thing limiting a component is a global table size no generation
named. Authority to spawn comes from the executable grant, which *is* checked;
what goes unchecked is how many times it may be used.

The blast radius is currently small — no seL4 fixture spawns near its declared
budget, and `boot_contracts` already clamps the decoded value to
`MAX_SPAWN_BUDGET` — so it is a latent hole rather than an observed defect.

**Evidence:** `Component::spawn_budget` is decoded in
`boot-contracts/src/generation.rs` and read nowhere in `slime-root/`;
`contracts/generation/v1/fixtures/sel4-spawn.zti` declares `spawnBudget = 4`
for `init`, which spawns twice, so no boot currently reaches the bound. Noted
while implementing spawn in P5.3.3; see
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Proposed fix:** count live children per task in `TaskTable`, decremented when
a child is reclaimed, and refuse a spawn past the declared budget with
`ERR_OUT_OF_MEMORY` — matching the retired kernel's code, since
`init.rs::spawn_optional_storage` already distinguishes that from `ERR_BAD_CAP`.
The count must be decremented on both death paths, not only on clean exit.

**Why deferred rather than fixed in P5.3.3:** the exit condition that slice
carries is about *which* executables resolve and how a child's fate is
observed, not how many children may exist. Adding a counter would be
straightforward, but the arm that proves it needs a fixture whose component
spawns past its declared budget, which is a scenario rather than a line —
P5.3.4 composes the sample plane and is where a multi-child graph already
exists.

**Exit condition:** a component whose generation declares `spawnBudget = N` is
refused `ERR_OUT_OF_MEMORY` on its `N+1`th live child and succeeds again once
one is reclaimed, observed under a named seL4 gate, with the five existing seL4
gates still passing.

**Resolved 2026-08-05** by P5.3.4; see
[`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md).

`slime-root/src/main.rs::serve_spawn` now reads the caller's declared
`spawnBudget` and refuses a spawn past it, before anything is allocated. The
count is *derived* rather than tracked: `Task` records the id of the task that
spawned it, and `TaskTable::live_children` counts the table. A counter would
need decrementing on the clean-exit path, the fault path, and every spawn
unwind, and a missed decrement would silently tighten a bound the generation
declared — whereas a reclaimed task frees its parent's budget by ceasing to
exist.

The refusal is `ERR_OUT_OF_MEMORY`, matching `sys_spawn`, which maps
`BudgetExhausted` and `TooManyTasks` alike to that code and everything else to
`ERR_BAD_CAP`. That distinction is the caller's business in a way the preflight
refusals are not: a component at its ceiling learns something true about itself
and can wait for a child to exit.

The deferral reason was "P5.3.4 composes the sample plane and is where a
multi-child graph already exists," and that is this slice.

**Observed exit condition, both clauses.**
`contracts/generation/v1/fixtures/sel4-sample.zti` declares `init` a budget of
exactly two — the two children the composition needs — so the third spawn is a
denial arm rather than an unused allowance. `just sel4_sample_check` asserts
`SLIME_GRAPH spawn refused task=N child=... class=budget live=2 budget=2` and
`[init] spawn budget refused`, which `drive_sample_plane` prints only after
requiring exactly `ERR_OUT_OF_MEMORY`.

The second clause — "succeeds again once one is reclaimed" — is asserted too,
and getting it required a real fix. `TaskTable::reclaim` was reachable from the
P5.1 fixture path and from `release_child`, but from neither death arm in
`serve_component_graph`, so a dead child kept its table entry and the derived
count made the budget a *lifetime* cap. Both arms now reclaim, and init spawns
once more after both children exit; a lifetime cap would refuse there too, so
that arm is what distinguishes the two readings. All six seL4 gates pass.

**Fault injection.** With the budget check disabled the gate fails on
`spawn budget did not bite`; with task reclamation removed from the death paths
it fails on `budget did not recover after a child exited`. Both arms are covered
rather than merely present.

### B13 — `slime-root` admits a shared-buffer allocation without resolving a factory capability

**Problem:** `slime-root/src/main.rs::serve_buffer_create` ignores the factory
slot its caller names and admits the allocation against the holder's declared
quota alone. The retired kernel resolves a `RIGHT_BUFFER_CREATE` capability
first (`kernel/src/syscall/mod.rs::sys_shared_buffer_create`), so a component
the generation grants no factory allocates nothing there whatever its budget
says. On seL4 the budget is the only bound: a component with a non-zero ceiling
and no factory grant still allocates.

That inverts the intended relationship between the two. The grant authorizes
the operation and the budget bounds it; they are independent by design, and
`components/bins/src/shared_buffer_probe.rs` documents exactly that. With the
grant unchecked, authority to allocate follows from a budget entry — which is
ambient authority arriving through the back door, against the invariant that
`slime-root`'s whole capability model exists to hold.

The blast radius is currently small: every seL4 generation that declares a
budget holder also intends it to allocate, so no live graph is mis-admitted.
It is a latent hole rather than an observed defect.

The same discarded word carries the caller's `writable` flag
(`slot_with_flag(factory_slot, writable)` in
`components/runtime/src/syscall/wire.rs`), so every region is created writable
whatever the caller asked for. That is permissive in the same direction and
belongs to the same fix.

**Evidence:** `slime-root/src/main.rs::serve_buffer_create` takes no slot
argument and the `SharedBufferCreate` arm reads only `words[1]`, against
`kernel/src/syscall/mod.rs::sys_shared_buffer_create`'s capability resolution.
`graph::Resource::SharedBufferFactory` is defined and never installed or
resolved anywhere in the crate. Noted while adding the loan plane in P5.3.2 and
confirmed by that slice's review; see `devlog/2026-08-04-p5-3-2-loan-plane/`.

**Proposed fix:** materialize the boot layout's `shared-buffer-factory` role and
the generation's `bufferCreate` grants into the holding components' capability
tables, the way `channel::materialize` already does for send/recv grants, and
resolve the slot in `serve_buffer_create` before admitting anything — reading
the `writable` flag from the same word while it is being decoded.

P5.3.2 made this sharper rather than causing it: replacing the uniform
`SHARED_QUOTA` with the generation's declared ceilings means the budget now
carries the weight the factory grant used to. Authority to allocate currently
follows from a budget entry alone, which is why the entry moved to the top of
the open list.

**Why deferred rather than fixed in P5.3.2:** installing non-channel grants
changes what occupies each component's capability table, and therefore the slot
numbers `channel::materialize`'s cursor hands out for channel ends. Those
numbers are asserted marker-for-marker by `just sel4_component_graph_check` and
`just sel4_channel_check`. Renumbering them is the same distribution problem
P5.3.3 solves for spawn grants, and doing it twice — once here and once there —
would rewrite two gates' evidence for one change.

**Exit condition:** a component holding a budget entry but no `bufferCreate`
grant is refused `ERR_BAD_CAP` by `shared_buffer_create`, observed under a named
seL4 gate, with `just sel4_component_graph_check`, `just sel4_channel_check`, and
`just sel4_loan_check` still passing.

**Resolved 2026-08-05** by P5.3.3; see
[`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md).

`slime-root/src/main.rs`'s `SharedBufferCreate` arm now resolves the factory
slot the caller names, requiring `RIGHT_BUFFER_CREATE`, before admitting
anything — and reads the `writable` flag out of the same word while it is being
decoded, so a region created read-only no longer carries write rights. The
generation's `bufferCreate` grants are materialized into the holding
components' capability tables beside the channel ends: at the boot layout's
role slot for the bootstrap component, and above the executables for every
other, which is the same split `channel::materialize` already made.

The deferral reason was verbatim "the same distribution problem P5.3.3 solves",
and that is this slice, so it was closed here rather than deferred again.

**Observed exit condition.** `just sel4_loan_check` asserts
`SLIME_GRAPH buffer create refused task=N class=ungranted` before any ceiling is
grazed, so the refusal is a capability answer rather than a quota answer wearing
another name. Two arms in one marker pair: an empty slot and a slot holding real
authority of another kind are refused identically, which is what stops a
component probing its table by watching which error comes back.
`just sel4_component_graph_check`, `just sel4_channel_check`,
`just sel4_loan_check`, and `just sel4_spawn_check` all pass.

**Fault injection is what made this real.** Removing the factory check left
*every* gate passing: no fixture had a component that held a budget and tried to
allocate without a grant, so the fix was uncovered by construction. The loan
fixture's `init` now names one deliberately. Recorded because a gate that passes
against an injected build is evidence of nothing, and this one nearly shipped
that way.

### B11 — test scaffolding is declared in the product boot generation

**Resolved:** 2026-08-01. See
`devlog/2026-08-01-b11-product-boot-profiles/`.

**Problem:** The source manifest had one global component graph and health
policy. It declared the sixteen probes and scenario doubles originally named by
B11, plus the test-only `storage-writer`, as peers of product services with
real capability grants. Selecting a fabric profile changed interposition only;
it could not remove a component, its executable object, authority, budget, or
health edge from the authenticated generation.

**Fix:** Added a versioned Zutai `BootProfile` to the existing profile mechanism.
The builder resolves one profile to a closed component/object/grant/state/budget/
health/fabric graph before encoding. `default` is the scaffolding-free product
profile; `test`, `visibility`, and `unified` explicitly declare the verification
participants their gates use. The boot-layout emitter and kernel placer accept
profile-absent scaffolding while retaining exact rights and filled-slot checks,
and init consumes the same generated labels for every scenario executable and
authority role.

**Exit condition (observed):** `just product_boot_check` boots a healthy 45-slot
product generation that names none of the seventeen test-only components. `just
boot_layout_check` passes all nineteen profile/layout pairs while preserving all
eighteen pre-B11 fixtures. Every probe-dependent gate explicitly selects its
profile and passes, including all five storage gates, directory, powerbox,
sample-plane, fabric authority/stream/QoS/call/operation/visibility/full-graph,
generation commands, rollback, bootstate trace, and transfer. `just test` passes
189 assertions; contracts, generation determinism, formatting, lint, Python
lint, spelling, devlog, and Framework safety checks are clean.

### B10 — init's capability layout is a positional convention, so boot paths are selected at kernel compile time

**Resolved:** 2026-08-01. See `devlog/2026-07-31-boot-layout-baseline/` for the
equivalence baseline and `devlog/2026-08-01-boot-layout-resolution/` for the
change.

**Problem:** `launch_init` builds init's capability vector by writing fixed
indices (`caps[46] = ...`) rather than resolving named grants the generation
declares. `MAX_CAPS = 64`, and the vector was 61 occupied before C8.10, so a new
participant set cannot be appended — it must squat on another profile's slots or
fork a whole `launch_*_init`. Both happened. The gates that read those slots read
them positionally, which is why the layout cannot simply be renumbered.

The escape hatch chosen instead was compile-time selection: `option_env!` reads a
`SLIME_*_CHECK` flag and compares `generation.number` against a literal. Because
`option_env!` is evaluated at compile time and Cargo tracks these as build inputs
(the kernel's dep-info records `env-dep:SLIME_DANGO_CHECK`,
`env-dep:SLIME_GENERATION_CMD_CHECK`, `env-dep:SLIME_POWERBOX_CHECK` and
siblings), each gate builds a *different kernel binary*. There is no single
kernel artifact that passes the gate suite.

This blocks P1. That milestone requires that "architecture-neutral code can be
type-checked for AArch64 without importing x86-only modules", which cannot hold
while the boot path is selected by x86-gate build flags and hardcoded generation
numbers.

**Evidence:** `kernel/src/runtime/bootstrap.rs:176-182` states the constraint
outright — the vector is "61 of `MAX_CAPS = 64` before this milestone adds
anything", the three new C8.10 roles "need nine slots against three free", and
the vector "is also the layout six passing QEMU gates read positionally — the
`caps[46] = ...` blocks below rewrite it per generation number — so renumbering
it to fit would rewrite C8.3-C8.8's evidence rather than extend it".

Counted at the commit that opened this item:

- 26 positional writes over 13 distinct slots (46-59) in `bootstrap.rs`;
- 3 `launch_*_init` forks: `launch_init` (168), `launch_fabric_boot_init` (964),
  `launch_recovery_init` (1087);
- 9 `generation.number ==` branches in `launch_init`, including
  `generation.number == 14` reassigning slots 46/47/49 under the comment that
  "the call gate reuses the executable/control slots occupied by three stream
  participants in every other generation profile", and the mutually exclusive
  call/operation profiles at lines 793 and 828 sharing one slot range;
- 21 distinct `option_env!("SLIME_*")` flags over 70 sites (18 in `kernel/src`,
  52 in `components/`);
- 11 distinct generation numbers driven by check scripts (6, 7, 8, 9, 10, 11,
  12, 13, 14, 16, 99), e.g. `check-fabric-stream.py` sets
  `SLIME_FABRIC_STREAM_CHECK=1` with number 12, `check-fabric-qos.py` sets
  `SLIME_FABRIC_QOS_CHECK=1` with 13, and `check-data-fabric-boot.py` sets
  `SLIME_FABRIC_BOOT_CHECK=1` against the kernel's `generation.number == 17`.

**Fix as proposed when the item opened:** Resolve init's grants by name from
the generation instead of by index in kernel source, so a profile's participant
set is generation data. The hard constraint is that every profile in use today
must resolve to **the same slot numbers it occupies now** — a naming layer over
the existing
layout, not a renumbering, because renumbering rewrites six gates' evidence
rather than extending it. With grants named, the `option_env!` and
`generation.number` branches in `launch_init` lose their purpose and the
`launch_*_init` forks collapse.

Storage identity selection at `bootstrap.rs:571` and `bootstrap.rs:595`
(generation numbers 2, 3, 4 selecting different capabilities and a different
storage component) is the same pattern on a different axis. Decide explicitly
whether it is in scope before starting; do not leave it undecided.

Component-side flags are not assumed to fall out of this: 52 `option_env!` sites
in `components/` (9 reading `SLIME_FABRIC_VISIBILITY_CHECK` alone) make their own
build-time decisions independent of the kernel layout, and may need their own
pass.

**Fix:** A `contracts/boot-layout/v1` resource declares which capability slot
holds which role, under which name, with which rights, per generation number.
`launch_init` offers each capability it mints to a placer under the name the
layout knows it by, and the layout decides where it lands; a capability the
layout does not name, or a declared slot nothing fills, stops the boot. The
storage `generation.number` matches disappear by construction rather than by a
separate fix, because the layout names the component and declares the rights.
Profile branches ask what the layout declares instead of comparing against a
literal, and the C8.10 fork keys on the layout declaring the fabric's own route
workers — putting it in the same category as the `component_named("recovery")`
fork beside it. The script-install and idle-exit gates were each `flag &&
number == N` with a unique number per gate, so the flag was redundant in all
ten. `init.rs` reads the same table, rendered as Rust at component build time,
dropping 84 lines of constants that previously agreed with the kernel only by
inspection.

An entry declares a *role*, not a concrete object: the storage slot resolves to
a block device when the platform enumerates one and an object store when it
does not, which is decided by PCI enumeration at boot and is not knowable to
the host builder.

**Exit condition (observed):** `just boot_layout_check` — a new gate, since
P0/P1's `architecture_contract_check` and `x86_portability_check` do not exist
— boots all eighteen distinct profiles and finds every slot, label, and rights
value identical to the pre-change fixtures. `launch_init` contains no
`option_env!` and no `generation.number` branch. One kernel binary now serves
every gate: built with no flags and with `SLIME_FABRIC_BOOT_CHECK`,
`SLIME_DANGO_CHECK`, `SLIME_FABRIC_CALL_CHECK`, `SLIME_POWERBOX_CHECK` and
`SLIME_GENERATION_CMD_CHECK` all set, it hashes identically, where the same
comparison previously gave three distinct binaries. The named gates observe
their existing results: `dango_check`, `sample_plane_live_check`,
`fabric_stream_check`, `fabric_call_check`, `fabric_operation_check`,
`fabric_visibility_check`, `data_fabric_boot_check`, plus `fabric_qos_check`,
`fabric_authority_check`, `generation_cmd_check`, `powerbox_check`,
`directory_check`, `transfer_check`, `rollback_check`, `bootstate_trace_check`,
`test`, `contracts_check`, `generation_check`.

**Fault injection:** three defects surfaced during the change, each caught by a
fixture rather than by reading code. Generation 4 declares two identical
object-store entries, so resolving a role by first-match filled one slot twice;
generation 14 leaves `fabric-subscriber-b` in slot 50 because the call profile
rewrote 46-49 and stopped; generation 15 takes slot 50 but leaves the same
component's control channel at 55 and 60. The last two are the argument for the
change — which slots a profile overwrote was implied by the index range a
rewrite block happened to cover, stated nowhere and checked by nothing. The
emitter's own guards were fault-injected too: a duplicate slot, a named role
without a label, an unnamed role carrying one, and a stale component fallback
table are each rejected.

**Follow-up:** `launch_fabric_boot_init` still builds its 53-slot table
positionally while the layout declares those same slots, so the C8.10 path
keeps the one-sided-authority property `init.rs` shed; `boot_layout_check`
covers it, but by inspection rather than construction. `launch_recovery_init`
is unchanged and was decided out of scope: its trigger is already
generation-data-driven, and no layout fixture covers its four-slot table.
`SLIME_INTERACTIVE` remains in `on_idle` — a user-facing mode from `just run`,
not a gate, and it does not divide the kernel binary across the suite. 52
`option_env!` sites remain in `components/`, which B10's text anticipated; the
component images are per-generation artifacts by design.

### B9 — terminated tasks are never reaped, so their frames never return

**Resolved:** 2026-07-28. See `devlog/2026-07-28-b9-task-frame-reclamation/`.

**Problem:** `task::terminate` marked a task `Terminated`, drained its
capabilities, and reclaimed its shared buffers, but never removed the `Task`
from the scheduler. The `Task` — and the `AddressSpace` it owns — therefore
lived for the rest of the boot, so `AddressSpace::drop` never ran. Even when it
did, that `Drop` freed only the PML4 frame and deliberately leaked every
user-half page table; the image and stack frames mapped by
`spawn_with_caps_for` had no release path at all. Every spawn permanently
consumed its image pages plus its stack pages, so a repeated spawn/exit
workload drained the frame allocator monotonically.

**Evidence:** `kernel/src/task/mod.rs` — `terminate` pushed to
`sched.terminated` and left the task in `sched.tasks`; `remove_task` was called
only from the `spawn_from_cap` capability-insert failure path.
`kernel/src/memory/address_space.rs` — `Drop` dealloc'd `self.pml4` alone, with
the comment that intermediate user-half tables "intentionally leak for the
small M2 isolation test". The per-cycle delta is no longer an inference: a boot
probe running four real spawn/release cycles before `launch_init` reported
`spawn/exit leaked: 52 frame(s) over 4 cycles` — 13 frames per cycle.

**Fix:** two gaps on one path, closed together. `vmm::free_user_half` walks
PML4 entries 0..256, freeing leaf pages then the tables that held them, and
`AddressSpace::drop` now calls it before releasing the PML4 — so every frame an
address space owns has a release path, including on the `spawn_with_caps_for`
early-return paths, which hold it as a local. `reap_terminated` gives the
scheduler a reclamation point, removing every terminated task except the one
the CPU is standing on; it runs from `schedule_next` after the switch target is
chosen. Reaping is deferred rather than immediate because `terminate` executes
on the terminating task's own kernel stack and address space. `sched.terminated`
stays a separate log, so `supervision_status` and `SYS_WAIT` still answer for a
reaped child. The kernel half (entries 256..512, shared aliases of the one
kernel hierarchy) is never touched.

**Exit condition (observed):** the boot probe reports `spawn/exit conserves
frames: 14 per cycle, 0 drift`, asserted by `just dango_check`. `just test`
passes 185 assertions including five new `task_reclamation` cases — eight-cycle
conservation, release scaling with image size, a task holding capabilities, a
rejected spawn, and the shared-buffer double-free ordering. Supervision results
stay observable after reaping, proven by `just spawn_service_check` and `just
dango_check`, whose components spawn and exit through `terminate` and the
reaper and still report a healthy slice; `just sample_plane_live_check` and
`just fabric_stream_check` are unaffected. Fault injection confirms the guards
bite: removing the `free_user_half` call makes both the harness tests and the
live probe fail, and inverting the reclaim/release order fails the double-free
test.

**Follow-up:** a task that terminates when nothing else is runnable is reaped by
the *next* scheduling event, which on the non-interactive path never comes —
`on_idle` exits QEMU. One task's frames are therefore returned to an allocator
that is about to stop existing, which is harmless today but is the residual
lag C10.4's spawn/exit measurement should quantify. The live probe covers the
release path rather than the reaper; a gate counting frames across a full
spawn/exit/reap cycle needs a userspace loop and belongs with that milestone.

### B8 — budget validation bounded each holder but never the aggregate

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** `SharedBufferBudget::validate_against` checked each holder's quota
against the fixed kernel ceilings but never summed holders, so a budget could
promise N holders `MAX_TOTAL_PAGES` each. Not exploitable —
`SharedBufferTable::create` still enforced the real global ceiling — but the
roadmap said decode rejects "globally impossible" limits, and an aggregate
over-commit degraded a declared quota into first-come-first-served: a
late-starting component failed with `BytesExhausted` despite holding a quota the
generation promised it.

**Evidence:** `boot-contracts/src/shared_buffer_budget.rs:116-148` looped per
entry with no accumulator; its comment noted `max_buffer_pages` was retained
only "for symmetry". Lib tests covered per-holder impossibility only.

**Fix:** Chose the stricter reading, since `AGENTS.md` requires generation data
to be deterministic, bounded, and explicitly validated: `validate_against` now
sums `byte_pages`, `buffer_count`, `mapping_count`, and `loan_count` with
saturating adds and rejects any total past its kernel ceiling, so a budget that
validates is one the kernel can honour with every holder at its ceiling at once.
Also added the two per-holder bounds the check was missing — `mapping_count` and
`loan_count` against `MAX_MAPPINGS`/`MAX_LOANS`, without which a holder could
declare 200 mappings against a 64-entry table. `validate_against` grew to five
parameters; the kernel caller passes the new ceilings.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 24
tests, including `aggregate_over_commitment_is_rejected`,
`aggregate_buffer_mapping_and_loan_ceilings_are_enforced`, and
`per_holder_mapping_and_loan_ceilings_are_enforced`. Fault injection confirms it
bites on the live path: raising the manifest to 306 aggregate pages (> 256) made
the boot fail closed, and the real budget (18/256 pages, 5/32 buffers, 10/64
mappings, 5/64 loans) passes. `just generation_check` (two byte-identical
builds), `just contracts_check`, `just spawn_service_check`, `just
sample_plane_live_check`, `just test`, and fmt/lint are clean.

**Follow-up:** The host builder does not validate the aggregate; only the kernel
does at decode, so an over-committed manifest builds and fails at boot. That is
fail-closed and keeps one source of truth for the rule.

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b7-b8-budget-hygiene/`.

**Problem:** C7.1's deliverable was to replace the grandfathered generic
`RIGHT_MAP` name with an object-specific shared-buffer map right. The kernel
constant became `RIGHT_BUFFER_MAP`, but the manifest key stayed `map`, so
generation authors kept writing a generic name for buffer-specific authority.

**Evidence:** `scripts/build/build-generation.py:112` mapped `"map": 1 << 9`
alongside object-specific siblings `bufferWrite`, `bufferCreate`, `bufferLoan`;
`kernel/src/capability/mod.rs:39` defined the same bit as `RIGHT_BUFFER_MAP`.

**Fix:** Renamed the builder key to `bufferMap`. No wire or identity change —
the bit value is unchanged and no manifest fixture referenced the old key.

**Exit condition (observed):** No `"map"` key remains in the builder rights
table; `just generation_check` produces two byte-identical builds and `just
framework_safety_check` stays clean.

### B6 — the retained-v2 "still boots" claim was proven only as decode

**Resolved:** 2026-07-26 (scope corrected + admission covered). See
`devlog/2026-07-26-b6-retained-v2-rollback-scope/`.

**Problem:** C7.1's exit condition stated that a retained v2 known-good artifact
"still decodes **and boots**". Only decode was proven; no v2 generation was ever
booted.

**Evidence:** `scripts/lib/boot_contracts.py:7-8` pins `GENERATION_MAGIC =
b"SLIMEG3\0"` / version 3, so the builder emits v3 only. The sole v2 artifacts
were hand-built in memory (`boot-contracts/src/generation.rs`,
`kernel/tests/sample_plane.rs:564`).

**Resolution:** The boot arm is not merely unproven, it is unconstructible from
this tree, and investigating why closed a more interesting question.
`stage0::verify_kernel` (`stage0/src/lib.rs:320-325`) resolves
`generation.kernel_object`, so each generation embeds and boots its **own**
kernel. A retained v2 generation therefore runs its v2-era kernel — which is
also why this tree's v3-only rights cannot break the rollback window, despite
`bufferCreate` (bit 24) lying outside v2's 24-bit rights space and
`require_grant` being unconditional. Any "v2 boot" staged today would pair a v2
manifest with a v3-era kernel: a configuration that has never existed.

Covered the provable and load-bearing part instead — the stage-0 admission
chain, which had no coverage. Two `boot-contracts` tests were added:
`retained_v2_generation_passes_stage0_admission` (identity seal, kernel object,
bootstrap component, tamper detection) and
`retained_v2_authority_manifest_is_width_stable`, which pins the 32-bit v2
authority hash. That second one guards a real hazard: `release.rs:163` binds a
signed release to `authority_manifest_identity`, so losing the version branch
would fail every retained v2 release while every gate stayed green. C7.1's
status and exit condition now claim decode + release authorization + admission,
and state why the boot arm cannot be staged.

**Exit condition (observed):** `cargo test -p boot-contracts --lib` passes 21
tests (19 prior + 2 new). Fault injection confirms the guard bites: removing the
v2 branch from `authority_manifest_identity` so it hashes at 64-bit made
`retained_v2_authority_manifest_is_width_stable` fail, and the branch was
restored. `just contracts_check`, `just generation_check`, and `just
transfer_check` all pass.

**Follow-up:** If a real v2 generation is ever recovered from history, booting
it under QEMU would upgrade this from admission to a true rollback boot. The
rollback window also remains unlimited in code — v2 retention is unconditional
decode support, noted since C7.1.

### B5 — no C7 gate exercised the syscall layer or real components

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b5-live-sample-plane/`.

**Problem:** No test or component reached any `SYS_SHARED_BUFFER_*` syscall. The
gates called `SharedBufferTable` methods on locally constructed tables and never
touched the global `SHARED_BUFFER_TABLE`, so the rights gates, the loan receiver
binding, and reclamation through real termination were unproven. C7.7's "two
isolated components" were the `u64` constants `0x71`/`0x72`, and its "peer death"
was a direct `reclaim_owner` call. This is the blind spot B3's boot wedge shipped
through.

**Evidence:** `grep 'dispatch|UserFrame|sys_'` and `grep SHARED_BUFFER_TABLE`
over `kernel/tests/` both returned no matches, while `SharedBufferTable::new()`
appeared 33 times. `kernel/tests/sample_plane.rs:57-58` defined its holders as
bare integers; `:462` stood in for peer death with `reclaim_owner`.

**Fix:** Added the four missing loan wrappers (`loan`/`loan_map`/`return`/
`revoke`) to `slime_rt`, completing the nine-syscall surface begun in B4. Added
two real components, `sample-lender` and `sample-receiver`, that the generation
grants a factory, a channel, and a `supervise` handle; init spawns the receiver
first so the lender names its loan receiver by capability rather than ambient
task id. `just sample_plane_live_check` asserts an ordered transcript covering
the happy path plus six denial arms, and rejects any component `fail:` line.
A first draft exposed a real ordering property: a lender that exits before the
receiver maps has its loan settled by its own termination, so the lender now
waits for a settle message — the C7.5 retention rule, asserted rather than raced.

**Exit condition (observed):** `just sample_plane_live_check` passes: two
separately spawned components move a two-page payload — larger than `MAX_MSG` —
through the real syscalls, with only the 64-byte descriptor crossing the IPC
channel, and every denial arm observed before the operation it guards.
`just sample_plane_check` (5/5), `just test`, all shared-buffer gates
(8/8/8/7/4), `just spawn_service_check`, `just dango_check`, `just
powerbox_check`, `just transfer_check` (exercising the renumbered slots 45/46),
`just generation_cmd_check`, `just generation_check`, `just
framework_safety_check`, and fmt/lint with `_components` are all clean.

**Follow-up:** `SYS_SHARED_BUFFER_REVOKE` has a wrapper and in-harness coverage
but no live caller, since the lender settles by return. The two insert-failure
rollback paths still need a full capability table at the moment of insert, which
neither gate stages.

### B4 — the C7 shared-buffer plane was dormant on the live boot path

**Resolved:** 2026-07-26. See `devlog/2026-07-26-b4-live-shared-buffer-budget/`.

**Problem:** Nothing in a running system could allocate a shared buffer. No
generation declared a `shared-buffer-budget/v1` resource, so every component
launched with `HolderQuota::DENY`; no manifest granted `bufferCreate`; the
kernel never minted a `SharedBufferFactory`; and `slime_rt` had no wrapper for
any shared-buffer syscall. C7.3's exit condition ("two holders receive distinct
generation-declared budgets") therefore held only inside the kernel test
harness. C7.2/C7.3/C7.4 each deferred this wiring to C7.7, which closed without
doing it.

**Evidence:** The built `generation-1.bin` held 21 objects and zero of kind
`KIND_RESOURCE`; the one `SLIMESB` match sat inside the kernel object's byte
range, not an object payload. No `bufferCreate` grant in the manifest fixture;
`bootstrap.rs` minted `EndpointFactory` and `Input` but never
`SharedBufferFactory`.

**Fix:** Emit the budget as a digest-authenticated `KIND_RESOURCE` object from
`build-generation.py` (entries sorted by `holder_identity` and duplicate-checked,
as `SharedBufferBudget::decode` requires); declare per-holder quotas and two
`bufferCreate` grants in the manifest; mint one transferable
`SharedBufferFactory` in `bootstrap.rs` at a fixed slot ahead of the optional
transfer block (renumbering the transfer slots to 41/42) and validate both
grants with `require_grant`; add the five missing `slime_rt` wrappers; and run a
bounded create/map/write/seal/unmap/release self-check at dango and
spawn-service startup so a normal boot proves its own quota.

**Exit condition (observed):** A built generation contains exactly one
`KIND_RESOURCE` budget object (128 bytes, digest verified, magic `SLIMESB\0`,
two holders sorted by identity) that `crate::generation::decode` validates.
A normal boot prints `[generation] shared-buffer factory grants valid`,
`[dango] shared-buffer quota live`, and `[spawn-service] shared-buffer quota
live`, then `vertical slice healthy`. The new
`booted_generation_declares_distinct_holder_budgets` case decodes the booted
generation and asserts two distinct non-`DENY` quotas with an absent component
denied. `just generation_check` produces two byte-identical builds; `just
test`, all six C7 sub-slice gates (8/8/8/7/4/5), `just dango_check`, `just
transfer_check`, `just generation_cmd_check`, `just contracts_check`, `just
framework_safety_check`, and fmt/lint (with `_components`) are clean.

**Follow-up:** B5 is partly addressed — five syscalls are now exercised on a
live boot, but the four loan syscalls still have no wrapper and no test drives
any syscall.

### B3 — C7.5 wedged every full-graph boot (kernel-stack overflow)

**Resolved:** 2026-07-26. See
`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`.

**Problem:** From C7.5 onward every boot that launched the full component graph
hung instead of draining its ready queue. `transfer_check` stalled after
`[init] generation transfer installed`; `spawn_service_check` and `dango_check`
stalled after `[init] spawn graph launched`. `on_idle` is the only path to
`exit_qemu`, so the guest never exited and each gate died on its timeout — the
same observable class as B2, but an unrelated cause.

**Evidence:** Bisected one gate per worktree: `just transfer_check` passed at
C7.2 `991dcbb`, C7.3 `ed49fb5`, and C7.4 `928389e`, and wedged at C7.5
`ca15764` and HEAD; `just spawn_service_check` passed at `928389e` and wedged
at `ca15764` and HEAD. Not timeout tuning: raising the inner QEMU timeout from
60 s to 600 s still wedged. `git diff --stat ca15764 HEAD -- kernel/src` is
empty, so C7.6/C7.7 were not implicated. Full transcript in
`devlog/2026-07-26-c7-audit/transcript.txt` §3–§4.

**Root cause:** Kernel-stack overflow, not the reclamation logic first
suspected. C7.5 grew `SharedBufferTable` to 10520 bytes of fixed arrays
(`loans: [Option<Loan>; 64]` plus a widened `Mapping`), and the table was
published through a `LazyLock`, whose initializer builds the value on whichever
stack first touches the static. Because no `SharedBufferFactory` is minted on
the live path (B4), the first touch is `SHARED_BUFFER_TABLE.lock()` inside
`task::terminate` (`kernel/src/task/mod.rs:832`) — on a 32 KiB task kernel stack
allocated as a plain boxed slice with no guard page. The 10 KiB temporary
overflowed it while `SCHEDULER` was held, corrupting adjacent memory silently
rather than faulting, so the boot wedged with no panic. Confirmed by raising
`KERNEL_STACK_SIZE` to 128 KiB with no other change, which made the gate pass.

**Fix:** Replaced the `LazyLock` with a plain `const`-initialized
`Mutex<SharedBufferTable>` static, matching `FRAME_ALLOCATOR` and the
`drivers/input.rs` tables. `SharedBufferTable::new()` was already a `const fn`,
so the laziness bought nothing; const-initializing places the table in `.bss`
and removes the stack temporary. The diagnostic stack bump was reverted. Added
a compile-time assertion that `size_of::<SharedBufferTable>() * 2 <
KERNEL_STACK_SIZE`, verified to fire by temporarily setting `MAX_LOANS = 1024`.

**Exit condition (observed):** `just transfer_check` (install, pending boot,
promotion, rollback retention), `just spawn_service_check`, and `just
dango_check` all reach their success lines and exit QEMU `Success` at the stock
32 KiB stack. `just test` (160 assertions), all six C7 sub-slice gates (8/7/8/7/
4/5), `just generation_cmd_check`, `just contracts_check`, `just
generation_check`, `just framework_safety_check`, `just fmt_check`, `just
lint`, `just fmt_check_components`, and `just lint_components` are clean.

**Follow-up:** Task kernel stacks still have no guard page, so a future
overflow will again corrupt memory silently instead of faulting. This fix
removes the trigger, not the class.

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Resolved:** 2026-07-24. See `devlog/2026-07-24-b2-blocked-task-state/`.

**Problem:** `TaskState` had only `Ready`/`Running`/`Terminated`. A task waiting
on input or IPC poll-and-yielded, staying `Ready`, keeping the ready queue
non-empty, so `on_idle` (the only path to `exit_qemu`) never fired and every
non-scripted full-graph boot wedged at `dango>`. A default Escape input script
masked the wedge without removing the pathology.

**Fix (design A — wait-set, not blocking recv):** Added
`TaskState::Blocked(BlockReason{Endpoint,Input,Supervision})` and a multi-source
`SYS_WAIT` syscall (max 8 sources, descriptors pack `kind<<32|slot`). `recv`/
`send`/`input_read`/`supervision_status` stay non-blocking; userspace sweeps its
sources then calls `wait` instead of `yield_now`. Waiter registration lives on
each wake source — `recv_waiter` in a new `ipc::Channel`, a global `INPUT_WAITER`
in `drivers/input.rs`, and `wake_on_terminate` on the child `Task`. Wakes are
deferred through a `PENDING_WAKES` queue drained inside `schedule_next` under
`SCHEDULER` (strict order `SCHEDULER → Channel/QUEUE/INPUT_WAITER →
PENDING_WAKES`), fed by `ipc::send`, the keyboard IRQ, `pump_script`,
`task::terminate`, and `Endpoint::Drop`. `wait` re-checks readiness under
IF-clear before parking to close the lost-wakeup race. The default-Escape hack
is removed; `on_idle` now treats an alive, cleanly-blocked persistent service as
healthy while one-shot probes must still `Exit(0)`, and `SLIME_INTERACTIVE`
routes into a new `task::idle_dispatch` (`sti; hlt`) loop instead of exiting.
A pre-existing regression was also fixed: `copy_from_current` bounded a byte
copy at `MAX_CAPS`=64 via a per-byte scratch array, and the `u64`-rights
`SpawnGrant` widening made dango's 5 grants (80 B) exceed it, so `sys_spawn`
returned `ERR_INVALID_ARG` and dango could not spawn.

**Evidence:** `devlog/2026-07-24-boot-check-hangs/` — every non-scripted
full-graph boot hung at `dango>` until an Escape keystroke was scripted.

**Exit condition (observed):** A non-scripted gen-1 boot parks `console`,
`dango`, and `spawn-service` as `idle-blocked` (consuming no CPU), the ready
queue drains to `on_idle`, and QEMU exits `Success` — no scripted Escape. Every
wake source re-readies its waiter: `just dango_check` (`dango native runtime
check: ok`), `just powerbox_check` (input + endpoint waiters), `just
generation_cmd_check` (multi-source generation-manager), `just
spawn_service_check`/`just storage_read_check` (`vertical slice healthy`), and
`just test` all pass, with `just fmt_check`/`just lint` (and `_components`)
clean.

### B1 — `generation_cmd_check` negative scenarios corrupted the wrong generation

**Resolved:** 2026-07-24.

**Problem:** `just generation_cmd_check` failed on its `bad-closure` and
`bad-release` scenarios. The original diagnosis (init's `spawn_and_wait`
aborting on a rejecting `Exit(1)`) was wrong: `generation-stage` already
classifies a `-4`/`-3` rejection internally and exits `0`, and init already
exits cleanly after the staged rejection. The real defect was in the fixture
builder `scripts/check/check-generation-commands.py`. `build_fixture` corrupted
`entries[1]` by fixed directory index, but the bootstore directory is
identity-sorted and staging targets the *candidate* generation (identity ≠
known-good). When component images changed the identity sort order, the
corruption landed on the untouched known-good generation, so staging *succeeded*
(`status=0`), `generation-stage` hit its non-`-4`/`-3` `fail()` path, and the
boot exited `Failed`.

**Evidence:** Instrumented `generation-stage` printed `unexpected status=0` on
`bad-closure`; probing the fixture confirmed the flipped byte fell inside the
known-good generation's blob, which staging never reads.

**Fix:** Select the candidate entry by `identity != known_good` (read from
BootState) instead of a fixed directory index, so the corruption always lands on
the generation staging actually validates.

**Exit condition (observed):** `just generation_cmd_check` passes for `success`
(`staged release=3`), `bad-closure` (`rejected status=-4`), and `bad-release`
(`rejected status=-3`), with rejected staging leaving both BootState slots
unchanged.
