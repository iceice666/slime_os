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
8. Generation format v2 must map manifest grant rights strings 1:1 to bit
   names and `transferable` to `RIGHT_TRANSFER`; bootstrap wiring must be
   derived from manifest data, not hardcoded.

## Current matrix

Rights are a flat `u32`; bits 16–31 are free.

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
| SharedBuffer | BUFFER_WRITE (8) | future write-into-region operation | kernel `SharedRegion::new`; no userspace path | **ungated** |
| SharedBuffer | MAP (9) | future map-into-address-space operation | same | **ungated** |
| BlockDevice | BLOCK_READ (10) | read requests in `SYS_BLOCK_TRANSACT` for the capability's exact PCI function | kernel bootstrap | gated |
| BlockDevice | BLOCK_WRITE (11) | write and flush requests in `SYS_BLOCK_TRANSACT` for the capability's exact PCI function | kernel bootstrap | gated (M5.3) |
| ObjectStore | STORE_READ (12) | stat/get requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |
| ObjectStore | STORE_WRITE (13) | put requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |
| GenerationControl | HEALTH_CONFIRM (14) | `SYS_HEALTH_CONFIRM` for the currently running pending generation | kernel bootstrap, only for the declared generation-management service | gated (M5.6) |
| GenerationControl | BOOT_UPDATE (15) | `SYS_RECOVERY_RECONSTRUCT` after signed-index, generation, state-closure, and release scrub | kernel bootstrap, only for the declared recovery service | gated (M5.9) |
| Executable | SPAWN (16) | executable slot validation in `SYS_SPAWN` | generation manifest | gated (M6.1) |
| EndpointFactory | ENDPOINT_CREATE (17) | `SYS_ENDPOINT_CREATE` | generation manifest | gated (M6.1) |
| Supervision | SUPERVISE (18) | `SYS_SUPERVISION_STATUS` | returned by successful `SYS_SPAWN` | gated (M6.1) |
| Directory | DIRECTORY_READ (19) | `SYS_DIRECTORY_INSPECT` before filesystem reads | kernel bootstrap from the generation's declared root | gated (M6.3) |
| Directory | DIRECTORY_WRITE (20) | `SYS_DIRECTORY_INSPECT` before mutation and `SYS_DIRECTORY_COMMIT` for atomic root swap | same | gated (M6.3) |
| Directory | DIRECTORY_LIST (21) | `SYS_DIRECTORY_INSPECT` before bounded enumeration | same | gated (M6.3) |
| Directory | DIRECTORY_DERIVE (22) | `SYS_DIRECTORY_DERIVE` for a subdirectory-scoped, narrow-rights copy | same; powerbox minting needs only this operation | gated (M6.3) |

Semantics not visible in the table:

- Receiving a capability over IPC costs the receiver no rights bit; the cap
  arrives with exactly the rights the sender attached.
- `derive` and spawn grants are non-consuming and narrow-only. A derived copy
  that retains `TRANSFER` requires that meta-right on its source.
- Subdirectory-scoped capabilities may browse and derive further, but cannot
  commit the namespace root. Root transitions require an unscoped WRITE cap;
  scoped writes are rejected before object-store I/O.
- Spawn retains the executable and all grant sources. It returns one
  supervision handle and is bounded independently per spawner by the
  manifest's `spawnBudget`, plus the global live-task ceiling.
- Spawned code cannot be injected: `Executable` objects reference only
  generation-module bytes verified at boot. Spawn composes known components
  with gifted authority; it cannot introduce new code.

## Bounds

| Resource | Bound | Enforcement |
| --- | --- | --- |
| Capabilities per task | `MAX_CAPS = 64` | `CapabilityTable::insert` |
| Capabilities per IPC message | `MAX_CAPS_PER_MSG = 4` | `SYS_SEND`/`SYS_RECV` argument checks |
| IPC queue depth | `CHANNEL_QUEUE = 16` | `ipc::send` |
| Live tasks | `MAX_TASKS = 32` | `SpawnError::TooManyTasks` |
| Live children per spawner | manifest `spawnBudget <= 32` | `SpawnError::BudgetExhausted` |
| Pinned DMA regions | `MAX_PINNED_REGIONS = 32` | DMA table |
| Directory path bytes | `MAX_DIRECTORY_PATH = 48` | `SYS_DIRECTORY_DERIVE`; filesystem schema |
| Directory path depth | `MAX_DIRECTORY_DEPTH = 4` | `DirectoryAuthority::derive`; filesystem schema |
| Directory entries per snapshot | `MAX_ENTRIES = 16` | filesystem protocol and snapshot decoder |

`MAX_TASKS` is coupled to the heap: each task eagerly allocates a 32 KiB
kernel stack, so the global ceiling reserves at most 1 MiB of the 24 MiB heap.
Per-spawner budgets prevent one client from consuming that global allowance.

## Horizon (claimed directions, not decisions)

| Candidate object | Candidate rights | Trigger | Open questions |
| --- | --- | --- | --- |
| BootState update authority beyond recovery | possibly STAGE_PENDING | M6 generation staging | Boundary between userspace staging and immutable stage-0 slot writes |
| NetworkDestination | CONNECT / SEND / RECV / LISTEN | M7 | Object shape: (protocol, address, port) declared in the generation? |
| EnergyAccount | READ? | M7 | Whether accounting is authority at all or read-only telemetry |
| SharedBuffer creation | CREATE / quota | userspace driver path | Block payloads need userspace-created buffers; who may create, how much |

M6.1 landed userspace endpoint minting, non-consuming narrow derive-copy spawn
grants, per-spawner accounting, and supervision handles. Future resource
factories must follow the same named-capability and hard-bound rules.

## Debt register

- `RIGHT_MAP` (bit 9) predates the naming rule; rename to
  `RIGHT_BUFFER_MAP` when convenient (touches `capability/mod.rs`, the
  storage-authority allowlist, and tests).
- Terminated tasks are never reaped from the scheduler table; their address
  spaces and kernel stacks leak until reboot. Acceptable at current uptimes,
  but the live-task count already excludes them, so nothing else may rely on
  table length for accounting.
