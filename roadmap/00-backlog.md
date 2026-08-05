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

### B16 — a supervision termination record is never reclaimed, so a long-lived graph exhausts the table

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

**Deferral re-reviewed 2026-08-05, before opening P5.3.4's gate.** Still
deferred: that slice's graph creates five tasks against `MAX_RECORDS = 32`, so
the bound is not approached. See `devlog/2026-08-05-p5-3-4-sample-plane/`.

**Why deferred rather than fixed in P5.3.3:** the counting version touches every
path that installs or releases a capability, and the refusal version needs a
gate whose graph spawns past the record table to prove it. Neither is a line;
both want the multi-child graph P5.3.4 composes.

**Exit condition:** a graph that creates more than `MAX_RECORDS` tasks over its
lifetime still answers `supervision_status` correctly for every live handle,
observed under a named seL4 gate, with the five existing seL4 gates passing.

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four

**Problem:** `slime-root`'s spawn reads its grant array out of the caller's
transfer window through `transfer_window::read_staged`, which refuses anything
over `MAX_STAGED_BYTES` — `ipc::MAX_MESSAGE_BYTES`, 64 bytes. At 16 bytes per
record that is **four grants**. The retired kernel's `sys_spawn` reads the array
straight out of caller memory and is bounded only by
`kernel/src/capability/mod.rs::MAX_CAPS` (64).

Real x86 callers already exceed four: `init.rs::GENERATION_MANAGER_CAPS` and
`dango_caps()` are six grants each, and `spawn-service.rs` builds up to five.
On seL4 those would be refused `ERR_INVALID_ARG` where the oracle succeeds — a
component that runs on the retired kernel failing to launch its children on the
cutover, which is the one property P5.4 must be able to claim.

Latent today: every declared seL4 generation spawns with at most one grant, so
no gate observes it. `MAX_SPAWN_GRANTS` is now derived from the staging bound
rather than asserted to match the kernel's, so the ceiling is stated in the
source instead of discovered as a length error.

**Evidence:** `transfer_window::MAX_STAGED_BYTES` = `ipc::MAX_MESSAGE_BYTES` = 64
against `SPAWN_GRANT_RECORD_BYTES` = 16, and the six-element grant arrays in
`components/bins/src/bin/init.rs`. Noted in the P5.3.3 review; see
`devlog/2026-08-05-p5-3-3-spawn-plane/`.

**Proposed fix:** stage the grant array across more than one message-sized frame,
or give the spawn path its own staged-payload bound independent of the control
message's. The transfer window is already `MIN_TRANSFER_WINDOW` = 4096 bytes and
`sel4_transport::spawn` encodes into a `MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES`
buffer, so the room exists; what is missing is a root-side reader that will
accept more than one message's worth.

**Deferral re-reviewed 2026-08-05, before opening P5.3.4's gate.** Still
deferred, on an observation rather than by omission: that slice's largest grant
list is `sample-lender`'s three, which is 48 bytes against the 64-byte staging
bound, so the ceiling is not reached and the composition needs no widening. See
`devlog/2026-08-05-p5-3-4-sample-plane/`.

**Why deferred rather than fixed in P5.3.3:** it is a transport change rather
than a spawn change — `read_staged` and its bound are the channel plane's, and
widening them touches every operation that stages a payload, each with its own
gate. P5.3.4 is where a graph with realistic grant lists first runs.

**Exit condition:** a component spawns a child with at least six declared grants
on seL4 and the child holds all six at the slots its numbering fixes, observed
under a named seL4 gate, with the five existing seL4 gates passing.

### B12 — the component build's `--remap-path-prefix` names a path that does not exist

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

## Resolved

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
