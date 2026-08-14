# Slime OS capability matrix

The canonical object-by-rights surface, the rules for extending it, and the
planned-but-undecided authority horizon. Update this file in the same change
that adds an object kind or right; a row without a gate is a bug, not a plan.

Authority lives in three places and they must agree: the declared kinds and
rights in `boot-contracts/src/generation.rs` (validated against
`contracts/generation/v1/schema.zt`), the rights types the root enforces in
`slime-root/src/graph.rs`, and the operations that check them in
`slime-root/src/main.rs`'s dispatcher and the owning mechanism module. Rights
numbering is generated-contract truth, not prose.

## Grammar

Every new object kind or right must satisfy these rules before it ships:

1. One rights bit names exactly one root-checked operation, never a policy
   concept. Policy — who may hold the capability — belongs in the generation
   manifest and userspace services.
2. A new right ships with at least one gate: an operation site that checks it.
3. Every transfer path checks transferability on each moved capability. The
   current paths are the capability-export protocol (`CAPABILITY EXPORT` /
   `IMPORT` / `EXPORT_CANCEL` / `EXPORT_FINALIZE`) and `SPAWN` grants
   (`task::preflight_spawn_grant`). Any future path inherits the rule.
4. `capability_rights_valid` rejects rights meaningless for the kind, and
   requires each kind's mandatory bits. `narrow` narrows only and never widens;
   a `rights_type!` refuses bits outside its own `VALID` mask.
5. Object creation authority is root-only. Native Endpoints and Notifications
   are seL4 objects the root creates while decoding the generation; userspace
   cannot forge object identities and can only hold, derive, and transfer. The
   one runtime mint is `SHARED BUFFER CREATE` through a `SharedBufferFactory`,
   and it is bounded by a declared budget as well as gated by a right.
6. Every resource table has a hard bound (see Bounds). "Unbounded" is not an
   error-handling strategy.
7. Rights constants are named `RIGHT_<OBJECT>_<OPERATION>`.
8. Generation v5 maps manifest grant rights strings 1:1 to bit names and
   `transferable` to `RIGHT_TRANSFER`. Every generation this repository builds
   is v5; `just contracts_check` proves it by building each fixture and reading
   the magic.

## Capability kinds

The nine declared kinds are `CapabilityKind` in `boot-contracts/src/generation.rs`,
numbered by `boot-contracts/src/generated/generation.rs`.

| Kind | Number | What it names | Where its operations are served |
| --- | --- | --- | --- |
| `Endpoint` | 1 | one side of a declared native seL4 Endpoint edge | the kernel; no root mediation |
| `Executable` | 2 | generation-module bytes verified at boot | root service `SPAWN` |
| `SharedBufferFactory` | 3 | authority to mint shared buffers | root service `SHARED BUFFER CREATE` |
| `Block` | 4 | one enumerated block device | console service `BLOCK TRANSACT` |
| `Directory` | 5 | a namespace root, possibly scoped | console service inspect/commit, root service derive |
| `Input` | 6 | the decoded key source | console service `INPUT READ` |
| `Supervision` | 7 | one spawned task's outcome | root service `SUPERVISION STATUS` / `DERIVE` |
| `SharedBuffer` | 8 | one allocated buffer | root service shared-buffer operations |
| `Loan` | 9 | a receiver-bound loan of a subrange | root service loan operations |

## Current matrix

Rights are a flat `u64`. `RIGHT_ALL = (1 << 26) - 1`, so bits 26–63 are free.

