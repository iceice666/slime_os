# Slime OS component ABI

The canonical surface is `components/runtime/src/syscall.rs` (the operations a
component may name), `components/runtime/src/syscall/sel4_transport.rs` (how
each one reaches the root), and `slime-root/src/main.rs`'s dispatcher plus
`slime-root/src/ipc.rs` (what answers). Update this file in the same change that
adds, deletes, or renumbers an operation label, changes an argument packing, or
changes the reply convention.

There is no Slime kernel and no Slime trap vector. seL4 is the kernel; every
Slime operation is either a native seL4 invocation or one bounded `seL4_Call` on
a badged endpoint the generation granted. The retired custom kernel's
trap-numbered surface (`int 0x80`, `SYS_*` 0–30, `SYS_ENDPOINT_CREATE`,
`SYS_STORE_TRANSACT`, `SYS_GENERATION_TRANSACT`, `SYS_GENERATION_RECEIVE`,
`SYS_RECOVERY_RECONSTRUCT`, `SYS_HEALTH_CONFIRM`, `SYS_WAIT`, `SYS_CAP_TRANSFER`)
was deleted by B39–B50. The product ABI identity is `SLIME_AARCH64_SEL4_V1`
(`contracts/target-profile/v1/schema.zt`); the three trap-based ABI numbers
remain in that contract as unadmitted identities no image is built for.

## Two paths

**Native.** Component-to-component traffic is direct seL4. `send`, `call`,
`reply`, `try_send`, `recv`, `recv_blocking` invoke a declared Endpoint;
`notification_signal`, `notification_wait`, `notification_poll` invoke a declared
Notification. The root neither sees nor mediates these; backpressure, atomic
call/reply pairing, and rendezvous are the kernel's. `yield_now` is
`seL4_Yield`.

**Root-served.** Everything the root owns as mechanism — lifecycle, spawn,
supervision, the capability table, capability transfer, shared buffers,
directories, input, blocks, debug output — crosses as `seL4_Call` with an
operation *label* on one of two badged endpoints. The badge authenticates the
caller; a component cannot forge or relabel another task's identity.

| Endpoint | Child CSpace slot | Served by | Carries |
| --- | --- | --- | --- |
| Root service | 1 (`ROOT_SERVICE_SLOT`) | the graph dispatcher thread | lifecycle, spawn, supervision, capability table, capability transfer, shared buffer, directory derive |
| Console service | 32 (`CONSOLE_SERVICE_SLOT`) | the console dispatcher thread (B41) | debug write, input read, block transact, directory inspect/commit |

Two endpoints because one thread serves each: a slow disk or a noisy console
must not queue behind lifecycle traffic, and a console defect must not share the
system dispatcher's fault domain.

## Root service operations

Labels are the operation numbers. Operands are the fast message registers
`MR0`–`MR3`; `slot_pair(a, b)` packs two 32-bit slots into one word and
`slot_with_flag(slot, flag)` packs a slot with one boolean in bit 32
(`components/runtime/src/syscall/wire.rs`).

