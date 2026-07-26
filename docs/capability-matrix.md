# Slime OS capability matrix

The canonical object-by-rights surface of the kernel, the rules for extending
it, and the planned-but-undecided authority horizon. Update this file in the
same change that adds an object or right; a row without a gate is a bug, not
a plan.

## Grammar

Every new object or right must satisfy these rules before it ships:

1. One rights bit names exactly one kernel-checked operation, never a policy
   concept. Policy (who may hold the capability) belongs in the generation
   manifest and userspace services.
2. A new right ships with at least one gate (an operation site that checks
   it). The six ungated M5.1 bits below are the last grandfathered exception;
   they gain gates when the userspace-driver path lands.
3. Every transfer path checks `RIGHT_TRANSFER` on each moved capability.
   Current paths: `SYS_SEND` cap attachment and `SYS_SPAWN` grants
   (`task::preflight_spawn_grant`). Any future path inherits the rule.
4. `CapabilityTable::insert` rejects rights meaningless for the object kind
   (`KernelObject::valid_rights`). `derive` narrows only and never widens.
5. Object creation authority is kernel-only unless this matrix names a mint
   operation. Userspace cannot forge object identities; it can only hold,
   derive, and transfer.
6. Every resource table has a hard bound (see Bounds). "Unbounded" is not an
   acceptable error-handling strategy.
7. Rights constants are named `<OBJECT>_<OPERATION>`.
8. Generation format v3 must map manifest grant rights strings 1:1 to bit
   names and `transferable` to `RIGHT_TRANSFER`; bootstrap wiring must be
   derived from manifest data, not hardcoded. Format v2 generations remain
   decodable for the bounded rollback window; their 32-bit rights widen to
   `u64` without changing meaning.

## Current matrix

Rights are a flat `u64` (generation format v3); bits 26–63 are free.

| Object | Right (bit) | Gated operation | Creation authority | Gate status |
| --- | --- | --- | --- | --- |
| Endpoint | SEND (0) | `SYS_SEND` via this endpoint | kernel `ipc::channel()` or `SYS_ENDPOINT_CREATE` with factory cap | gated |
| Endpoint | RECV (1) | `SYS_RECV` on this endpoint | same | gated |
| *(meta, any cap)* | TRANSFER (2) | cap attachment in `SYS_SEND`; transferable derived spawn grants | — | gated on both paths |
| Executable | EXEC (3) | executable slot validation in `SYS_SPAWN` | generation module only, hash-verified at boot | gated |
| PciFunction | MAP_MMIO (4) | future map-BAR operation (userspace driver path) | kernel PCI enumerator | **ungated** |
| PciFunction / DmaMemory | DMA_PIN (5) | future pin operation | kernel DMA allocator on a driver's behalf | **ungated** |
| DmaMemory | DMA_RELEASE (6) | future release/reclaim operation | same | **ungated** |
| Irq | IRQ_ACK (7) | future ack operation | kernel interrupt subsystem | **ungated** |
| SharedBuffer | BUFFER_WRITE (8) | writable `SYS_SHARED_BUFFER_MAP`; irreversible `SYS_SHARED_BUFFER_SEAL` | `SYS_SHARED_BUFFER_CREATE` via a `SharedBufferFactory` cap; kernel-assigned identity | gated (C7.4) |
| SharedBuffer | BUFFER_MAP (9) | read-only `SYS_SHARED_BUFFER_MAP`; exact `SYS_SHARED_BUFFER_UNMAP` | same | gated (C7.4) |
| BlockDevice | BLOCK_READ (10) | read requests in `SYS_BLOCK_TRANSACT` for the capability's exact PCI function | kernel bootstrap | gated |
| BlockDevice | BLOCK_WRITE (11) | write and flush requests in `SYS_BLOCK_TRANSACT`; receiver writes in `SYS_GENERATION_RECEIVE` require this together with BLOCK_READ and BOOT_UPDATE | kernel bootstrap | gated (M5.3/M6.7) |
| ObjectStore | STORE_READ (12) | stat/get requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |
| ObjectStore | STORE_WRITE (13) | put requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |
| GenerationControl | HEALTH_CONFIRM (14) | `SYS_HEALTH_CONFIRM` for the currently running pending generation | kernel bootstrap, only for the declared generation-management service | gated (M5.6) |
| GenerationControl / BlockDevice | BOOT_UPDATE (15) | `SYS_GENERATION_TRANSACT` for validated staging/select/rollback; `SYS_RECOVERY_RECONSTRUCT` after signed-index, generation, state-closure, and release scrub; receiver mutation in `SYS_GENERATION_RECEIVE` only with BLOCK_READ and BLOCK_WRITE | kernel bootstrap, only for the declared generation-management/recovery service or the generation-declared transfer receiver | gated (M5.9/M6.5/M6.7) |
| BlockDevice | TRANSFER (2) | source reads in `SYS_GENERATION_RECEIVE`, together with BLOCK_READ; the receiver requires BLOCK_READ, BLOCK_WRITE, and BOOT_UPDATE and does not require TRANSFER | generation manifest names the exact source and receiver PCI functions | gated (M6.7) |
| Executable | SPAWN (16) | executable slot validation in `SYS_SPAWN` | generation manifest | gated (M6.1) |
| EndpointFactory | ENDPOINT_CREATE (17) | `SYS_ENDPOINT_CREATE` | generation manifest | gated (M6.1) |
| Supervision | SUPERVISE (18) | `SYS_SUPERVISION_STATUS` | returned by successful `SYS_SPAWN` | gated (M6.1) |
| Directory | DIRECTORY_READ (19) | `SYS_DIRECTORY_INSPECT` before filesystem reads | kernel bootstrap from the generation's declared root | gated (M6.3) |
| Directory | DIRECTORY_WRITE (20) | `SYS_DIRECTORY_INSPECT` before mutation and `SYS_DIRECTORY_COMMIT` for atomic root swap | same | gated (M6.3) |
| Directory | DIRECTORY_LIST (21) | `SYS_DIRECTORY_INSPECT` before bounded enumeration | same | gated (M6.3) |
| Directory | DIRECTORY_DERIVE (22) | `SYS_DIRECTORY_DERIVE` for a subdirectory-scoped, narrow-rights copy | same; powerbox minting needs only this operation | gated (M6.3) |
| Input | INPUT_READ (23) | `SYS_INPUT_READ` drains one decoded keyboard event | kernel bootstrap, only through a generation grant | gated (M6.4) |
| SharedBufferFactory | BUFFER_CREATE (24) | `SYS_SHARED_BUFFER_CREATE` mints a kernel-identified `SharedBuffer` under fixed global byte/object bounds; `SYS_SHARED_BUFFER_RELEASE` reclaims it | generation manifest | gated (C7.2) |
| SharedBuffer | BUFFER_LOAN (25) | `SYS_SHARED_BUFFER_LOAN` mints an exact sealed-region loan for a named receiver; `SYS_SHARED_BUFFER_REVOKE` settles it as lender | same | gated (C7.5) |
| SharedBufferLoan | BUFFER_MAP (9) | receiver-bound read-only `SYS_SHARED_BUFFER_LOAN_MAP` within the loaned subrange; `SYS_SHARED_BUFFER_RETURN` settles the loan once | kernel-created by `SYS_SHARED_BUFFER_LOAN`; transfer delivers it to the named receiver only | gated (C7.5) |

