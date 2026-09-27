# Glossary

Terms the rest of the documentation uses without redefining. Each entry gives
the working meaning and the page or file that owns the precise one. This page
is a lookup aid, not a source: when it disagrees with an owner, the owner wins.

**Admission.** The root's judgment of a decoded generation before any
component exists: every payload hash-verified and target-qualified, the whole
CSlot plan costed, every resource budget satisfiable. Failure is
`SLIME_ROOT FATAL` or `SLIME_GRAPH FAIL`, never a partial boot.
→ `slime-root/src/generation.rs`; [boot walkthrough](getting-started/03-boot-walkthrough.md).

**Arm.** One scenario of a checker that can boot several (`--arm`); several
`just` recipes may drive one checker with different arms.
→ [Add a gate](getting-started/07-add-a-gate.md).

**Backlog.** Work items tagged `backlog`: known defects. Open ones are resolved,
deferred, or blocked before new milestone work; `just tasks_check` enforces
it. → `AGENTS.md`.

**Badge.** The word seL4 attaches to a capability so the receiver knows which
capability a message arrived on. The root mints a badge per task on its
service endpoint and authenticates every request by it; a component cannot
forge another's. → [trust model](architecture/trust-model.md).

**Binding.** The association of a grant with a slot in a holder's capability
table. Deterministic by grant name unless pinned. Components resolve bindings
by name with `resolve_binding`. → [component runtime](getting-started/10-component-runtime.md).

**Bless.** Deliberately accept a changed frozen fixture (`just
sel4_boot_layout_bless`) after reading the diff. Blessing to silence a gate is
the thing the fixture exists to catch.

**Boot action.** The authenticated integer the root passes the bootstrap
instance as its startup argument, naming which composition behavior to run.
Zero for every other component; readable via `boot_action()`.

**Boot layout.** Init's resolved capability layout on a plane, frozen as a
fixture and compared by `just sel4_boot_layout_check`. → `contracts/boot-layout/`.

**BootState.** The two redundant 512-byte on-disk slots recording known-good
and pending generation identities, attempt budget, and accepted release
sequence. Committed older-slot-first. → [generation management](architecture/generation-management.md).

**Boot selector.** The pre-admission path compiled only into
`slime_boot_selector` images that reads BootState and a boot-store directory
from disk to choose which generation to admit. → `slime-root/src/boot_selector.rs`.

**Bootstrap instance.** The one instance the root activates itself (`init` in
every current composition). Everything else is spawned by a userspace owner.

**Capability.** An unforgeable reference to an object plus a rights mask.
Narrowing-only, explicitly transferred, never ambient. → [concepts/capabilities](concepts/capabilities.md), [capability matrix](capability-matrix.md).

**Chain.** An ordered list of serial markers a gate requires; markers within a
chain must appear in order, chains are independent. → `scripts/lib/sel4_gate_markers.py`.

**Closure** (system-image closure). The generated record naming, by digest,
every input an image is built from. Re-resolved before every build; a stale
digest is `closure does not resolve`. Regenerate with
`generate-system-image-closures.py`. → [component/system/image](architecture/component-system-image.md).

**Component.** The unit of isolation and fault containment: one crate, one
ELF, one task with its own CSpace and VSpace, holding only granted
capabilities. Not a process. → [concepts/components](concepts/components.md).

**Component spec.** The composition-independent declaration of a component:
identity, requires/provides, resources, lifecycle, health, evidence link.
→ `contracts/component-spec/v1/components/<name>.zti`.

**Composition.** Which components run together, with what grants, slots,
budgets, and health rules — authored as a system spec, rendered as a
generation manifest. → `contracts/system-spec/`.

**Contract.** A versioned Zutai schema under `contracts/<name>/vN/` that is the
single source of truth for a format crossing a process, persistence, or boot
boundary. Bindings are generated, never hand-written. → [concepts/contracts](concepts/contracts.md), [write a contract](getting-started/09-write-a-contract.md).

**CSpace / CSlot.** seL4's capability table for a task and one entry in it.
The root plans every task's CSpace before activation; grant slots are the
component's own numbering from 0.

**Devloop.** The pinned tool that admits, validates, gates, and completes
spec-driven work items. → [work-item lifecycle](getting-started/06-work-item-lifecycle.md).

**Epoch.** An IO0 ring generation counter: a driver reset settles every lease
and advances the epoch, and a client holding the old one is refused.
→ [I/O substrate](architecture/io-substrate.md).

**Evidence class.** Whether a claim rests on direct observation, inherited
evidence, or inference; recorded in PRs and work items. Hardware claims bind
to target, revision, and binary identity.

**Fabric.** The typed data plane: graph-declared routes between participants,
stream/call/operation semantics, QoS, recording. → [typed data fabric](architecture/typed-data-fabric.md).

**Fail closed.** Missing evidence is a failing check, not a skip. Every gate
here is built this way, and `just sel4_gate_control_check` proves it.

**Fault.** A hardware exception (VM fault, cap fault, unknown syscall) in a
component thread, supervised by the root and reported to the parent as a
typed termination without an address. → `slime-root/src/fault.rs`.

**Gate.** A `just` recipe that boots an image or runs a checker and fails
closed on missing, reordered, or explicit-failure evidence. Narrow by design;
several may drive one checker. → [add a gate](getting-started/07-add-a-gate.md).

