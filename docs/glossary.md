# Glossary

Terms the rest of the documentation uses without redefining. Each entry gives
the working meaning and the page or file that owns the precise one. This page
is a lookup aid, not a source: when it disagrees with an owner, the owner wins.

**Acceptance.** One exam of a requirement in `specs/<area>.zt`: an `h.observed`
binds a `just` target and predicates over the counts its record reports; an
`h.human` binds a signed answer. A requirement is enforced when every
acceptance is. → [spec lifecycle](getting-started/06-spec-lifecycle.md).

**Admission.** The root's judgment of a decoded generation before any
component exists: every payload hash-verified and target-qualified, the whole
CSlot plan costed, every resource budget satisfiable. Failure is
`SLIME_ROOT FATAL` or `SLIME_GRAPH FAIL`, never a partial boot.
→ `slime-root/src/generation.rs`; [boot walkthrough](getting-started/03-boot-walkthrough.md).

**Arm.** One scenario of a checker that can boot several (`--arm`); several
`just` recipes may drive one checker with different arms.
→ [Add a gate](getting-started/07-add-a-gate.md).

**Area.** One `specs/<area>.zt` file: what one plane or subsystem promises
today — requirements, invariants, non-goals, decisions, open questions and
the paths it `owns`. → [spec lifecycle](getting-started/06-spec-lifecycle.md).

**Backlog.** `just spec_status --unenforced`: every requirement that is
failing, claimed or advisory, plus `roadmap/`. A failing requirement is
fixed, or marked `planned` with a note, by the next PR that touches its area.
→ `AGENTS.md`.

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

**Closure digest.** The key of an evidence record: a digest over a target's
recipe definitions, grader set, declared inputs, the derivation ids of the
images its checkers build, and the toolchain pins.
`python3 scripts/spec/closure <target> --explain` prints the composition.

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

**Epoch.** An IO0 ring generation counter: a driver reset settles every lease
and advances the epoch, and a client holding the old one is refused.
→ [I/O substrate](architecture/io-substrate.md).

**Evidence.** An `observation/v1` record on the `evidence` branch at
`<target>/<closureDigest>.zt`, written by CI after a push to `main`, or a
signed human or operator record. Keyed by the target's closure digest;
change any input in it and the record no longer applies.
→ [spec lifecycle](getting-started/06-spec-lifecycle.md).

**Evidence class.** Whether a claim rests on direct observation, inherited
evidence, or inference; recorded in PRs. Hardware claims bind
to target, revision, and binary identity.

**Exam** (also *grader*). What an acceptance is judged by: the `just` target
it names, run under `scripts/spec/record`. A change inside the exam's grader
set needs a human approval on the PR, so an implementation cannot quietly
change its own exam. → `CONTRIBUTING.md` (Grader independence).

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

**Grader set.** A bound target's recipe definitions, the scripts they invoke
and the `scripts/lib` modules those import, plus `scripts/spec/lib.py`,
`scripts/spec/record` and the observation contract. A diff touching it asks
for a human approval. Not the system-image **Closure**.
→ `.agents/skills/spec-lifecycle/SKILL.md`.

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

**Recipe.** A `just` target, defined under `just/` or in the root `Justfile`. A
recipe that verifies something is a **Gate**; every public recipe is listed in
[`just` targets](reference/just-targets.md).

**Release.** The detached, threshold-signed record binding a generation
identity, parent, sequence, and target; verified by selector images before
candidate bytes are read. → `contracts/release/v1/`.

**Legacy item.** A work item of the retired `.tasks/` store, indexed in
`specs/legacy.zt` by UUID with its frozen state and the requirement ids it
became; its full text is in Git at the commit the row names.

**Rights.** The bit set on a capability, one bit per root-checked operation;
vocabulary generated from `contracts/generation/v5/schema.zt`.

**Root** (`slime-root`). The initial task; owns all dynamic mechanism and is
part of the trusted computing base. → [trust model](architecture/trust-model.md).

**Shared buffer.** Root-minted, budget-bounded pages mappable by several
holders; the only runtime object mint. → `slime-root/src/shared_buffer.rs`.

**Slot pin.** A `slotPins` entry fixing a grant's slot for a holder, with a
reason (`componentAbi`, `bootLayout`, `encodedLayout`, `allocatorOrder`).

**Requirement.** One promise in an area file: a UUIDv7 id, a statement, a
rationale and its acceptances; `planned` while no exam binds it. The unit of
work. → [decision](decisions/living-spec-devloop.md).

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