| Label | Operation | Operands | Result convention |
| --- | --- | --- | --- |
| 3 | `EXIT` | `MR0=status` | Does not return; the root suspends and reclaims the task. |
| 4 | `SPAWN` | `MR0=executable_slot`, `MR1=transfer descriptor` over the grant array, `MR2`/`MR3` inline payload when it fits | Primary is the supervision capability slot; task identity is never returned. |
| 5 | `DIRECTIVE` | `MR0=REQUEST_TAG`, `MR1` | Boot-fixture handshake only (`sel4_root_boot_check`); not part of the component ABI. |
| 9 | `UNHEALTHY` | none | `0` after the boot selector records it; `-4` when no selector is configured, `-1` when the caller is not a required instance. |
| 12 | `SUPERVISION STATUS` | `MR0=supervision_slot` | `-3` means still live. `0` exit, `1` fault; the auxiliary word carries the exit status or the fault reason code. Consumes the handle on a terminal answer. |
| 13 | `CAP DROP` | `MR0=capability_slot` | `0` on release. Needs no right; an empty slot is `-1` so the answer cannot map the table. |
| 15 | `DIRECTORY DERIVE` | `MR0=slot_pair(directory_slot, rights)`, `MR1=transfer descriptor` over the relative path | Derived capability slot, or a negative error. |
| 21 | `SHARED BUFFER CREATE` | `MR0=slot_with_flag(factory_slot, writable)`, `MR1=pages` | Primary is the capability slot, auxiliary the kernel-assigned buffer identity. |
| 22 | `SHARED BUFFER RELEASE` | `MR0=buffer_slot` | `0` on release. |
| 23 | `SHARED BUFFER MAP` | `MR0=slot_with_flag(buffer_slot, writable)`, `MR1=base`, `MR2=offset`, `MR3=length` | `0` on mapping. |
| 24 | `SHARED BUFFER UNMAP` | `MR0=buffer_or_loan_slot`, `MR1=base` | `0` on unmap; the mapping charge returns. |
| 25 | `SHARED BUFFER SEAL` | `MR0=buffer_slot` | `0` on seal; live writable mappings are downgraded first. |
| 26 | `SHARED BUFFER LOAN` | `MR0=slot_pair(buffer_slot, receiver_supervision_slot)`, `MR1=offset`, `MR2=length` with bit 63 requesting a writable loan | Primary is the loan capability slot, auxiliary the single-return loan identity. |
| 27 | `SHARED BUFFER LOAN MAP` | `MR0=loan_slot`, `MR1=base`, `MR2=offset`, `MR3=length` | `0` on mapping, at the protection the loan was minted with. |
| 28 | `SHARED BUFFER RETURN` | `MR0=loan_slot` | `0` on the one permitted return; a second is `-1`. |
| 29 | `SHARED BUFFER REVOKE` | `MR0=buffer_slot`, `MR1=loan_id` | `0` on revoke as lender. |
| 30 | `SHARED BUFFER OCCUPANCY` | `MR0=0` | Primary `0`; the auxiliary word packs the caller's own live `pages`, `buffers`, `mappings`, `loans` as four 16-bit fields from the low bits up. Read-only and self-scoped: the holder is the badge, so no holder can be named and the operand word is ignored. A holder the generation's `sharedBufferBudget` does not declare is `-1`. |
| 31 | `CAPABILITY SLOT OCCUPANCY` | `MR0=0` | Primary `0`; the auxiliary word packs the caller's own `declared`, `declared_peak`, and `populated` as three 16-bit fields from the low bits up. `declared` and `declared_peak` are the live count and the root-tracked high-water mark in the component's own logical slot numbering — the space `capabilitySlots` budgets. `populated` is its physical CNode occupancy, where a logical index resolves to a fixed higher address; that space's bound is the CNode's capacity, a compile-time constant of this root rather than a per-holder fact, so it is not shipped. The two spaces are reported separately because their bounds differ. Read-only and self-scoped: the CSpace counted is the badge's, so no task can be named and the operand word is ignored. The generation's graph-wide `capabilitySlots` ceiling is deliberately not reported, so the query discloses no graph shape. `populated` is a fresh kernel census, so it includes capabilities the component installed itself; `declared`/`declared_peak` are root-credited, since every install into that space is a root operation. Needs no right; an unknown task is `-1`. |
| 32 | `SUPERVISION DERIVE` | `MR0=supervision_slot` | A second handle naming the same task, at the source's own rights (B25). Non-consuming; requires `RIGHT_SUPERVISE`. |
| 33 | `CAPABILITY EXPORT` | `MR0=slot_pair(endpoint_slot, capability_slot)`, `MR1=expected_kind` with the disposition in bit 32, `MR2=transfer descriptor` over the 64-byte typed descriptor, `MR3=rights_mask` | Export id, or a negative error. |
| 34 | `CAPABILITY IMPORT` | `MR0=0` | The slot the claimed capability landed in. |
| 35 | `CAPABILITY EXPORT CANCEL` | `MR0=export_id` | `0` on cancel; restores the source. |
| 36 | `CAPABILITY EXPORT FINALIZE` | `MR0=export_id` | `0` once the receiver-bound export commits. |
| 37 | `CAPABILITY RESOLVE BINDING` | `MR0=0`, `MR1=name length`, `MR2=transfer descriptor` over the binding name bytes | The caller's own logical slot holding that binding, or a negative error. Read-only and self-scoped: the instance resolved is the badge's, so no task can be named and there is no caller identity to forge. An unprefixed name is a manifest grant. `kind:<capabilityKind>` or `kind:<capabilityKind>+<right>,<right>` instead asks by what the capability *is*, over the caller's own bindings, because grant names are not stable across generations and so cannot be written into a component; kind matches exactly, rights are a superset test, an unknown kind or right is refused rather than widened, and a role matching more than one binding is refused rather than resolved to one of them. `executable:<name>` and `channel:<name>` instead address `contracts/boot-layout/v1`'s two identity domains and are answered only for the bootstrap instance, whose CSpace that table describes; the prefix is required because the two tables use overlapping names for different things, so an unprefixed layout lookup would answer a channel question with an executable slot. `minted:<name>` resolves a `mintedBindings` record for the caller's own holder index -- the slot the generation fixes for an object its holder's owner creates at runtime, such as a supervision handle that cannot exist before the task it names. `notification:<grant>` (optionally `+signal`/`+wait`) resolves a `notificationBindings` record, which is a separate declaration from capability grants: one notification grant binds a slot in *both* peers, so the answer is scoped to the caller's own holder index and that scoping is the meaning rather than a restriction. A name the caller's instance does not bind answers `-4`, never another instance's slot — which is what makes this safe to serve to every component (CP2). |
| 38 | `CAPABILITY GRAPH READ` | `MR0=cursor`, `MR1=0`, `MR2=transfer descriptor` selecting the caller's reply window | The declared participant rows of this generation's fabric graph, from `cursor` onward, written into the caller's window and answered as the count returned plus a descriptor; the caller resumes from `cursor + count` until the count is short. Scoped by who asks. The instance the graph names as its fabric component reads every row -- `FabricGraph` carries a `fabricComponentIdentity` and the root already folds instance names to that identity to admit the graph, so the test is a property of the generation rather than a policy judgement. Every other instance reads its *own* rows plus the rows of components it shares a declared capability edge with -- the first is the scoping `RESOLVE_BINDING` applies to bindings, the second exists for a route worker brokering for participants it neither spawned nor holds the graph of, and discloses nothing new since the caller already holds an endpoint the root placed from the manifest. `cursor` counts the rows that caller may see rather than rows of the table, so a participant cannot infer where its rows sit among everyone else's. A caller with no declared rows reads nothing rather than being refused, since a missing graph and an empty share are different facts. Enumerating the graph stays impossible for a non-holder, so C8.8's per-caller route filtering remains the fabric's to enforce and `sel4_visibility_check`'s ungranted-caller assertion is untouched. Paged because one record is 128 bytes against a 64-byte message bound; a call answers at most `MAX_STAGED_ARRAY_BYTES / 128` rows (B70). |
| 39 | `CAPABILITY GRAPH ROUTE INDEX` | `MR0=0`, `MR1=0`, `MR2=transfer descriptor` over the 32-byte route identity | The graph's index for that route, or a negative error where the generation embeds no graph or declares no such route. A participant knows its route by identity -- it folds the route name, its interface identity, and the contract kind exactly as the builder does -- while a participant row names the route by index into a table sorted by that identity, so this resolves the two without a component assuming the resource's sort order. Unscoped and safe for any caller: the identity is one the asker already holds, so the answer confirms a fold it computed itself and names no route it did not already name (B70). |
| 40 | `CAPABILITY BOOT ACTION` | `MR0=0` | The `BootAction` id the authenticated generation declares (`boot-contracts/src/generation.rs`), as a nonnegative primary; the operand word is ignored. Unscoped, because a boot action is a property of the one generation every caller already runs inside rather than of any instance within it, so there is no per-caller answer to leak and no identity to forge. It names no route, component, slot, or capability, so unlike `CAPABILITY GRAPH READ` it discloses no graph shape — a caller learns only which composition it is part of, which its own declared behavior already depends on. The frozen numeric id crosses, never the source spelling: the root already delivers the same id as the bootstrap thread's first C parameter, and answering with it keeps one encoding for both delivery paths. This exists because the eleven fabric participants that branch on the composition are *not* the bootstrap instance and so were never told, forcing the string to be compiled in from a `build.rs`-private per-plane table (B70). Gated on the **lifecycle** service rather than the capability table its label namespace belongs to: the service is the authority gate, and this is the one operation that must be answerable to every launched instance. `declared_services` grants the capability-transfer service only to an instance with a spawn budget, an endpoint, or a transferable grant, which 30 of the 182 instances the seL4 fixtures declare do not have; every caller reads a refusal as “not this plane”, so gating there would select a component's schedule by what it can delegate. |
| 41 | `CAPABILITY GRAPH QUERY` | `MR0=field` | The selected scalar from the authenticated fabric graph header: table cardinality or declared resource ceiling. Query ids are frozen `QUERY_*` constants generated from `contracts/fabric-graph/v1/schema.zt`; an unknown id or a generation with no graph is refused. Table cardinalities and other graph-shape fields are holder-only. The generated `RuntimeLimits` subset is also available to any participant with at least one row visible through `CAPABILITY GRAPH READ`, so independently built workers admit traffic against the authenticated generation ceilings without compiling a per-generation profile. A caller with no visible row is refused identically. |
| 42 | `CAPABILITY SPAWN BUDGET` | `MR0=0` | The `spawnBudget` this generation declares for the caller's own executable, as a nonnegative primary; the operand word is ignored. Read-only and self-scoped like the two `OCCUPANCY` operations: the executable resolved is the badge's, so no instance can be named and there is no caller identity to forge. It is the same number the root already reads to bound `serve_spawn`, so a caller learns the ceiling it is about to be admitted against and nothing about anyone else. Gated on the `spawn` service: an instance the generation grants no spawn authority is refused rather than answered zero, because zero is a number a caller could act on while the refusal says the question does not apply. `spawn-service` sizes its live-child table and validates each request's `client_budget` against this, and `dango` states it in every request, so neither compiles a manifest-derived budget (B70). |
| 43 | `LIFECYCLE PRIVATE MEMORY GROW` | `MR0=delta`, a page count as an unsigned word | Primary is the caller's private-memory page count *before* the growth, so an allocator learns where its region ended without a second call; auxiliary is the window's base address, answered rather than derived because the root is what chose it. `delta = 0` is therefore a pure size query that allocates nothing and still reports the base. Growth is all-or-nothing: an attempt that fails part way through unmaps and returns every frame it took, leaving the page count and every existing mapping unchanged. Pages are freshly zeroed and always mapped user/read-write/execute-never, so no growth can yield an executable mapping. Self-scoped like the two `OCCUPANCY` operations: the request carries a delta and no task argument, so a caller can only grow its own region and there is no identity to forge. Refused with `-5` when the delta would overflow, pass the reserved window, exceed the caller's declared quota, exceed the root-wide ceiling every region shares, or exhaust frames; the causes are distinguished by the root's own `SLIME_MEM` markers rather than by the wire status, so a component cannot map the root's internal predicates. Gated on the `lifecycle` service, the one every launched instance holds, because a private heap is a property of being a task rather than of any grant — a task with no declared quota is refused by its zero ceiling, which is a budget answer rather than an authority one. The region is not nameable, transferable, loanable, sealable, or mappable by another component, so `docs/capability-matrix.md` is unchanged: C10 adds no object kind and no right (C10.1). |