**Generation.** The whole bootable graph as one deterministic, hash-identified,
versioned artifact: executables, instances, grants, budgets, state bindings.
The unit of deployment and rollback. → [concepts/generations](concepts/generations.md).

**Generation manifest.** The derived `.zti` rendering of a system spec that the
generation builder consumes. An output, never edited. → `contracts/generation-manifest/v1/`.

**Grant.** A system-spec declaration that a holder receives a capability of a
kind, from a source, with exact rights and a transferability flag.
→ [add a component](getting-started/05-add-a-component.md) §4.

**Health census.** The final `SLIME_GRAPH HEALTHY generation= required= live=
completed= failed=` line; gates pin every number.

**Holder.** The instance that receives a grant.

**Instance.** One placement of an executable in a composition, with owner,
autostart, dependencies, and health class.

**Interposition.** A fabric hop inserted between participants for policy or
recording; `just sel4_fault_check` boots a plane whose hop is compiled to die.

**IO0 / IO1 / IO2 …** The numbered I/O substrate layers: IO0 is the shared
ring (queue/epoch/lease), higher numbers add device, DMA, link, and block
authority. → [I/O substrate](architecture/io-substrate.md).

**Lease.** A bounded loan of a buffer slice to a driver for one request,
settled on completion or reset.

**Loan.** A shared-buffer sub-range lent to another holder with narrowed
rights, revocable by the lender. → `slime-root/src/shared_buffer.rs`.

**Marker.** A serial line a gate asserts on: `SLIME_ROOT …`, `SLIME_GRAPH …`,
`SLIME_BOOT …`, `SLIME_MEM …`, `[component] …`. Contract surface; change the
emitter and the gate together. → [boot walkthrough](getting-started/03-boot-walkthrough.md).

**Mechanism vs policy.** Mechanism (object allocation, IPC, supervision, admission)
lives in `slime-root`; policy (who spawns what, when, with which grants) lives
in userspace components and the system spec.

**MyQue.** The tool managing `.tasks/`, the canonical work-item store; allocates
UUIDs and writes relationships. → `AGENTS.md`.

**Native endpoint.** An seL4 endpoint object the root creates from an
`endpoint` grant and installs in both holders' CSpaces; traffic on it is
direct and unmediated. → [IPC and capabilities](architecture/ipc-and-capabilities.md).

**Notification.** An seL4 notification object declared in the system spec
with `signal` and `wait` bindings; the cheap wake primitive, and the base of a
wait set.

**Pin.** A recorded identity the build refuses to deviate from: submodule
commits, toolchains, kernel hashes (`sel4/pins.toml`); or a fixed slot in a
system spec (`slotPins`); or a marker count in `check-sel4-gate-controls.py`.

**Plane.** One QEMU-bootable composition plus the checker that asserts its
markers, named `sel4-<domain>`. → [add a gate](getting-started/07-add-a-gate.md).

**Private memory.** A component's fixed-base, growable, non-shareable,
non-executable region, bounded by a generation quota. → [private memory](architecture/private-memory.md).

**Release.** The detached, threshold-signed record binding a generation
identity, parent, sequence, and target; verified by selector images before
candidate bytes are read. → `contracts/release/v1/`.

**Retired item.** A done or cancelled work item reduced to a minimal terminal
record after its final bytes are retained in Git.

**Rights.** The bit set on a capability, one bit per root-checked operation;
vocabulary generated from `contracts/generation/v5/schema.zt`.

**Root** (`slime-root`). The initial task; owns all dynamic mechanism and is
part of the trusted computing base. → [trust model](architecture/trust-model.md).

**Shared buffer.** Root-minted, budget-bounded pages mappable by several
holders; the only runtime object mint. → `slime-root/src/shared_buffer.rs`.

**Slot pin.** A `slotPins` entry fixing a grant's slot for a holder, with a
reason (`componentAbi`, `bootLayout`, `encodedLayout`, `allocatorOrder`).

**Spec-driven item.** A work item whose body is a fenced `dev-spec/v1` Zutai
payload with bound acceptance gates; mandatory for identities after
2026-10-01. → [decision](decisions/mandatory-spec-driven-work-items.md).

**Supervision handle.** The opaque capability a spawner receives for a child,
through which it observes termination as a typed status. Collecting a terminal
status consumes it.

**System spec.** The authored composition source (`systems/<name>.zti`, or
`sources/<name>.zt` for computed ones). The only place to change grants,
slots, or budgets. → `contracts/system-spec/README.md`.

**Target profile.** The exact architecture/platform/ABI tuple an image is
built and qualified for (`aarch64-sel4-qemu-virt`,
`x86_64-sel4-framework13-ai300`, …); admission refuses a payload for any
other. → [targets and portability](architecture/targets-and-portability.md).

**Terminal condition.** The regex that ends a plane's QEMU run: the last
marker of the last chain or any failure marker.

**Transfer window.** The per-thread page the runtime binds at startup for
payloads too large to ride inline in an IPC message.

**Wait set.** A userspace multiplexer over one notification: registered
badges, endpoint slots, and a timer, dispatched in bounded order.
→ [runtime authority](architecture/runtime-authority.md).

**Zutai.** The schema and generation language every contract is written in
(`.zt` sources, `.zti` instances), pinned as a submodule under `deps/zutai`.