Semantics not visible in the table:

- Receiving a capability over IPC costs the receiver no rights bit; the cap
  arrives with exactly the rights the sender attached.
- `derive` and spawn grants are non-consuming and narrow-only. A derived copy
  that retains `TRANSFER` requires that meta-right on its source.
- Shared-buffer mappings are page-aligned, non-executable, charged one unit per
  live map to the mapper's generation quota, and never overwrite an existing
  user page. `SYS_SHARED_BUFFER_SEAL` downgrades every live writable mapping
  before publishing an Arc-shared irreversible read-only state; release and
  subtree teardown remove PTEs before returning the buffer frames.
- Subdirectory-scoped capabilities may browse and derive further, but cannot
  commit the namespace root. Root transitions require an unscoped WRITE cap;
  scoped writes are rejected before object-store I/O.
- Spawn retains the executable and all grant sources. It returns one
  supervision handle and is bounded independently per spawner by the
  manifest's `spawnBudget`, plus the global live-task ceiling.
- Spawned code cannot be injected: `Executable` objects reference only
  generation-module bytes verified at boot. Spawn composes known components
  with gifted authority; it cannot introduce new code.
- Shared-buffer allocation is charged to the creating component's
  supervision-subtree account against its generation-declared per-holder quota
  (`shared-buffer-budget/v1`). A component absent from the budget holds the
  deny-by-default quota and cannot allocate. Release, peer death, supervised
  restart, and revocation reclaim every page and charge in that subtree without
  disturbing another subtree's account (C7.3/C7.5). Mapping quota is consumed
  and reclaimed by `SharedBufferTable::map` (C7.4). Loan quota is consumed by
  `SharedBufferTable::loan`; a released creator's pages and buffer charge remain
  retained until the final single-return loan settles (C7.5).
- The reference generation declares that budget in practice, not only in
  principle: `contracts/generation/v1/fixtures/valid.zti` carries a
  `shared-buffer-budget` `KIND_RESOURCE` object plus one `bufferCreate` grant
  each to `dango`, `spawn-service`, and `sample-lender`, and `bootstrap` mints a
  single transferable `SharedBufferFactory` that init derive-copies to exactly
  those holders. The grant authorizes the operation and the budget bounds it: a
  component with a grant but no budget entry still allocates nothing. Both
  holders run a bounded create/map/write/seal/release self-check at startup, so
  a healthy boot is itself evidence that the declared quotas are live (B4).