A label with no surviving mechanism is refused with `-4` and reported as
`SLIME_GRAPH unsupported service`; the caller survives.

## Compatibility and versioning

This is the contract a component crate outside this repository builds against.
CP3 makes that a real audience: a component is its own crate depending on
`slime-rt` by pinned commit, so it can be compiled against one revision of this
ABI and admitted into a generation built from another.

Within `contracts/syscall-abi/v1` (`formatVersion = 1`):

- **Labels are frozen.** A number, once assigned, keeps its operation, its
  operand packing, and its result convention forever. `contracts/syscall-abi/v1/schema.zt`
  declares each label explicitly rather than by position, so a reordering of the
  list cannot renumber anything, and `components/proto/tests/syscall_abi.rs`
  pins every number against renumbering.
- **Growth is additive only**, into values no operation has used. New operations
  take the next unused label; they never reuse one.
- **Retired numbers stay reserved.** An operation whose mechanism is deleted
  leaves its label unassigned rather than freeing it for reuse, and the root
  refuses it with `-4` (`SLIME_GRAPH unsupported service`) while the caller
  survives. That refusal is the compatibility mechanism: a component built
  against a newer ABI that names an operation this generation does not implement
  is denied one call, not mis-served a different one. The gap the
  `routes-nowhere` control in `slime-root/src/ipc.rs` guards is the live
  example, and it is guarded rather than merely documented: assigning a label
  must remove it from that control, which is why label 40 left it when
  `BOOT_ACTION` took the number.

  The retired trap ABI's `SYS_*` 0–30 are *not* examples of this rule. Those
  were `int 0x80` syscall numbers in a namespace that no longer exists, not
  operation labels, and this table reuses several of those numbers for live
  operations — `3` is `LIFECYCLE EXIT`, `4` is `SPAWN`, `9` is `UNHEALTHY`. The
  reservation rule binds a number only once it has been assigned *as a label in
  this namespace*; it says nothing about the deleted one.
