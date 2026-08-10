# B39 — generation v5 header cutover: authenticated boot action, host checkers, and fabric provenance

| Field | Value |
|---|---|
| Date | 2026-08-10 |
| Kind | Defect |
| Status | Root-caused |
| Scope | `boot-contracts/src/generation.rs`, `stage0/src/lib.rs`, `slime-root/src/generation.rs`, `scripts/check/check-generation.py`, `scripts/lib/release_trust.py`, `just contracts_check`, `just generation_check`, `just sel4_boot_check` |
| Roadmap | B39 |
| Gates | `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host` |
| Trigger | The in-flight generation v4→v5 cutover for B39 left the host-side checkers, stage-0 consumer, and fabric provenance check on the retired v4 header layout and instance model. |
| Baseline | Before the v5 header grew its process/thread/kernel-object/mapping/binding/schedule/quota plan sections, `just generation_check` passed and every consumer read a 31-field header whose string table began at byte 208. |

## Summary

The v5 wire format landed in `boot-contracts` with ten new plan record types and a
22-field offset table, but four consumers were never migrated with it: the Python
structural checker still destructured the v4 31-field header, `release_trust.py`
still read the string-table offset from the v4 byte position, `stage0` still called
the deleted `component()` API, and `slime-root`'s fabric provenance check resolved
participant names against declared *instances* rather than the *executable*
catalogue. The first three made `just generation_check` fail outright; the fourth
made every fabric-bearing generation unbootable
(`SLIME_ROOT FATAL generation admission rejected: UndeclaredFabricParticipant`).
All four are fixed and their gates pass. B39 remains open: `just sel4_boot_check`
now fails later, at `SLIME_GRAPH spawn refused task=0 slot=6 ungranted`, because
`preflight_spawn_grants` requires each dynamically spawned child to match a
declared owned instance while the seL4 fixtures still declare only `init`.

## Observable symptom

- Command: `just generation_check`
- Expected: byte-identical double build, then generation and boot-store admission.
- Observed: `ValueError: too many values to unpack (expected 31, got 51)` in
  `check_generation`, then after that fix `CheckError: WrongReleaseTarget`.
- Exit/fault/serial evidence: `just sel4_boot_check` reached
  `SLIME_ROOT FATAL generation admission rejected: UndeclaredFabricParticipant`
  immediately after `SLIME_ROOT virtio probed`.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `GENERATION_HEADER` is now `8s I I Q 32s Q 32s` + 22×`I` + 22×`Q` + 152 pad = 51 fields; `check_generation` destructured 31. | The Python twin of the decoder had not been migrated at all. |
| 2 | Header byte 100 was a reserved `u32` in v4 and is the boot-action string offset in v5. | The authenticated boot-composition selector needed a decoder-side type, not a raw offset. |
| 3 | `release_trust.generation_release_fields` read `string_offset` from byte 208 (`208 if version >= 4 else 184`); in v5 it lives at byte 328. | Release target text decoded from `dependency_offset`, so every release mismatched its generation. |
| 4 | `stage0/src/lib.rs` called `generation.component(index)` and `generation.kernel_object`, both removed by the v5 rewrite. | `just lint_all` failed on both UEFI targets. |
| 5 | `fabric_graph_participants_are_declared` built its name set from `generation.instance(slot)`, but every seL4 fixture declares exactly one instance (`init`) and carries participants as executables spawned dynamically. | Fabric admission rejected the graph before any component launched. |
| 6 | With admission fixed, the boot plane advanced to `[init] fabric boot control channels minted` and then `spawn refused … ungranted`; `preflight_spawn_grants` requires an instance owned by the caller whose executable matches, plus matching bindings on both parent and child. | The remaining B39 work is the fixture instance-model migration, not a decoder defect. |

## Root cause