| Object | Right (bit) | Gated operation | Creation authority | Gate status |
| --- | --- | --- | --- | --- |
| Endpoint | SEND (0) | `seL4_Send`/`seL4_Call`/`seL4_NBSend` on the declared edge; the root mints the child's cap at send-only rights | root, from the generation's declared edges (`slime-root/src/peer_endpoint.rs`) | gated |
| Endpoint | RECV (1) | `seL4_Recv` on the declared edge; the root mints at receive-only rights | same | gated |
| *(meta, most kinds)* | TRANSFER (2) | `CAPABILITY EXPORT` and transferable derived spawn grants | — | gated on both paths |
| Executable | EXEC (3) | executable slot validation in `SPAWN` | generation module only, hash-verified at boot | gated |
| SharedBuffer | BUFFER_WRITE (8) | writable `SHARED BUFFER MAP`; irreversible `SHARED BUFFER SEAL` | `SHARED BUFFER CREATE` via a `SharedBufferFactory`; root-assigned identity | gated (C7.4) |
| SharedBuffer | BUFFER_MAP (9) | read-only `SHARED BUFFER MAP`; exact `SHARED BUFFER UNMAP` | same | gated (C7.4) |
| Block | BLOCK_READ (10) | read requests in `BLOCK TRANSACT` for the capability's exact device | root bootstrap from the generation's declared device | gated |
| Block | BLOCK_WRITE (11) | write and flush requests in `BLOCK TRANSACT` | same | gated (M5.3) |
| Executable | SPAWN (16) | instance launch in `SPAWN`; always travels with EXEC | generation manifest | gated (M6.1) |
| Supervision | SUPERVISE (18) | `SUPERVISION STATUS` and `SUPERVISION DERIVE` | returned by a successful `SPAWN` | gated (M6.1/B25) |
| Directory | DIRECTORY_READ (19) | `DIRECTORY INSPECT` before filesystem reads | root bootstrap from the generation's declared root | gated (M6.3) |
| Directory | DIRECTORY_WRITE (20) | `DIRECTORY INSPECT` before mutation and `DIRECTORY COMMIT` for atomic root swap | same | gated (M6.3) |
| Directory | DIRECTORY_LIST (21) | `DIRECTORY INSPECT` before bounded enumeration | same | gated (M6.3) |
| Directory | DIRECTORY_DERIVE (22) | `DIRECTORY DERIVE` for a subdirectory-scoped, narrow-rights copy | same; powerbox minting needs only this operation | gated (M6.3) |
| Input | INPUT_READ (23) | `INPUT READ` drains one decoded key event | root bootstrap, only through a generation grant | gated (M6.4) |
| SharedBufferFactory | BUFFER_CREATE (24) | `SHARED BUFFER CREATE` mints a root-identified `SharedBuffer` under fixed global byte/object bounds; `SHARED BUFFER RELEASE` reclaims it | generation manifest | gated (C7.2) |
| SharedBuffer | BUFFER_LOAN (25) | `SHARED BUFFER LOAN` mints an exact loan for a named receiver; `SHARED BUFFER REVOKE` settles it as lender | same | gated (C7.5) |
| Loan | BUFFER_MAP (9) / BUFFER_WRITE (8) | receiver-bound `SHARED BUFFER LOAN MAP` within the loaned subrange at the loan's own protection; `SHARED BUFFER RETURN` settles it once | root-created by `SHARED BUFFER LOAN`; delivered to the named receiver only | gated (C7.5) |

Bits 4–7 and 12–15 are unassigned. The retired custom kernel spent them on
`PciFunction`, `DmaMemory`, `Irq`, `ObjectStore`, and `GenerationControl` object
kinds; those kinds and their syscalls were deleted with that kernel. Their
mechanisms live where they belong now: sectors behind a `Block` capability, and
the object store, generation management, rollback, and recovery entirely in
userspace components over that capability
(`components/bins/src/bin/sel4-{store,generation-manager,rollback,recovery}-*.rs`,
`boot-contracts/src/object_store.rs`). Reassign the bits deliberately; do not
assume they still mean what a pre-cutover document says.

Semantics not visible in the table:

- Receiving a capability costs the receiver no rights bit; it arrives with
  exactly the rights the sender named and root authenticated.
- `derive` and spawn grants are non-consuming and narrow-only. A derived copy
  that retains TRANSFER requires that meta-right on its source.
- The capability-transfer protocol is the *consuming* movement path, and it is
  four messages rather than one: `EXPORT` reserves a receiver-bound export and,
  for a native Endpoint, mints the real kernel ticket the message will carry;
  the descriptor and (for an Endpoint) that ticket cross the declared native
  edge; `EXPORT_FINALIZE` commits and `EXPORT_CANCEL` restores the source at its
  original rights. Root authenticates the declared object kind and the nonzero
  rights mask from the request registers, never from the descriptor bytes.
- A native Endpoint is the one kind with a kernel object to travel in the
  message; it lands in the receiver's receive slot and is relocated into its
  transferred-handle region. Every other kind is a root-owned logical capability
  with nothing for the kernel to carry, so the descriptor arrives alone and the
  receiver takes up the authority behind it with `CAPABILITY IMPORT`.
- Only `Endpoint`, `SharedBuffer`, `Loan`, `Supervision`, and `Directory` are
  nameable by a transfer (`capability_kind` in `slime-root/src/main.rs`). No
  other kind can move; an `Executable` or a factory reaches a child as a spawn
  grant or a declared binding, never as a transfer.