- **Statuses are frozen** in the same way as labels, since a caller branches on
  them. `## Error model` below is the whole set.
- **An incompatible change is a new major contract version** — a new
  `contracts/syscall-abi/vN` directory — never a reinterpretation of a v1 label.
  The superseded schema is retained as format history and type-checked, never
  generated from, exactly as retired `contracts/generation/vN` schemas are.
  Roadmap invariant 7 is why: a format bump is rollback-*safe by refusal*
  rather than rollback-compatible by migration, so a v1 component meeting a v2
  root must be refused rather than guessed at.

What is *not* promised: the argument-passing mechanism below the label. Which
registers carry an operand, whether a payload rides inline or through a transfer
window, and the badge layout are implementation details of
`components/runtime/src/syscall/sel4_transport.rs` and the root's dispatcher,
versioned together with them. A component reaches them only through `slime-rt`,
which is why the SDK pins a `slime-rt` commit rather than reimplementing the
transport.

`just contracts_check` enforces the documentation half of this: every label the
contract declares is documented here, and every numeric row here is a declared
label (`scripts/generate/generate-syscall-abi-bindings.py --check`). It compares
label coverage, not prose, so the policy above is enforced by review and by the
frozen-label test, not by that gate.

## Console service operations

| Label | Operation | Operands | Result convention |
| --- | --- | --- | --- |
| 0 | `WRITE` | `MR0`=transfer descriptor (or inline registers) over the bytes | Bytes written. One line is emitted as one uninterruptible unit (B18), bounded by `MAX_STAGED_ARRAY_BYTES` (1024) rather than by `MAX_MSG`. |
| 1 | `INPUT READ` | `MR0=input_slot` | Primary `0` with the encoded event in the auxiliary word, `-3` when no event is ready. Requires `RIGHT_INPUT_READ`. |
| 2 | `BLOCK TRANSACT` | `MR0=block_slot`, `MR1=transfer descriptor` over the 64-byte request | `0` means delivered; the block outcome is in the returned reply record. Sector payloads ride behind the record in the same window. |
| 3 | `DIRECTORY INSPECT` | `MR0=slot_pair(directory_slot, required_rights)`, `MR1=reserved window descriptor` | Nonnegative scope byte length; the immutable root and scope return through the window. |
| 4 | `DIRECTORY COMMIT` | `MR0=directory_slot`, `MR1=transfer descriptor` over expected‖new root | `0` on commit, `-3` when the expected root is stale. |