- The whole nine-syscall surface is driven by real components, not only by
  in-harness tables: `sample-lender` and `sample-receiver` exchange a payload
  larger than `MAX_MSG` under `just sample_plane_live_check`, with the loan
  receiver named through a `RIGHT_SUPERVISE` capability rather than an ambient
  task id. The gate asserts the denial arms in order — a factory capability is
  not a buffer, an unsealed region cannot be loaned, a sealed region never
  yields a writable mapping, a stale descriptor maps nothing, a loan cannot
  address past its subrange or gain write access, and a loan returns exactly
  once (B5).
- A C7.6 sample descriptor is a userspace control message (`sample-descriptor/v1`),
  not a kernel object: it references an exact transferred `SharedBufferLoan` by
  its unforgeable identity plus a page-aligned offset/length, type identity,
  sequence, and known flags. It carries no rights bit and mints no authority —
  the receiver still needs the loan capability to map. The descriptor fits one
  channel message (`DESCRIPTOR_LEN == MAX_MSG`), so a sample larger than the
  message bound crosses as descriptor plus shared buffer without copying payload
  bytes through the kernel queue or widening `MAX_MSG`.

## Bounds

| Resource | Bound | Enforcement |
| --- | --- | --- |
| Capabilities per task | `MAX_CAPS = 64` | `CapabilityTable::insert` |
| Capabilities per IPC message | `MAX_CAPS_PER_MSG = 4` | `SYS_SEND`/`SYS_RECV` argument checks |
| IPC queue depth | `CHANNEL_QUEUE = 16` | `ipc::send` |
| Live tasks | `MAX_TASKS = 32` | `SpawnError::TooManyTasks` |
| Live children per spawner | manifest `spawnBudget <= 32` | `SpawnError::BudgetExhausted` |
| Pinned DMA regions | `MAX_PINNED_REGIONS = 32` | DMA table |
| Live shared buffers | `MAX_SHARED_BUFFERS = 32` | `SharedBufferTable`; `SharedBufferError::ObjectsExhausted` |
| Shared-buffer total pages | `MAX_TOTAL_PAGES = 256` (1 MiB) | `SharedBufferTable`; `SharedBufferError::BytesExhausted` |
| Pages per shared buffer | `MAX_BUFFER_PAGES = 64` | `SharedBufferTable`; `SharedBufferError::BadSize` |
| Per-holder shared-buffer quota | generation `shared-buffer-budget/v1` resource (`byte_pages`, `buffer_count`, `mapping_count`, `loan_count`); deny by default | `SharedBufferTable::create` charges the creating subtree; `SharedBufferError::QuotaExceeded` |
| Live shared-buffer mappings | `MAX_MAPPINGS = 64` | `SharedBufferTable::map`; `SharedBufferError::MappingsExhausted` |
| Live shared-buffer loans | `MAX_LOANS = 64` plus generation `loan_count` per lender | `SharedBufferTable::loan`; `SharedBufferError::LoansExhausted` / `QuotaExceeded` |
| Declared holders per budget | `MAX_HOLDERS = 32` | `SharedBufferBudget::decode` |
| Directory path bytes | `MAX_DIRECTORY_PATH = 48` | `SYS_DIRECTORY_DERIVE`; filesystem schema |
| Directory path depth | `MAX_DIRECTORY_DEPTH = 4` | `DirectoryAuthority::derive`; filesystem schema |
| Directory entries per snapshot | `MAX_ENTRIES = 16` | filesystem protocol and snapshot decoder |
| Decoded keyboard events | `QUEUE_CAPACITY = 128` | oldest event dropped by the kernel input queue |

`MAX_TASKS` is coupled to the heap: each task eagerly allocates a 32 KiB
kernel stack, so the global ceiling reserves at most 1 MiB of the 24 MiB heap.
Per-spawner budgets prevent one client from consuming that global allowance.

## Horizon (claimed directions, not decisions)

| Candidate object | Candidate rights | Trigger | Open questions |
| --- | --- | --- | --- |
| BootState update authority beyond recovery | possibly STAGE_PENDING | M6 generation staging | Boundary between userspace staging and immutable stage-0 slot writes |
| NetworkDestination | CONNECT / SEND / RECV / LISTEN | [Hardware H6](../roadmap/04-platform-hardware.md) | Object shape: (protocol, address, port) declared in the generation? |
| EnergyAccount | READ? | [Hardware H track](../roadmap/04-platform-hardware.md) | Whether accounting is authority at all or read-only telemetry |

M6.1 landed userspace endpoint minting, non-consuming narrow derive-copy spawn
grants, per-spawner accounting, and supervision handles. Future resource
factories must follow the same named-capability and hard-bound rules.

## Debt register

- Terminated tasks are never reaped from the scheduler table; their address
  spaces and kernel stacks leak until reboot. Acceptable at current uptimes,
  but the live-task count already excludes them, so nothing else may rely on
  table length for accounting.