- The 64-byte `capability-transfer/v1` descriptor is opaque to the root:
  `route_identity` and `direction` are bytes a userspace fabric uses to bind the
  move to a declared edge, and `FLAG_RETAIN_TRANSFER` is the descriptor's own
  record of a disposition the request registers already carried.
- Shared-buffer mappings are page-aligned, non-executable, charged one unit per
  live map to the mapper's generation quota, and never overwrite an existing
  user page. `SHARED BUFFER SEAL` downgrades every live writable mapping before
  publishing the irreversible read-only state; release and subtree teardown
  remove PTEs before returning frames.
- Subdirectory-scoped capabilities may browse and derive further but cannot
  commit the namespace root. Root transitions require an unscoped WRITE
  capability; scoped writes are rejected before object-store I/O.
- Spawn retains the executable and all grant sources. It returns one supervision
  handle and is bounded independently per spawner by the manifest's `spawnBudget`
  plus the global live-task ceiling.
- Spawned code cannot be injected: `Executable` objects reference only
  generation-module bytes verified at boot. Spawn composes known components with
  gifted authority; it cannot introduce new code.
- Shared-buffer allocation is charged to the creating component's
  supervision-subtree account against its generation-declared per-holder quota
  (`shared-buffer-budget/v1`). A component absent from the budget holds the
  deny-by-default quota and cannot allocate. Release, peer death, supervised
  restart, and revocation reclaim every page and charge in that subtree without
  disturbing another subtree's account (C7.3/C7.5). Mapping quota is consumed
  and reclaimed by `SharedBufferTable::map` (C7.4); loan quota by
  `SharedBufferTable::loan`, and a released creator's pages and buffer charge
  stay retained until the final single-return loan settles.
- The reference generation declares that budget in practice, not only in
  principle: `contracts/generation/v1/fixtures/valid.zti` carries a
  `shared-buffer-budget` `KIND_RESOURCE` object with eleven holder quotas and
  seven `bufferCreate` grants. The grant authorizes the operation and the budget
  bounds it: a component with a grant but no budget entry still allocates
  nothing, and a budget entry without a grant authorizes nothing.
- A C7.6 sample descriptor is a userspace control message
  (`sample-descriptor/v1`), not a root object: it references an exact
  transferred `Loan` by its unforgeable identity plus a page-aligned
  offset/length, type identity, sequence, and known flags. It carries no rights
  bit and mints no authority — the receiver still needs the loan capability to
  map. The descriptor fits one message (`DESCRIPTOR_LEN == MAX_MSG == 64`), so a
  sample larger than the message bound crosses as descriptor plus shared buffer
  without copying payload bytes through IPC or widening `MAX_MSG`.
- C8.3 makes the C8.2 fabric graph load-bearing without teaching the root about
  it. A userspace `fabric-service` receives both halves of each declared route
  as generation-declared native Endpoint edges and delegates each participant
  one role: SEND to the publisher, RECV to the subscriber, TRANSFER to neither.
  A client is authenticated by the generation-provisioned control endpoint its
  request arrived on — a binding init establishes at spawn and no component can
  forge — never by the route name, direction, or type identity the request
  carries. Ignoring those fields is a tested property:
  `just fabric_authority_check` boots a component that supplies byte-identical
  route strings for an edge the graph never declared and observes a denial with
  no capability attached.
- A `mintedBindings` entry authorizes a holder to *receive* a named capability at
  a declared slot with an exact rights ceiling, deferring only the object
  identity to the owner that creates it. It is admission, not authority: nothing
  arrives until a real delegation crosses.

## Bounds