Directory *derive* is deliberately on the root service instead: it is the only
one of the three that writes the caller's capability table, which the graph
dispatcher also writes, and two threads writing one task's table is a race.

## Reply and transfer conventions

A reply carries the logical `i64` result in `MR0` and a service-specific
auxiliary value or transfer descriptor in `MR1`. A reply with no result register
is malformed and is reported as `-4`, never as a silent success.

At most four message registers cross in each direction. Payloads of at most 16
bytes with no capability ride inline in `MR2`/`MR3` (`FORM_INLINE`); anything
larger, and anything carrying capability slots, rides in the caller's
root-mapped startup transfer window (`FORM_WINDOW`), described by a descriptor
register packing payload length, capability count, carrier form, and the sending
thread's window index. The thread index is invocation metadata, not authority:
the root already authenticated the process from the badge and uses it only to
select which of that process's windows to read. A payload that does not fit its
window is refused, never truncated.

## Error model

Negative results are errors; nonnegative results have the per-operation meaning
above. The constants are `components/runtime/src/syscall.rs`; the root maps its
own `IpcError` onto the same values in `slime-root/src/ipc.rs`.

| Value | Constant | Meaning |
| --- | --- | --- |
| 0 | `ERR_SUCCESS` | Successful completion or delivery. |
| -1 | `ERR_BAD_CAP` | Missing capability, wrong object kind, or insufficient rights — one code for all three so a probe cannot map its own table. Also answers a capability that will not move. |
| -2 | `ERR_PEER_DEAD` | The peer is gone. |
| -3 | `ERR_WOULDBLOCK` | Not ready without blocking, or a stale optimistic state check. |
| -4 | `ERR_INVALID_ARG` | Bad argument, length, descriptor, request, or unsupported label. |
| -5 | `ERR_OUT_OF_MEMORY` | A task, capability, frame, object, byte, mapping, loan, or declared quota bound is exhausted. |