The v5 header inserted ten count fields and ten offset fields between
`health_offset` and `string_offset`, shifting every offset past byte 184 and
growing the header tuple from 31 to 51 fields. Consumers that addressed the
header positionally (`fields[17]`, `fields[8]`) or by hardcoded byte offset
(`208`) silently read neighbouring fields instead of failing. Separately, the
v5 rewrite introduced `fabric_graph_participants_are_declared` against the
instance catalogue, but a fabric participant only exists as an *executable*
until init spawns it, so the check could never be satisfied by any existing
fixture.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `boot-contracts/src/generation.rs` | Added `BootAction` (25 variants, `#[repr(u32)]`, parsed from the header's boot-action string at byte 100) and decoded it into `Generation::boot_action`; an unknown spelling is `DecodeError::UnknownEnum`. | The boot composition is authenticated generation data with a stable numeric ABI, independent of the source spelling. |
| `scripts/check/check-generation.py` | Rewrote `check_generation` for the 51-field v5 header: all twenty section-bound assertions, trailing header padding, boot-action admission, and structural validation of process, thread, kernel-object, mapping, cap-binding, service-binding, schedule, fault-policy, spawn-template, and resource-quota records, including "every grant materializes exactly once, or is explicitly policy-only". | The host checker is again an independent twin of the Rust decoder. |
| `scripts/check/check-generation.py` | `check_release` no longer reads `object_offset`/kernel index positionally, and the unreachable v2/v3 kernel-bundle branch is deleted. | No consumer addresses the header by tuple position. |
| `scripts/lib/release_trust.py` | `generation_release_fields` reads `GENERATION_HEADER_STRING_OFFSET_OFFSET` instead of the hardcoded v4 byte 208. | Release target, identity, and authority manifest are derived from the actual v5 layout. |
| `stage0/src/lib.rs` | `admit_generation_closure` locates the kernel by scanning the object closure for `KIND_KERNEL` and walks `executable_count()`/`executable()` instead of the removed `kernel_object` field and `component()` API. | Stage-0 admits a v5 closure on both UEFI targets. |
| `slime-root/src/generation.rs` | `fabric_graph_participants_are_declared` resolves participant identities against the executable catalogue (`MAX_ADMITTED_EXECUTABLES`, `TooManyExecutables`). | A graph naming a component the generation dropped is still refused, while a participant the generation carries as a spawnable executable is admitted. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A header field is added and a positional consumer silently misreads its neighbour | `just generation_check` | Determinism check raises `CheckError` or a bound assertion fires |
| A v5 grant is declared with no materializing capability | `just generation_check` (`UnmaterializedGrant` in `check_generation`) | Admission of an unbacked grant |
| Fabric provenance regresses to matching the wrong catalogue | `just test_sel4_root` (`a_graph_may_not_name_a_component_the_generation_lacks`) | An undeclared participant admits, or a declared one is refused |
| Stage-0 drifts from the generation API again | `just lint_all` | `E0599` on the UEFI targets |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | Pass — 178 contract tests, all bindings current, boot-layout resource current | Direct |
| `just generation_check` | Pass — two isolated builds byte-identical; generation and boot-store admission passed | Direct |
| `just test_host` | Pass — 203 boot-contracts tests plus the slime-proto suites | Direct |
| `just test_sel4_root` | Pass — 130/130 | Direct |
| `just lint_all` | Pass — stage0 (both UEFI targets), boot-contracts, slime-root, components | Direct |
| `just fmt_check_all`, `just ruff`, `just typos` | Pass | Direct |
| `just sel4_boot_check` | **Fail** — advances past fabric admission to `SLIME_GRAPH spawn refused task=0 slot=6 ungranted` | Direct |

## Decisions

- Decision: `BootAction` is a decoder-side enum with an explicit numeric ABI rather than a string compared at each use site.
- Rationale: the boot composition must reach the bootstrap thread as a word, and component images must stay byte-identical across manifests that differ only in composition.
- Rejected alternative: keeping the raw string offset in the header and letting each consumer compare text, which reintroduces the build-flag coupling B39 exists to remove.

- Decision: fabric participants are resolved against the executable catalogue.
- Rationale: a participant is spawned dynamically by init and has no instance record until it exists; the provenance property under test is "the graph names something this generation carries", and the executable catalogue is what carries it.
- Rejected alternative: requiring an instance per participant, which is the same fixture migration `preflight_spawn_grants` already demands and would have conflated two separate invariants in one check.

## Open risks and follow-ups

- [ ] B39 is not closed. `just sel4_boot_check` fails at `preflight_spawn_grants`: it requires every dynamically spawned child to have a declared instance owned by the caller, with the transferred grant bound on both parent and child, while `sel4-boot.zti` and the other seL4 fixtures declare only `init`. Closing B39 requires migrating each fixture's instance model (`contracts/generation/v1/fixtures/sel4-*.zti`), which is a content migration rather than a decoder change.
- [ ] `init.rs` still selects its composition through `option_env!("SLIME_GENERATION_NUMBER")` and the `SLIME_*_CHECK` flags. `Generation::boot_action` is decoded but not yet passed to the bootstrap thread or consumed by `init`, so B39's "two builds of one component image cannot select different boot graphs" clause is unproven.
- [ ] `scripts/build/build-transfer.py` references `CHECK.GENERATION_COMPONENT`, which no longer exists. It is unreachable from every Justfile target and was already stale before this work; it belongs to B50's residue deletion.
- [ ] The Python checker deliberately does not reimplement `grant_applies_to_instance`'s ownership semantics; that arm is covered only by the QEMU planes. **[INFERENCE]** that this is adequate, on the grounds that the property is admission policy rather than wire-format integrity.

## Artifacts and provenance

- Focused report: none; the investigation log above is the record.
- Raw transcript: none retained.
- Serial/debugger/model output: quoted inline under *Observable symptom* from `just sel4_boot_check`.
- Related roadmap item: [`roadmap/00-backlog.md` B39](../../roadmap/00-backlog.md)

## Corrections

**2026-08-10 — the fabric-provenance change in *Changes* was wrong and has been reverted.**

`fabric_graph_participants_are_declared` was changed to resolve participants
against the executable catalogue on the reasoning that participants are
executable-only. Enumerating the fixtures disproved that: `sel4-stream`,
`sel4-qos`, `sel4-call`, `sel4-operation`, and `sel4-visibility` all declare
every fabric participant as an instance, and only `sel4-boot.zti` does not.
The instance-based check was correct; the function is restored to it.

The real defect was one layer down and is fixed in
`fix(sel4/generation): plan every declared instance`. The v5 plan builder
emitted process, thread, schedule, fault-policy, and quota records only for
**root-owned** instances, so an owner-spawned child had no process at all and
every grant sourced from one materialized no capability — refused as
`BadBinding` before boot. The plan now covers every declared instance. This is
what B39's "prove the object plan before activation" clause actually requires:
an owner-spawned process consumes kernel objects exactly as a root-autostart
one does.

A second rule was also wrong. `preflight_spawn_grants` required the *spawner*
to hold every grant it hands a child (`reason=owner-missing`). That
contradicts the fabric planes' stated invariant that init keeps no route
authority over channels it mints and passes on. Provenance now comes from the
child's own declared binding via `grant_applies_to_instance`; the declared
rights ceiling and the parent's held capability at the requested slot still
bound what may be transferred, so authority cannot be amplified.

**2026-08-10 — `just sel4_spawn_check` is red, and was already red before this
work.** Verified by checking out `3228eb6` (the pre-session commit) and by
re-running the gate at each intermediate commit: the failure and its serial
signature are identical throughout. Its fixture declares `console` and
`sysinfo` with empty binding lists while `init.rs` spawns them with one and
six runtime-minted grants respectively, and `preflight_spawn_grants` requires
`count == child.binding_count()`. Declaring those grants in the fixture is
rejected by the builder's own closure rule
(`bindings do not close over related grants`), so closing it needs a decision
about how runtime-minted channel capabilities are declared, not just fixture
content. Recorded as B39 remaining work rather than fixed here.

**2026-08-10 — probing the spawn plane's declaration model, and why it was not
committed.** Two candidate shapes for declaring runtime-minted channel
capabilities were built and booted, then reverted:

| Shape | Builder | Boot |
|---|---|---|
| Grant `source = child; target = init` | Rejected: `bindings do not close over related grants` — `expected_bindings` adds the grant to *both* endpoints, so `init` must bind a channel it is meant not to retain | not reached |
| Grant `source = target = child` (self-targeted) | Accepted | `console` and `sysinfo` both spawn and receive their declared slots; `console` then exits 1 |

Widening `console-control` from `recv` to `send`+`recv` removed the exit-1 but
the plane then reached `SLIME_GRAPH HEALTHY generation=1 required=3 live=3` and
parked without producing `[init] spawn plane complete`, timing out at 120s.
That is a *different* scenario outcome, not a fix: the gate's subject is the
two children `init` spawns, and this graph left three live tasks parked rather
than running the scenario to its terminal marker.

Both were reverted. The self-targeted shape is mechanically viable but makes a
grant record mean "this child receives some capability of these rights at this
slot" instead of naming a concrete authority edge, which weakens the exact
property B39 exists to establish. The choice binds B40–B50, so it is recorded
as an open decision rather than settled by whichever spelling boots.

**2026-08-10 — the declaration model was settled: a new `MintedBinding`
record.** The open decision above was resolved in favour of a distinct v5
record rather than either probed spelling, so `CapabilityGrant` keeps meaning
a concrete authority edge between two declared endpoints.

`MintedBinding` names the owner, the holder, the destination slot, an exact
rights ceiling, and a `transferable` flag. Everything about the edge is fixed
before activation; only the object identity is deferred to the owner that mints
it. The decoder rejects a binding whose holder its owner does not own, whose
rights are empty, carry `exec`, or fall outside the vocabulary, and any two
claiming the same holder slot. Spawn preflight resolves each request against
exactly one declaration — grant-backed bindings first, then minted ones in
ascending destination-slot order — and the destination is always the declared
slot, never a number the caller chose.

Two further defects surfaced while wiring it, both from the plan-coverage
commit and both fixed here:

| Defect | Symptom | Fix |
|---|---|---|
| A grant was materialized only in its `source` instance | `authority-bearing grant console-shared-buffer-factory has no concrete binding`, failing every loan-plane build | Materialize in whichever instance declares a binding; a delegated authority such as `bufferCreate` is bound only by its target |
| `transfer` was declared as a named right | `unknown right transfer` | `MintedBinding` carries `transferable`, matching `CapabilityGrant`; transfer is a property of the edge, not a right |

The stream fixture is migrated as the template: the fabric's endpoint and
shared-buffer factories become real grants against concrete objects, and the
probe channels, per-publisher buffer factory, and supervision handles become
minted bindings. Under it the stream plane advances from refusing the very
first spawn to provisioning the entire graph — `[fabric] every declared stream
edge provisioned`, with all four authority denials observed — before failing
later in a shared-buffer remap (`ARMPageMap: Attempting to remap a frame that
does not belong to the passed address space`). That residual fault is past
every capability-declaration boundary this entry concerns.

**2026-08-10 — `bootAction` is now delivered and consumed; every
`SLIME_SEL4_*_CHECK` branch is gone from `init.rs`.** The decoded action
reaches the bootstrap thread in its first C parameter (`c_param_mut(0)`,
available on every architecture), and `sel4-runtime-common`'s
`declare_rust_entrypoint!` already forwards typed C parameters, so no assembly
or link-time change was required. `slime_rt::entry!` forwards it to `main`,
whose signature gains the argument across all 56 component binaries; only
`init` reads it. Non-bootstrap instances and dynamically spawned children
receive zero.

`compose_declared_graph` replaces twenty-two `option_env!` branches with a
match on the authenticated value. An action the image does not implement exits
non-zero rather than falling through to another graph, which is the property
B39's "two builds of one component image cannot select different boot graphs"
clause asks for: the image is byte-identical across every manifest and only the
admitted action differs.

Three x86 oracle guards (`SLIME_FABRIC_QOS_CHECK`,
`SLIME_FABRIC_OPERATION_CHECK`, `SLIME_FABRIC_VISIBILITY_CHECK`) carried a
second half whose only job was to exclude the seL4 plane sharing the flag;
those halves are deleted, and each flag now names exactly one composition.
`qos_plane()` became a `qos` parameter threaded from the action.
`launch_fabric_boot` and its four helpers — the x86-only full-graph
composition keyed on `SLIME_GENERATION_NUMBER == 17` — are deleted as dead.

`check-sel4-component-graph.py` no longer asserts init's exact transfer-window
base. That address sits above init's own image and moves whenever `init.rs`
changes size; the property under test is that init bound a one-page window at
all. The two child window addresses stay exact, since those images are not
edited by work on init's composition.

**Four planes fail identically before and after this change** —
`sel4_channel_check` (`generation declares 1 instances with service authority,
need 2`), `sel4_sample_check` (`spawn receiver`), `sel4_supervision_check`
(`the parked handle landed in no slot`), and `sel4_crossing_check` (`crossing
peer`). Verified by stashing this work and re-running each gate at `99f6c45`:
the failure strings match exactly. They follow from the plan-coverage commit's
requirement that every declared instance be planned, and are fixture-migration
work, not regressions from the boot-action cutover.

**2026-08-10 — declarations are matched by destination slot, and the seL4
planes' baseline was established at `3228eb6`.**

Matching ran in two positional runs — grant-backed bindings, then minted ones —
which forced a spawning component to order its grant array by declaration kind.
No component knows about that distinction: `init` lists grants in the order its
child's slots run. Both kinds are now ranked together by destination slot,
which is a total order because the decoder rejects duplicate holder slots in
either section.

`sel4-boot.zti` is migrated on the stream template: nineteen instances for the
components `init` and the fabric construct, thirty-seven minted bindings for
the control channels and supervision handles `init` mints at runtime, and the
two worker executables bound on `fabric-service` at the slots the fabric reads.
`fabric-service` joins `requiredInstances`, since a required instance must be
listed there. The plane moves from refusing generation admission outright
(`UndeclaredFabricParticipant`) to `[init] fabric boot participants spawned` —
the fabric service and all sixteen participants constructed — before a
participant exits non-zero.

**Baseline audit.** Every seL4 plane gate was re-run at `3228eb6`, the
pre-session commit, by checking the tree out and building from it:

| Gate | At `3228eb6` | Now |
|---|---|---|
| `sel4_root_boot_check` | pass | pass |
| `sel4_component_graph_check` | pass | pass |
| `sel4_reclamation_check` | pass | pass |
| `sel4_channel_check` | `declares 1 instances with service authority, need 2` | identical |
| `sel4_sample_check` | `spawn receiver` | identical |
| `sel4_supervision_check` | `the parked handle landed in no slot` | identical |
| `sel4_crossing_check` | `crossing peer` | identical |
| `sel4_spawn_check` | `spawn plane fail: console` | both children spawn; `sysinfo` exits later |
| `sel4_stream_check` | `spawn subscriber` | whole graph provisioned; later buffer remap |
| `sel4_boot_check` | `SLIME_ROOT FATAL` | fabric and all sixteen participants spawn |

No gate regressed. Four are unchanged and were already red before this work;
three improved materially; three that passed still pass.

**2026-08-10 — independent security review of the v5 work, and the three
defects it found.** A read-only reviewer traced `preflight_spawn_grants` end to
end against five questions. Two positive results and three defects:

*Confirmed sound.* A spawning component cannot place a capability at an
undeclared slot or exceed the declared ceiling. The destination comes only from
the declaration, never the request; rights are bounded by both the ceiling and
what the parent actually holds; declaration count, duplicate source slots, and
re-passing the executable slot are each refused; and installing into an
occupied slot fails. Authority also remains un-amplifiable after the
`owner-missing` removal, bounded by `min(ceiling, parent's held rights)`.

*Fixed.*

| Defect | Fix |
|---|---|
| The decoder rejected duplicate holder slots only *within* the minted section, so a minted binding could collide with one of the holder's own grant-backed bindings, making the slot ranking ambiguous | Cross-section collision now rejected in both the decoder and its Python twin (`7c28812`) |
| A zero-rights request passed both the ceiling and held-rights tests, installing an inert capability into the slot its declaration reserved | Refused as malformed (`703eac7`) |
| The Python checker discarded the minted record's reserved tail via its struct pad, so a record the decoder rejects passed the independent checker | Tail asserted zero (`703eac7`) |

*Accepted as a documented limit, not a defect.* `MintedBinding` does not bind
object identity — it cannot, since the object does not exist until the minter
creates it. A minter may satisfy a declaration with a capability of the right
kind and rights but a different underlying object: a supervision handle naming
another of its children, or a directory capability at a broader scope. Endpoint
substitution is incidentally neutralized because `construct_child` re-installs
declared channel ends from the `ChannelTable` rather than the passed grant, but
the other kinds are not covered. The earlier claim in this entry that the
record keeps the edge "authenticated" overstated it; the doc comment on
`MintedBinding` now states precisely what is and is not fixed before
activation, and says that a relationship needing identity pinned must use a
`Grant` against a concrete object.

The reviewer also read `transferable` as drift, since `Grant` carries a
separate flag and `MintedBinding` folds it into the rights word. That is
deliberate and is now documented on the record: one representation cannot
disagree with itself, whereas `Grant`'s two must be cross-checked for
coherence. The devlog sentence claiming `MintedBinding` "carries
`transferable`" was loose — it carries the property, as a rights bit.

**2026-08-10 — independent correctness review, and the regression it caught.**
A second reviewer checked the layout arithmetic and the boot-action cutover.

*Confirmed sound.* All 51 header field offsets were recomputed from the schema
field widths and compared against the decoder's 41 hardcoded byte literals and
the generated Python: all three agree, including `minted_binding_count` at 184,
`header_reserved` at 188, `minted_binding_offset` at 336, and `total_len` at
368. Fields sum to 376, plus 136 pad, equals the declared 512. All twenty
record layouts satisfy `used + trailingPadding == declared length`.

*P0 — three seL4 planes were made unreachable, and the earlier claim in this
entry that "each flag now names exactly one composition" was false.* Deleting
the second guard half from the three x86 oracle branches assumed the seL4
planes no longer set the oracle flag. They still do, deliberately:
`build-generation.py` maps `sel4-qos`, `sel4-operation`, and `sel4-visibility`
to both their seL4 flag *and* the oracle's, because the participants read the
oracle flag to select their QoS behaviour and must stay byte-identical between
the planes. So those images compiled with the oracle flag set, took the x86
branch, and exited before `compose_declared_graph` ran. Observed directly: the
seL4 QoS image died at `SLIME_ROOT FATAL`; after the fix it reaches
`drive_stream_plane`, its own composition. The fix moves the dispatch *above*
the oracle branches, so the exclusion is the authenticated action rather than a
second build flag — anything other than `PRODUCT` composes and does not return.
These three gates are in the known-red set, so the regression would have been
masked rather than caught.

*Also fixed.*

| Finding | Fix |
|---|---|
| `init`'s `boot_action` table is a hand copy of the contract's numbering with nothing tying them together; the new `boot_action_numbering_is_frozen` test pins the enum but not the copy | A `const _: () = assert!(...)` per variant in `init.rs`, verified by renumbering `QOS` and observing `assertion failed` at build time |
| Header byte 188 (`header_reserved`) was checked by the Python twin but not the decoder | `reserved_zero(bytes, 188, 192)` in `decode` |
| `is_v4()` returned a hardcoded `false` with no callers, and `kernel_object` was set to `usize::MAX` and read by nothing — both `pub`, so `dead_code` could not flag them | Deleted |
| The rank selector's slot comparison is unreachable once duplicate holder slots are rejected | Replaced with an explicit uniqueness assertion that fails the spawn, since a collision would mean the decoder admitted something it should not have |
| `build_sel4_plan` took an `executables` parameter it never read | Removed |

**2026-08-10 — the full graph reaches its healthy-idle terminal, and a minted
endpoint never actually reached its holder.**

*Root cause.* Child construction skipped every endpoint grant, on the rule that
`ChannelTable` re-installs a declared channel end from the generation's single
pre-created edge — copying the parent's end would install the wrong side. A
*minted* endpoint has no such edge: its object exists only because the parent
created it at runtime, which is exactly what the declaration defers. So the
skip silently dropped it and the holder's slot stayed empty, which surfaced as
`fabric-op-client-b-restart` failing its own `recv` on slot 0. The plan now
records which grants a minted declaration authorized, and those are copied
rather than skipped.

*Rule relaxed.* `exec` was barred on a minted binding, a rule chosen when the
record only carried channels. An owner may legitimately hand a child an
executable it holds — that is how the fabric spawns its two bounded route
workers — so `exec` is admissible, paired with `spawn`, in the decoder, the
builder, and the checker alike. The pairing is what keeps a minted executable
from reaching a holder the graph did not authorize to spawn.

*Fixture.* `sel4-boot.zti` needed the control edges to be **real grants**, not
minted ones: `build-generation.py` derives the fabric's entire control-slot
layout by scanning grants named `<participant>-control` with
`source = participant, target = fabric-service`, so declaring them minted left
the generated profile empty and the fabric read its slots from nothing. Control
edges and the fabric's two factories are now grants; the subscriber supervision
handles, the worker executables, and each route worker's own control set are
minted. Slot layout follows the generated profile exactly: factories 0–1,
stream controls 2–8, supervision 9–11, call/operation controls 12–20, worker
executables 21–22.

*Result.* `sel4_boot_check` moves from refusing generation admission outright
to the supervisor's terminal record —
`SLIME_GRAPH healthy generation=22 instances=11d74d026c7321ec required=1 live=1
idle=1 failed=0` — with all sixteen participants provisioned across five
routes and both bounded route workers spawned holding their six declared
capabilities each. The gate's frozen `instances=1` becomes `instances=20`,
which is the migration itself: `init`, the fabric, its two workers, and the
sixteen participants are each declared so the generation can state what its
owner hands it at spawn. The gate still fails, now on the call plane's role
provisioning rather than anywhere in the capability path.

The stream plane's residual `Caught cap fault` is unrelated to declarations: it
follows a shared-buffer frame alias (`ARMPageMap: Attempting to remap a frame
that does not belong to the passed address space`) after `loan mapped`
succeeds. Present at `3228eb6` as well.