| Resource | Bound | Enforcement |
| --- | --- | --- |
| Logical capabilities per task | `MAX_TASK_CAPS = 64` | `slime-root/src/graph.rs` |
| Capabilities per IPC message | `MAX_CAPS_PER_MSG = 1` | seL4 carries one per IPC; checked in `sel4_transport` and `slime-root/src/ipc.rs` |
| Payload bytes per message | `MAX_MESSAGE_BYTES = 64` | `slime-root/src/ipc.rs` |
| Live tasks | `MAX_TASKS = 48` | `slime-root/src/task.rs`; `SpawnError::TooManyTasks` |
| Supervised task records | `MAX_SUPERVISED_TASKS = 64`, `MAX_RECORDS = MAX_TASKS` | `slime-root/src/{fault,supervision}.rs` |
| Live children per spawner | manifest `spawnBudget <= 32` | `MAX_SPAWN_BUDGET`; `SpawnError::BudgetExhausted` |
| Declared native Endpoints per task | `CHILD_NATIVE_REGION_SLOTS = 31` | child CSpace regions in `slime-root/src/task.rs` |
| Peer endpoints / notifications | `MAX_PEER_ENDPOINTS = 48`, `MAX_NOTIFICATIONS = 31` | `slime-root/src/{peer_endpoint,notification}.rs` |
| Threads per component | `MAX_CHILD_THREADS = 2` | `slime-root/src/child_vspace.rs` (B47) |
| Task arenas / root CSlots | `MAX_TASK_ARENAS = 48`, `MAX_ROOT_CSLOTS = 262_144` | `slime-root/src/object_allocator.rs` |
| Live shared buffers | `MAX_SHARED_BUFFERS = 32` | `SharedBufferError::ObjectsExhausted` |
| Shared-buffer total pages | `MAX_TOTAL_PAGES = 256` (1 MiB) | `SharedBufferError::BytesExhausted` |
| Pages per shared buffer | `MAX_BUFFER_PAGES = 64` | `SharedBufferError::BadSize` |
| Per-holder shared-buffer quota | generation `shared-buffer-budget/v1` (`byte_pages`, `buffer_count`, `mapping_count`, `loan_count`); deny by default; declared totals must also fit the root ceilings, so a validating budget can be honoured with every holder at its ceiling at once | `SharedBufferTable::create`; `SharedBufferError::QuotaExceeded`; aggregate over-commit fails at generation decode |
| Live shared-buffer mappings | `MAX_MAPPINGS = 64` | `SharedBufferTable::map`; `MappingsExhausted` |
| Live shared-buffer loans | `MAX_LOANS = 64` plus generation `loan_count` per lender | `SharedBufferTable::loan`; `LoansExhausted` / `QuotaExceeded` |
| Declared holders per budget | `MAX_HOLDERS = 32` | `SharedBufferBudget::decode` |
| Directory path bytes / depth | `MAX_DIRECTORY_PATH = 128`, `MAX_DIRECTORY_DEPTH = 8` root-side; the userspace ABI and filesystem schema admit 48 bytes and depth 4 | `slime-root/src/directory.rs`; `components/runtime/src/syscall.rs`; `components/proto/src/fs.rs` |
| Directory scopes | `MAX_SCOPES = 64` | `slime-root/src/directory.rs` |
| Directory entries per snapshot | `MAX_ENTRIES = 16` | filesystem protocol and snapshot decoder |
| Boot-layout entries | `MAX_ENTRIES = 64` | `boot-contracts/src/generated/boot_layout.rs` |
| Admitted executables / instances | `MAX_ADMITTED_EXECUTABLES = 48`, `MAX_ADMITTED_INSTANCES = 48` | `slime-root/src/generation.rs` |
| Graph service iterations | `MAX_GRAPH_ITERATIONS = 32768` | `slime-root/src/main.rs` (B28); bounds a livelock rather than the gate timeout |

A task's kernel objects are allocated from a per-task arena, so termination
reclaims its TCBs, CNode, VSpace, IPC buffers, and frames rather than leaking
them until reboot; `just sel4_reclamation_check` and `just sel4_stress_check`
observe the graph returning to zero live tasks.

## Horizon (claimed directions, not decisions)

| Candidate object | Candidate rights | Trigger | Open questions |
| --- | --- | --- | --- |
| Generation-update authority as an object | possibly STAGE_PENDING | a generation service that must not hold raw block write | Boundary between userspace staging and immutable stage-0 slot writes, now that management is entirely a component over `Block` |
| NetworkDestination | CONNECT / SEND / RECV / LISTEN | the RPi5 DDS route ([RP5](../roadmap/09-rpi5-ros2-demo.md)) and [Hardware H6](../roadmap/04-platform-hardware.md) | Object shape: (protocol, address, port) declared in the generation? |
| Device/IRQ authority for userspace drivers | MAP_MMIO / DMA_PIN / IRQ_ACK | a driver outside the root ([H1](../roadmap/04-platform-hardware.md)) | Whether these are Slime kinds at all or thin wrappers over seL4's own frame and IRQ handler capabilities |
| EnergyAccount | READ? | [Hardware H track](../roadmap/04-platform-hardware.md) | Whether accounting is authority at all or read-only telemetry |

M6.1 landed non-consuming narrow derive-copy spawn grants, per-spawner
accounting, and supervision handles; B39–B50 moved endpoint creation from a
userspace factory syscall to generation-declared native seL4 objects. Future
resource factories follow the same named-capability and hard-bound rules that
`SharedBufferFactory` does: a right to ask, a declared budget to bound it, and a
root-assigned identity userspace cannot forge.