`SUPERVISION STATUS` uses nonnegative primaries as typed terminations rather
than plain success. `BLOCK TRANSACT` uses `0` to mean delivered, with the device
outcome inside the reply record.

## Declared service admission

An operation label is not reachable merely because it exists. Each label maps to
a service id (`service_for_root_label` in `slime-root/src/main.rs`), and the
caller's generation must carry a service binding for that id at the endpoint's
slot, or the request is refused with `-1` before any argument is read. The ids
are generated from the generation contract
(`boot-contracts/src/generated/generation.rs`): `1` lifecycle, `2` spawn,
`3` supervision, `4` capability transfer, `5` shared buffer, `6` directory,
`7` input, `8` block, `9` console. Lifecycle and console are required of every
instance; spawn, supervision, and capability transfer are required of any
instance holding a spawn budget or an executable grant; shared buffer is
required of any instance with a budget entry.

## Child CSpace layout

Slot numbers are fixed by `slime-root/src/task.rs` and mirrored by the runtime's
transport. A component's generation grants number their own logical slots from
0; those are indices into the regions below, not raw CPtrs.

| Slot(s) | Contents |
| --- | --- |
| 0 | null |
| 1 | badged root service endpoint |
| 2 | the task's own TCB, when supervised |
| 3 | badged fault-handler endpoint |
| 4 | the CSpace's own root CNode |
| 5–31 | received-endpoint handle region: a transferred Endpoint is relocated out of the receive slot into the first free slot here and named by its handle tag |
| 32 | badged console/debug endpoint |
| 33–63 | declared native Endpoints |
| 64–94 | declared Notifications |
| 95–125 | badged logical-authority mirrors |
| 127 | receive slot for the single capability a native receive may carry |

## Bounds

| Bound | Value | Owner |
| --- | --- | --- |
| Payload bytes per message | `MAX_MSG = 64` | `components/runtime/src/syscall.rs`, `slime-root/src/ipc.rs::MAX_MESSAGE_BYTES` |
| Capabilities per message | `MAX_CAPS_PER_MSG = 1` | seL4 carries one per IPC |
| Fast message registers | `FAST_REGISTERS = 4` | asserted equal to `sel4::NUM_FAST_MESSAGE_REGISTERS` |
| Inline payload bytes | `INLINE_BYTES = 16` | `components/runtime/src/syscall/wire.rs` |
| Staged array bytes | `MAX_STAGED_ARRAY_BYTES = 1024` | `slime-root/src/transfer_window.rs` |
| Transfer window bytes | `MIN_TRANSFER_WINDOW = 4096` | root-mapped at thread construction |
| Spawn grants per call | `MAX_SPAWN_GRANTS = 64` | matches the per-task capability capacity the root checks against |

## Architecture

The product target is `aarch64` under seL4 (`sel4/config/qemu-arm-virt.cmake`).
Register-level trap entry is seL4's own, not Slime's: components invoke through
the `sel4` crate's `seL4_Call`/`seL4_Send`/`seL4_NBSend`/`seL4_Recv`/`seL4_Yield`
wrappers and the per-thread IPC buffer, so Slime defines no calling convention
of its own and no `arch::<target>::trap` frame accessors exist. AArch64 fault
entry is decoded from seL4's fault messages by `slime-root/src/fault.rs` into the
architecture-neutral fault vocabulary supervision reports.

Porting to another architecture therefore changes the seL4 configuration and the
platform mechanisms (`slime-root/src/platform_timer.rs`, the device path), not
this table: labels, operand packings, reply convention, error values, bounds, and
rights checks are architecture-neutral by construction.
`just x86_portability_check` scans the neutral Rust trees for x86-only tokens to
keep that true; RV64 stays deferred until after the Raspberry Pi 5 demo.
