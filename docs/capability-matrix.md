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

Rights are a flat `u32`; bits 14–31 are free.

| Object | Right (bit) | Gated operation | Creation authority | Gate status |
| --- | --- | --- | --- | --- |
| Endpoint | SEND (0) | `SYS_SEND` via this endpoint | kernel `ipc::channel()`; no userspace path | gated |
| Endpoint | RECV (1) | `SYS_RECV` on this endpoint | same | gated |
| *(meta, any cap)* | TRANSFER (2) | cap attachment in `SYS_SEND`; grants in `SYS_SPAWN` | — | gated on both paths |
| Executable | EXEC (3) | executable slot of `SYS_SPAWN` | generation module only, hash-verified at boot | gated |
| PciFunction | MAP_MMIO (4) | future map-BAR operation (userspace driver path) | kernel PCI enumerator | **ungated** |
| PciFunction / DmaMemory | DMA_PIN (5) | future pin operation | kernel DMA allocator on a driver's behalf | **ungated** |
| DmaMemory | DMA_RELEASE (6) | future release/reclaim operation | same | **ungated** |
| Irq | IRQ_ACK (7) | future ack operation | kernel interrupt subsystem | **ungated** |
| SharedBuffer | BUFFER_WRITE (8) | future write-into-region operation | kernel `SharedRegion::new`; no userspace path | **ungated** |
| SharedBuffer | MAP (9) | future map-into-address-space operation | same | **ungated** |
| BlockDevice | BLOCK_READ (10) | read requests in `SYS_BLOCK_TRANSACT` | kernel | gated |
| BlockDevice | BLOCK_WRITE (11) | write and flush requests in `SYS_BLOCK_TRANSACT` | kernel | gated (M5.3) |
| ObjectStore | STORE_READ (12) | stat/get requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |
| ObjectStore | STORE_WRITE (13) | put requests in `SYS_STORE_TRANSACT` | kernel bootstrap | gated (M5.4) |

Semantics not visible in the table:

- Receiving a capability over IPC costs the receiver no rights bit; the cap
  arrives with exactly the rights the sender attached.
- `derive` is ungated but narrow-only, so it cannot amplify authority.
- Spawn consumes granted capabilities (move semantics) but retains the
  Executable capability, so one EXEC grant can instantiate a component
  repeatedly, subject to `MAX_TASKS`.
- Spawned code cannot be injected: `Executable` objects reference only
  generation-module bytes verified at boot. Spawn composes known components
  with gifted authority; it cannot introduce new code.

## Bounds

| Resource | Bound | Enforcement |
| --- | --- | --- |
| Capabilities per task | `MAX_CAPS = 64` | `CapabilityTable::insert` |
| Capabilities per IPC message | `MAX_CAPS_PER_MSG = 4` | `SYS_SEND`/`SYS_RECV` argument checks |
| IPC queue depth | `CHANNEL_QUEUE = 16` | `ipc::send` |
| Live tasks | `MAX_TASKS = 16` | `SpawnError::TooManyTasks` |
| Pinned DMA regions | `MAX_PINNED_REGIONS = 32` | DMA table |

`MAX_TASKS` is coupled to the heap: each task eagerly allocates a 64 KiB
kernel stack, so `MAX_TASKS * 64 KiB` must stay well within the 2 MiB heap.
Raising the task bound means growing the heap or shrinking per-task stacks
in the same change.

## Horizon (claimed directions, not decisions)

| Candidate object | Candidate rights | Trigger | Open questions |
| --- | --- | --- | --- |
| BootState / GenerationControl | HEALTH_CONFIRM; possibly BOOT_UPDATE | M5.6 | Confirm vs update split; boundary between userspace slot writes and the stage-0 selector |
| Directory | READ / WRITE / LIST? | M6 | Granularity; whether powerbox minting needs more than `derive` |
| Endpoint minting | *(no new object)* | M6 prerequisite | Unprivileged mint with quota vs a factory capability |
| RIGHT_SPAWN on Executable | SPAWN | generation format v2 | Deferred until grants are data-driven |
| NetworkDestination | CONNECT / SEND / RECV / LISTEN | M7 | Object shape: (protocol, address, port) declared in the generation? |
| EnergyAccount | READ? | M7 | Whether accounting is authority at all or read-only telemetry |
| SharedBuffer creation | CREATE / quota | userspace driver path | Block payloads need userspace-created buffers; who may create, how much |

M6's spawn service additionally needs kernel mechanisms that do not exist
yet: userspace endpoint minting, a non-consuming (derive-copy) grant path,
per-spawner resource accounting, and supervision handles. Record them here
so the milestone does not discover them mid-flight.

## Debt register

- `component_name_from_id` in `kernel/src/syscall/mod.rs` hardcodes component
  names; spawn identity must come from the generation manifest in format v2.
- `RIGHT_MAP` (bit 9) predates the naming rule; rename to
  `RIGHT_BUFFER_MAP` when convenient (touches `capability/mod.rs`, the
  storage-authority allowlist, and tests).
- Terminated tasks are never reaped from the scheduler table; their address
  spaces and kernel stacks leak until reboot. Acceptable at current uptimes,
  but the live-task count already excludes them, so nothing else may rely on
  table length for accounting.
