# Backlog (defects and unmasked debt)

**Purpose:** Track concrete defects, regressions, and latent bugs found in
implemented code that must be resolved before starting new roadmap-track
milestones. Backlog items are not new capability; they restore an already
claimed exit condition or remove debt that would compound under new work.

**Priority:** Backlog items are handled before roadmap-track milestones. A green
verification suite is a precondition for milestone work, not a milestone itself.
Clear or explicitly defer every open item here before opening a new track gate.

**Entry shape:** Each `### B<N> — <title>` heading is fixed once assigned —
never renumbered or reworded, since devlog `Roadmap` fields and anchored links
resolve against that exact text. An open item states the problem, the
evidence (how it was observed), the proposed fix, and the exit condition
that closes it. Close an item only when its exit condition is observed,
then collapse it in the resolved log below to five lines: `**Status:**`
(resolution date and class), `**Was:**` (one sentence naming the
observable wrong behavior), `**Exit condition (observed):**` (one sentence,
past tense), and `**Evidence:**` (a link to the devlog entry). The
investigation, fix narrative, and full evidence live in that devlog entry
and are not duplicated here; a resolved item with no devlog link stays in
full and says why.

## Open

(none)

## Deferred follow-ups

Real, unclosed follow-up work surfaced by resolved entries, not new backlog
items in their own right — each stays open until the linked devlog's
successor closes it.

- B61 left `serve_instance_graph` untestable pending a seL4 object-invocation seam.
- B63 left marker expectations as Python literals rather than blessable fixtures.
- B60 left two authority-derivation steps in Python rather than schema-declared.

Evidence for all three: [`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md).

- C10.4 left `fabric-service`'s 16-page private-memory quota uniform across ten
  fixtures whose declared graphs need four pages at most — the same
  over-declaration C10.4 removed from `.bss`, one level up. The builder already
  computes each generation's frame demand, so deriving the quota from the
  declared graph is possible.
- C10.4 left the remaining worst-case-sized components unconverted
  (`fabric-subscriber-b`'s mailboxes, the store plane's bump-allocator
  components). The mechanism is now proven on a product component, so these are
  ordinary work rather than milestone work.

Evidence for both: [`devlog/2026-08-24-c10-4-adoption-and-leak-evidence/`](../devlog/2026-08-24-c10-4-adoption-and-leak-evidence/index.md).

**Closed 2026-08-21:** B65's remaining half — "the 52-binary fixture population
uncollapsed" — is resolved by CP3 in the [Component platform
track](10-component-platform.md). `components/bins` is now 52 independent
workspace packages rather than one crate's hand-listed `[[bin]]` table, and
`just component_crate_split_check` pins that shape: a crate declaring more than
one binary, or one whose binary and directory names disagree, fails the gate.
**Evidence:** [`devlog/2026-08-21-cp3-crate-per-component/`](../devlog/2026-08-21-cp3-crate-per-component/index.md)

## Resolved
### B77 — `budget_us`/`period_us` are authenticated but unvalidated and unread

**Status:** Resolved 2026-08-24. **Class:** Debt (authenticated wire fields no
mechanism honoured, admitted by both readers and consumed by neither).
**Was:** A v5 generation could declare a nonzero `budget_us`/`period_us`, pass
`check-generation.py` and `Generation::validate`, boot, and be scheduled with no
budget at all, because the builder's zero was a convention rather than an
admitted invariant and `slime-root` read only `Schedule.priority`.
**Exit condition (observed):** both readers now refuse with distinct reasons —
`UndeclarableCpuBudget` from the host oracle and `DecodeError::NonZeroReserved`
from the decoder — and `just generation_check` drives four resealed mutations
(both fields, first and last schedule record) through both readers, each guard
proven load-bearing by weakening one at a time and observing the gate fail;
`just contracts_check`, `just test_sel4_root` (152), and
`just sel4_root_boot_check` pass.
**Evidence:** [`devlog/2026-08-24-b77-undeclarable-cpu-budget/`](../devlog/2026-08-24-b77-undeclarable-cpu-budget/index.md)

### B70 — component definitions and slot/route bindings are compile-time-coupled to one crate's private manifest parser, blocking out-of-tree components

**Status:** Resolved 2026-08-22. **Class:** Debt (a compile-time coupling that
made out-of-tree component development architecturally impossible, closed over
the CP0-CP5 track and eleven incremental migrations).
**Was:** Nineteen component sources compiled their CSpace slot numbers, route
tables, QoS policies, declared ceilings, command tables, and the composition
they branch on out of `build.rs`-private constant tables one crate generated by
string-parsing one generation manifest, so no component could be built outside
that crate against that manifest.
**Exit condition (observed):** `contracts/component-spec/v1` and
`contracts/system-spec/v1` exist and validate under `just contracts_check`;
`valid.zti` and `sel4-channel.zti` derive from those sources byte-identically;
no component source `include!`s a manifest-derived constant table, the renderer
and both command generators are deleted, and the full seL4 gate suite passes.
**Evidence:** [`devlog/2026-08-22-b70-profile-include-closure/`](../devlog/2026-08-22-b70-profile-include-closure/index.md)

### B75 — the fabric graph intermittently stops draining under host load, and the root cannot tell that from success

**Status:** Resolved 2026-08-20. **Class:** Defect (a gate asserting a property
the system never had, over three fields sampled from scheduling state rather
than determined by control flow).
**Was:** `just sel4_fabric_aggregate_check` compared every rendered `[trace]`
field verbatim and measured 6/10 under 24 spinners on 18 cores, diverging on a
resource record's `high_water` peak, a peer-death record's `now=` instant, and a
call-plane ack's arrival ordinal.
**Exit condition (observed):** the stall half closed by measurement — it did not
recur once in twenty loaded runs — and the divergence half closed on the exit
condition's second branch, the comparison being revised by a decision entry to
separate the fields a composition declares from those a run observes, scoped per
resource counter and per `(worker, order)` so a stream `resourceMapping`
regression and the call and operation planes' undeferred peer-death instants
stay compared; `just sel4_fabric_aggregate_check` then passed 10 consecutive
runs at 24 spinners on 18 cores with no `-icount` pin.
**Evidence:** [`devlog/2026-08-20-b75-observed-vs-declared-trace-fields/`](../devlog/2026-08-20-b75-observed-vs-declared-trace-fields/index.md)

### B76 — `IpcError::PeerDead` is declared and status-mapped but never constructed, so three brokers carry unreachable death-detection arms

**Status:** Resolved 2026-08-20. **Class:** Defect (a status the transport can
never produce, read as working redundancy behind two real detection gaps).
**Was:** `IpcError::PeerDead` had zero constructors and 43 unreachable
consuming arms across 14 files; `call_broker` inferred its clock's death from
the *server's* unrelated supervision handle, and `operation_broker`'s
`time_closed` was a write-only self-latch nothing else read.
**Exit condition (observed):** the variant and every unreachable arm were
removed; the call-plane clock was granted its own supervision handle
(`fabric-call-time-supervision`, slot 9, three fixtures) and the broker's exit
predicate now closes on it directly, with a park-condition fix (found by
review) preventing the broker from blocking on a wake no remaining peer could
signal; the operation plane's lack of clock supervision was recorded as a
deliberate decision rather than left implicit. `just sel4_call_check`,
`just sel4_operation_check`, and `just sel4_fabric_aggregate_check` (279
byte-identical trace records) all pass.
**Evidence:** [`devlog/2026-08-20-b76-peer-death-cleanup/`](../devlog/2026-08-20-b76-peer-death-cleanup/index.md)

### B74 — the aggregate gate's traffic schedule failed twice in one session on a gate B68 closed as deterministic

**Status:** Resolved 2026-08-20. **Class:** Defect (a root that stopped serving
without saying so, behind a deliberate guard suppression, plus a load-coupled
gate that could not name either failure).
**Was:** once the graph certified healthy, the root ran its dispatcher bound out
with tasks still live and exited printing only its ordinary service summary, so a
wedged guest was indistinguishable from a running one; because QEMU's `-serial
mon:stdio` does not exit on guest quiescence, the gate blocked until its watchdog
fired and reported a bare timeout naming nothing.
**Exit condition (observed):** clause 1 was unattainable — the gate failed 6/10
boot-pairs at 24 spinners on 18 cores while passing 0/10 idle and at 4 spinners,
so it cannot pass 10 consecutive runs under load comparable to the failures — so
closure went through clause 2: the sensitivity was identified (a fixed `-icount
shift=3` suppressed both signatures 6/10 — 0/10 under held load) and the root now
emits `SLIME_GRAPH exhausted live=N iterations=N certified=1`, on which the gate
failed a real `slime-sel4-fault.elf` boot at 16.8s against its 240s watchdog,
naming 32768 iterations and 7 live tasks, while six healthy boots of the same
image passed in 14.1-15.1s.
**Evidence:** [`devlog/2026-08-20-b74-aggregate-flake/`](../devlog/2026-08-20-b74-aggregate-flake/index.md)

### B73 — the matrix plane never checks the view a graph-visibility holder pages, only the observer's

**Status:** Resolved 2026-08-20. **Class:** Defect (a plane that asserted one
branch of its visibility policy and was read as covering both).
**Was:** the matrix plane asserted only what a `private` holder is denied, so
the route set shown to a `graph` holder was computed every boot and never read,
and a mutation erasing `telemetry-alt` from every graph-wide view stayed green.
**Exit condition (observed):** `fabric-publisher` now pages its own view and
asserts the three declared route names in order, and the admission-neutral flip
of both `telemetry-alt` participants from `graph` to `private` failed
`just sel4_matrix_check` naming `telemetry-alt`; restoring the fixture returned
it to green.
**Evidence:** [`devlog/2026-08-20-b73-matrix-graph-view/`](../devlog/2026-08-20-b73-matrix-graph-view/index.md)

### B72 — the visibility plane's QoS view records are counted but never checked against the route they describe

**Status:** Resolved 2026-08-19. **Class:** Defect (a gate that asserted a
structural property and was read as covering a semantic one).
**Was:** `just sel4_visibility_check` counted the composition's twelve view
records and two distinct traces but decoded none of them, so every field the
broker copies out of the declared graph — each route's name and its transport
QoS — went unconstrained, and a mutation swapping the two routes' declared QoS
left the gate green.
**Exit condition (observed):** the gate decoded all twelve records against a
frozen fixture using offsets the visibility contract renders itself, and an
admission-neutral swap of the two routes' declared `historyDepth` failed
`just sel4_visibility_check`, naming the two records that moved; restoring the
fixture returned it to green.
**Evidence:** [`devlog/2026-08-19-b72-frozen-visibility-view/`](../devlog/2026-08-19-b72-frozen-visibility-view/index.md)

### B71 — the boot-layout resource's binary encoding was never cross-checked against the manifest's real bindings, and unnamed roles are never narrowed

**Status:** Resolved 2026-08-18. **Class:** Defect (write-only generated data
that had silently drifted, found by its first real consumer).
**Was:** the boot-layout resource embedded in `generation.bin` encoded a second,
static statement of where the root places the bootstrap component's
capabilities, and it had drifted from the `InstanceBinding` records the root
actually places from — `spawn-service` at slot 4 against the real 5. Separately,
a role the generation does not grant kept whatever slot the full static table
gave it, so the component-graph plane's generated constants declared
`STORAGE_CAPABILITY_SLOT` as `ECHO_AGENT_SLOT`'s real 7 and
`GENERATION_CONTROL_SLOT` as the real shared-buffer factory's 8.
**Exit condition (observed):** the resource and the Rust constant table are both
derived from the manifest's own bindings by `boot_layout.layout_from_manifest`,
so neither can drift and an ungranted role renders `SLOT_ABSENT`; all 25 seL4
planes agree across resource, constants, and the frozen `.layout` the root
resolved (106 rows, verified by a new `check-boot-layout-resource.py` arm that
also refuses two differently-named constants at one slot), each half proven
non-vacuous by re-injecting its original defect; `just sel4_boot_layout_check`
matches all 25 frozen fixtures unchanged, and `just generation_check`,
`contracts_check`, `runtime_binding_resolution_check`, `test`, `fmt_check_all`,
`lint_all`, `ruff`, and `typos` pass. `init.rs`'s `main()` migration that B71
blocked now resolves `console`/`dango`/`spawn-service` through the CP2 query.
**Evidence:** [`devlog/2026-08-18-b71-boot-layout-binary-drift/`](../devlog/2026-08-18-b71-boot-layout-binary-drift/index.md)

### B69 — RP1's artifact gate never ran, and hid three admission defects

**Status:** Resolved 2026-08-17. **Class:** Defect (a claimed-green gate could
not reach its assertions, over a missing admission check).
**Was:** `just rpi5_artifact_check` crashed with `KeyError: 'instances'` before
reaching its assertions, and behind that crash sat three further defects: a
stale-boot-action rejection, a self-ownership check that misfired on
root-owned instances at index 0, and an admission path that never validated
the kernel object's target profile at all.
**Exit condition (observed):** `just rpi5_artifact_check` passes, reporting
the RPi5 target-qualified executable closure and artifacts passed, with
wrong-target kernels and per-component substitutions now rejected; `just
generation_check`, `just contracts_check`, and `just ruff` pass.
**Evidence:** [`devlog/2026-08-17-ros2-transport-zenoh-pivot/`](../devlog/2026-08-17-ros2-transport-zenoh-pivot/index.md)

### B68 — the determinism gate compared one scheduling interleaving

**Status:** Resolved 2026-08-17. **Class:** Defect (a gate asserting
determinism was itself nondeterministic).
**Was:** `just sel4_fabric_aggregate_check` failed about one run in four,
because its comparison zipped trace records positionally — asserting one
scheduling interleaving rather than the trace's actual per-worker-per-kind
determinism.
**Exit condition (observed):** `just sel4_fabric_aggregate_check` passes 10
consecutive runs, comparing all 280 records grouped by `(worker, kind)`
across both schedules, verified against a real injected divergence; `just
sel4_gate_control_check`, `just ruff`, and `just typos` pass.
**Evidence:** [`devlog/2026-08-17-b68-aggregate-trace-determinism/`](../devlog/2026-08-17-b68-aggregate-trace-determinism/index.md)

### B65 — `init.rs` held 21 plane launchers in one file; four moved out and the binary collapse did not

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (per-gate accretion).
**Was:** `init.rs` was 2286 lines with 21 `drive_*_plane` launchers making up
39% of it, dispatched by a hand-written match, and every plane's edit shared
one file with every other plane's.
**Exit condition (observed):** `init.rs` is 1644 lines with four plane
launchers moved into their own modules; `just sel4_loan_check`,
`sel4_spawn_check`, `sel4_crossing_check`, `sel4_supervision_check`, the full
seL4 plane gate suite, `fmt_check_all`, `lint_all`, and `ruff` pass. The
remaining 17 launchers and the 52-binary collapse are deferred (see Deferred
follow-ups).
**Evidence:** [`devlog/2026-08-17-b65-plane-modules/`](../devlog/2026-08-17-b65-plane-modules/index.md)

### B63 — the plane gates reimplemented helpers a shared library should own

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (verification-code
duplication).
**Was:** 39 seL4 gates each carried their own copy of pinned-QEMU-profile
readers and an artifact hasher instead of importing
`scripts/lib/harness.py`, so a pin reader duplicated 31 times was 31 chances
for gates to disagree on what machine profile to accept.
**Exit condition (observed):** no seL4 gate defines its own `profile_text`,
`profile_integer`, or `sha256_file`; all 33 migrated gates plus
`sel4_gate_control_check` (32 gates reject 1227 mutations) and `just ruff`
pass. Marker expectations remaining as Python literals is deferred (see
Deferred follow-ups).
**Evidence:** [`devlog/2026-08-17-b63-gate-helper-consolidation/`](../devlog/2026-08-17-b63-gate-helper-consolidation/index.md)

### B61 — `just run` booted a test fixture, and the dispatch routing was untestable

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt.
**Was:** `just run` — documented as "the seL4 product image" — actually built
the two-fixture verification variant, and the 5864-line `main.rs`'s
label→service dispatch routing was reachable only through a booted QEMU
image, since `lib.rs` explicitly excluded `main.rs` from the testable
surface.
**Exit condition (observed):** `just run` boots a product component graph
with zero fixture markers, verified by `just sel4_component_graph_check`; the
dispatch routing moved to `slime-root/src/ipc.rs` with host tests
(`test_sel4_root` 118 → 121/121); `just sel4_boot_check`, `sel4_spawn_check`,
`sel4_root_boot_check`, `fmt_check_all`, and `lint_all` pass. Moving the
remaining service handlers and spawn preflight into `lib.rs` needs an
object-invocation fault-injection seam this repo does not have (see Deferred
follow-ups).
**Evidence:** [`devlog/2026-08-17-b61-product-image-and-dispatch/`](../devlog/2026-08-17-b61-product-image-and-dispatch/index.md)

### B62 — the `.zti` fixtures were copy-paste: three 1882-line files differed by one field

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt
(B55's staleness mechanism).
**Was:** 30 `sel4-*.zti` fixtures totalled 16978 lines with nine pairs over
85% identical and three at 99.9%, so every plane addition was a full-file
copy and hand-renumber — the same pattern that caused B55's first defect.
**Exit condition (observed):** three fixtures (3728 lines) deleted in favor
of a declarative `VARIANT_GENERATION_DELTAS` mechanism in `build-sel4.py`;
27 fixtures remain (12172 lines), pairs over 85% identical fell from 9 to 3;
`just contracts_check`, `just generation_check` (byte-identical isolated
builds), `just data_fabric_profile_check`, the traffic/fault/saturation/
matrix/fabric-aggregate/boot-layout/gate-control checks, and `just ruff`
pass.
**Evidence:** [`devlog/2026-08-17-b62-fixture-deltas/`](../devlog/2026-08-17-b62-fixture-deltas/index.md)

### B64 — the format-coexistence answer existed in code but was written down nowhere, and one retained schema was unguarded

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt
(bears on roadmap invariant 7).
**Was:** the audit judged generation admission's pinned-version equality test
irreconcilable with rollback booting an older generation, and reported five
dead schema trees — but `contracts/generation/v4` was the only genuinely
unguarded one, and the repository already had two undocumented mechanisms
making format bumps rollback-safe by refusal rather than migration.
**Exit condition (observed):** the format-coexistence rule is documented at
`roadmap/README.md` invariant 7 naming both mechanisms; `just
sel4_boot_selection_check` gained an arm proving a v4-stamped pending
generation is refused without consuming the known-good root, verified
against a real divergence; `just contracts_check` now includes
`generation/v4`; `just ruff` passes.
**Evidence:** [`devlog/2026-08-17-b64-format-coexistence/`](../devlog/2026-08-17-b64-format-coexistence/index.md)

### B60 — authority-derivation policy lived in the builder, and one slot number had two independent sources

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt
(B55's mechanism).
**Was:** which grants constitute a control plane and where they terminate
lived in unconstrained Python functions, and one control slot had two
independent sources — a fixture-pinned integer and a runtime-recomputed one —
agreeing only by comment, the same shape that caused B55's root cause.
**Exit condition (observed):** `_assert_declared_control_slots` refuses at
build time any manifest whose pinned control slots disagree with the derived
order, verified by perturbing one slot and observing the named build
failure; `just contracts_check` (31 manifests), `just generation_check`
(byte-identical isolated builds), `just data_fabric_profile_check`, `just
sel4_boot_layout_check` (25 plane layouts), the boot/matrix/traffic/fault/
stream/visibility/call/operation checks, `just ruff`, and `just
fmt_check_all` pass. Supervision-table membership and notification-slot
naming remain in Python (see Deferred follow-ups).
**Evidence:** [`devlog/2026-08-17-b60-control-plane-authority/`](../devlog/2026-08-17-b60-control-plane-authority/index.md)

### B66 — `ipc.rs` carried two retired-mechanism constants, one of them load-bearing

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (B46 residue).
**Was:** `slime-root/src/ipc.rs` declared `CHANNEL_CAPACITY` (dead) and
`MAX_WAIT_SOURCES` (live, feeding generation admission) describing a root
wait-set mechanism B46 had already deleted, with the live one a third
spelling of a ceiling `contracts/fabric-graph/v1/schema.zt` already
declared.
**Exit condition (observed):** `CHANNEL_CAPACITY` deleted; `MAX_WAIT_SOURCES`
re-exports the one generated declaration; nine stale `SYS_WAIT` references
corrected. `just test_sel4_root` (118/118), `just data_fabric_profile_check`,
`just generation_check`, `just contracts_check`, `just sel4_boot_check`,
`just ruff`, `just fmt_check_all`, and `just lint_all` pass.
**Evidence:** [`devlog/2026-08-17-b59-b66-syscall-abi-contract/`](../devlog/2026-08-17-b59-b66-syscall-abi-contract/index.md)

### B59 — the syscall ABI had no single source: 97 rights declarations, two label tables, three error tables

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt.
**Subsumed:** B57's remaining duplication.
**Was:** four number tables crossing the root/userspace boundary — rights (97
sites), operation labels, error codes, and the spawn-grant record layout —
were hand-authored in multiple places with nothing forcing agreement, a
class of defect that had already caused a silent keystroke-misdecode
regression.
**Exit condition (observed):** a new `contracts/syscall-abi/v1` contract
generates the one shared module both crates consume; 97 hand-written rights
sites collapsed to 1 sentinel; regression tests pin all 23 labels, 6 status
codes, and the record layout, verified to bite by renumbering a label and
observing the freeze test abort. `just contracts_check`, `just
generation_check`, `just test_host`, `just test_sel4_root` (118/118), `just
architecture_contract_check`, `just sel4_root_boot_check`, the full seL4
plane suite, `just fmt_check_all`, `just lint_all`, and `just ruff` pass.
**Evidence:** [`devlog/2026-08-17-b59-b66-syscall-abi-contract/`](../devlog/2026-08-17-b59-b66-syscall-abi-contract/index.md)

### B67 — two negative controls picked declared slots, so neither could fail

**Status:** Resolved 2026-08-17. **Class:** Gate defect (negative controls
that proved nothing).
**Was:** `just sel4_capability_layout_check` failed because two of its six
CSpace-mutation arms computed their victim slot by restating a subset of the
predicate they were meant to violate, so both landed on an already-declared
slot and could never demonstrate a real defect.
**Exit condition (observed):** `ChildSlots` now owns the declared-slot
predicate that all mutation arms consult; verified non-vacuous by weakening
the audit three separate ways and observing each weakening make a specific
arm falsely pass, then reverting. `just sel4_capability_layout_check` passes
all 6 named mutations; `just test_sel4_root` (118/118), `just
sel4_boot_check`, `just sel4_boot_layout_check`, `just sel4_root_boot_check`,
`just fmt_check_all`, and `just lint_all` pass.
**Evidence:** [`devlog/2026-08-17-b67-blind-negative-controls/`](../devlog/2026-08-17-b67-blind-negative-controls/index.md)

### B57 — `RIGHT_ALL` had two definitions, and the wider one admitted an undefined rights bit

**Status:** Resolved 2026-08-17. **Class:** Defect (admission accepted what
no contract defined).
**Was:** the valid capability-rights set was computed two ways — an
enumerated union in the builder, a bit-width mask `(1 << 26) - 1` in the root
and its checker — and the two disagreed by exactly one undefined bit, which
admission's wider mask accepted even though no fixture could produce it.
**Exit condition (observed):** the rights vocabulary is now declared once in
`contracts/generation/v5/schema.zt` and generated on both sides as a fold
over named bits, with a regression test recomputing `RIGHT_ALL` and asserting
bit 17 stays clear, verified to bite by reverting the generated constant.
`just test_host`, `just test_sel4_root` (118/118), `just contracts_check` (31
manifests), `just generation_check` (byte-identical isolated builds), `just
architecture_contract_check`, `just sel4_root_boot_check`, `just
sel4_boot_check`, `just ruff`, `just typos`, `just fmt_check_all`, and `just
lint_all` pass.
**Evidence:** [`devlog/2026-08-17-b57-b58-rights-vocabulary/`](../devlog/2026-08-17-b57-b58-rights-vocabulary/index.md)

### B58 — `check-architecture-contract.py` hand-copied three generated header offsets

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (Zutai-rule
violation with a known prior drift).
**Was:** `object_payload` read the v5 generation header with three literal
byte offsets under a comment admitting they had already drifted once, even
though all three already had generated names in the `@generated` module the
file already imported from.
**Exit condition (observed):** all three literals now read through their
generated names; no numeric header offset remains in the file. `just
architecture_contract_check` (including its 181 boot-contracts unit tests)
and `just ruff` pass.
**Evidence:** [`devlog/2026-08-17-b57-b58-rights-vocabulary/`](../devlog/2026-08-17-b57-b58-rights-vocabulary/index.md)

### B56 — `data_fabric_profile_check` asserted a contradiction and had been red since B55

**Status:** Resolved 2026-08-17. **Class:** Gate defect (a check that could
not pass).
**Was:** `just data_fabric_profile_check` failed on the `unified` profile
because the gate swept every declared profile through one resolution rule,
but B55 had given `unified` a per-plane worker holder while every other
profile still terminates at `fabric-service` — a manifest declaring both
cannot satisfy both rules, so the gate demanded a contradiction, unobserved
since B55 landed.
**Exit condition (observed):** the sweep now resolves only the single-broker
profiles and fails loudly if a manifest declares none of them, with
`unified` left to its four boot gates for coverage. `just
data_fabric_profile_check`, `just sel4_boot_check`, `sel4_traffic_check`,
`sel4_fault_check`, `sel4_saturation_check`, `sel4_fabric_aggregate_check`,
`just contracts_check`, `just generation_check`, `just ruff`, and `just
typos` pass.
**Evidence:** [`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md)

### B55 — the full-graph boot plane refused its own first spawn, then five more defects behind it

**Status:** Resolved 2026-08-15. **Class:** Regression of a claimed exit
condition.
**Was:** `just sel4_boot_check` stopped at a refused spawn before any fabric
role was provisioned, so C8.10's claimed exit condition — one generation
booting every C8 role simultaneously — had never actually been observed; the
native-IPC cutover left seven further defects latent behind it, each masking
the next, plus a gate that stopped reading its own transcript before the
markers it needed could appear.
**Exit condition (observed):** `just sel4_boot_check` passes — 30 markers
across 5 causal chains, 19 composition tasks reaching 5 checked roles plus 10
declared role-less idles, none exited, stable across repeated boots. `just
sel4_boot_layout_check` (24 plane layouts), `just sel4_gate_control_check`
(28 gates reject 1082 mutations), `just contracts_check`, `just
generation_check`, `just sel4_root_boot_check`, the stream/qos/call/
operation/visibility checks, `just test_sel4_root`, `just fmt_check_all`,
`just lint_all`, `just ruff`, and `just typos` pass.
**Evidence:** [`devlog/2026-08-15-b55-full-graph-boot-restoration/`](../devlog/2026-08-15-b55-full-graph-boot-restoration/index.md), [`devlog/2026-08-17-structural-audit/`](../devlog/2026-08-17-structural-audit/index.md)

### B53 — dango echoed a line one byte past the message bound

**Status:** Resolved 2026-08-14. **Class:** Defect.
**Was:** `just sel4_dango_check` ran the first scripted command to
completion, then `dango` exited before reading the second line — its line
buffer was sized independently of the transport's message bound, so a
65-character line was refused one byte past it, with two further
B46-residue defects (an unclaimed working-directory capability, a session
with no shutdown signal) behind it.
**Exit condition (observed):** `just sel4_dango_check` passes: all four
scripted lines run, including denied and parse-error cases, and the session
closes on the scripted escape. `just fmt_check_all`, `just lint_all`, `just
test_sel4_root`, and `just test_host` pass.
**Evidence:** [`devlog/2026-08-14-b53-b54-last-two-planes/`](../devlog/2026-08-14-b53-b54-last-two-planes/index.md)

### B54 — the stress plane borrowed a component that never ends

**Status:** Resolved 2026-08-14. **Class:** Defect.
**Was:** `just sel4_stress_check` staged all 23 declared instances but never
reclaimed to zero live tasks, because 21 stress instances ran a component
whose main thread blocks waiting for a sender the stress fixture never
declared, so every instance parked forever.
**Exit condition (observed):** `just sel4_stress_check` passes: 23 instances
staged and the graph reclaimed to zero live tasks. `just fmt_check_all` and
`just lint_all` pass.
**Evidence:** [`devlog/2026-08-14-b53-b54-last-two-planes/`](../devlog/2026-08-14-b53-b54-last-two-planes/index.md)

### B46 — logical ChannelTable, Transit, ParkedReplies, and WaitSet duplicate seL4 IPC

**Status:** Resolved 2026-08-13. **Class:** Unmasked architectural debt.
**Was:** Slime channels were root-owned queues with userspace-managed blocking, wait sets, reply slots, and peer death, duplicating atomicity and lifetime properties seL4 Endpoints, Reply objects, and Notifications already supply.
**Exit condition (observed):** `channel.rs`, `transit.rs`, `parked.rs`, `WaitSet`, and the migrated universal labels no longer exist; all seven named native IPC gates — `sel4_channel_check`, `sel4_crossing_check`, `sel4_stream_check`, `sel4_qos_check`, `sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check` — passed in one ordered run.
**Evidence:** [`devlog/2026-08-13-b46-native-ipc-completion/`](../devlog/2026-08-13-b46-native-ipc-completion/index.md), [`devlog/2026-08-12-b46-arena-slot-occupancy/`](../devlog/2026-08-12-b46-arena-slot-occupancy/index.md)

### B50 — the logical capability and universal syscall compatibility model remains deletable residue

**Status:** Resolved 2026-08-14. **Class:** Unmasked architectural debt.
**Depends on:** B39–B49.
**Was:** even after native seL4 replacements landed per-plane, the logical
authority database (`GraphTables`), the universal `Operation` dispatcher,
public task IDs, generic cross-kind rights, name-only grants, and fixed-slot
constants remained as a second, competing authority/IPC model — B50 was the
repository-wide proof that none of it survived, blocked on B46 until B46
removed the model it was standing on.
**Exit condition (observed):** exact-source guards find no deleted model
symbols or build flags; every fixture uses generation v5; slot numbers are
auto-allocated per namespace with the byte-pinned boot-layout fixtures
unchanged; every `mintedBindings` of kind `endpoint` (unsatisfiable
post-cutover) is deleted, converting ten plane gates from admission-refused
to green. `just test_sel4_root`, `just contracts_check`, `just
generation_check`, `just sel4_gate_control_check`, every affected `just
sel4_*_check`, `just fmt_check_all`, `just lint_all`, and `just test_host`
pass.
**Evidence:** [`devlog/2026-08-14-b50-minted-endpoint-deletion/`](../devlog/2026-08-14-b50-minted-endpoint-deletion/index.md), [`devlog/2026-08-13-b50-endpoint-create-deletion/`](../devlog/2026-08-13-b50-endpoint-create-deletion/index.md), [`devlog/2026-08-13-r2-declared-slot-allocation/`](../devlog/2026-08-13-r2-declared-slot-allocation/index.md)

### B48 — all child execution shares one fixed priority and no scheduling authority

**Status:** Resolved 2026-08-12, MCS-only clauses explicitly deferred.
**Was:** every child ran at one fixed priority and the generation's schedule
records had no effect on running TCBs; MCS-only budget/period/donation/
timeout-fault features were also unavailable on the selected AArch64 kernel
configuration.
**Exit condition (observed):** priority is authenticated per-thread
generation data, bounded at 254, and applied to boot and spawn TCBs,
observed by a worker at lower priority spinning without starving its main
thread. MCS stays deliberately off — upstream AArch64 MCS proofs are in
progress — with the decision and revisit condition recorded separately.
`just sel4_qos_check`, `just sel4_sample_check`, and the platform-timer
assertions in `just sel4_root_boot_check` pass; `just devlog_check` passes
with the assurance decision indexed.
**Evidence:** [`devlog/2026-08-10-b48-declared-priority/`](../devlog/2026-08-10-b48-declared-priority/index.md), [`devlog/2026-08-10-b48-per-thread-priority/`](../devlog/2026-08-10-b48-per-thread-priority/index.md), [`devlog/2026-08-12-b48-mcs-assurance/`](../devlog/2026-08-12-b48-mcs-assurance/index.md)

### B49 — resource ceilings are reactive tables rather than an admitted object budget

**Status:** Resolved 2026-08-10.
**Was:** static table constants bounded tasks, capabilities, channels, and
transit at the largest graph seen so far rather than proving a generation's
objects fit before activation; a 48-instance generation was admitted and
then died mid-construction with `SlotsExhausted`, because nothing summed
per-instance quotas into an aggregate and the builder excluded the
root-mapped frames that are exactly the resource that runs out.
**Exit condition (observed):** `admit_total_slots` sums every quota against
the allocator's real free-slot count before any component starts; `just
sel4_stress_check` boots the largest admissible 23-instance graph,
constructs and reclaims all 23, and refuses one instance more with a named
`PlanExceedsRootSlots`. `just contracts_check`, `just generation_check`,
`just sel4_reclamation_check`, and `just sel4_boot_check` pass
(`test_sel4_root` at 149). IRQs and untyped size classes remain unmodelled.
**Evidence:** [`devlog/2026-08-10-b49-object-budget/`](../devlog/2026-08-10-b49-object-budget/index.md)

### B47 — package, process, thread, service instance, and lifecycle are one Task model

**Status:** Resolved 2026-08-10.
**Was:** one `Task` meant image instance, CSpace/VSpace owner, single TCB,
service identity, scheduling unit, and lifecycle identity at once, forcing
every component to be single-threaded.
**Exit condition (observed):** a process runs up to `MAX_CHILD_THREADS`
threads sharing one CSpace/VSpace, each with its own TCB, stack, IPC buffer,
transfer window, and schedule, with per-thread TLS identity set in the
register context rather than through a kernel call a later `WriteRegisters`
would overwrite; `sample-worker` declares an extra thread and both threads
are observed printing, with two mutations (never resuming the worker,
aliasing thread indices) confirmed to fail. `just test_sel4_root` (146),
`just sel4_spawn_check`, `just sel4_supervision_check`, `just
sel4_reclamation_check`, `just sel4_boot_check`, and the full 31-plane sweep
pass.
**Evidence:** [`devlog/2026-08-10-b47-runtime-threads/`](../devlog/2026-08-10-b47-runtime-threads/index.md)

### B52 — the loan plane never launches the receiver it loans to

**Status:** Resolved 2026-08-10.
**Was:** `just sel4_loan_check` failed because a loan names its receiver as
the unique live holder of the channel's other end, and the declared receiver
was never spawned — the same defect recurred one arm later for `console`,
and four of the gate's own assertions had never run because it failed
before reaching them.
**Exit condition (observed):** `just sel4_loan_check` passes — a sealed
subrange loaned to a receiver named by capability, mapped read-only,
returned once, and reclaimed; all four declared quota classes refused at
ceiling+1 without disturbing an unrelated holder. `just sel4_sample_check`,
`just sel4_spawn_check`, `just sel4_reclamation_check`, the other 28 plane
gates, `contracts_check`, `sel4_boot_layout_check`, and
`sel4_gate_control_check` pass.
**Evidence:** [`devlog/2026-08-10-b52-loan-plane-peers/`](../devlog/2026-08-10-b52-loan-plane-peers/index.md)

### B51 — the spawn preflight cannot tell a respawn from a first launch

**Status:** Resolved 2026-08-10.
**Was:** `spawn_preflight` checked a request against declared plus minted
bindings assuming one spawn per declaration, but a respawn of a collected
instance is that declaration launched again, and the root had no state
distinguishing it — a partial respawn request could bind positionally to
another declaration's slot under its rights ceiling with no error.
**Exit condition (observed):** a per-instance bitmap (`LaunchedInstances`)
that outlives collection now answers the respawn question; `just
sel4_sample_check` passes with the plane's third spawn (a respawn) admitted
and reaching a clean exit, and a gate-control mutation (a respawn carrying
one grant instead of none) is verified refused. `just sel4_spawn_check`,
`just sel4_reclamation_check`, `just sel4_component_graph_check`, and
twenty-three further plane gates pass.
**Evidence:** [`devlog/2026-08-10-b51-respawn-provenance/`](../devlog/2026-08-10-b51-respawn-provenance/index.md)

### B45 — directory, filesystem, and store services still depend on universal root IPC

**Status:** Resolved 2026-08-10.
**Was:** Directory inspection, derivation, and commit, and store requests, reached clients as operation labels on the root endpoint, so capability provenance was checked in a global software table rather than expressed by holding a service endpoint.
**Exit condition (observed):** `just sel4_directory_check`, `just sel4_filesystem_check`, `just sel4_store_check`, `just sel4_powerbox_check`, and `just sel4_dango_check` all passed, with `DirectoryInspect`, `DirectoryCommit`, and `StoreTransact` absent from `slime-root/src/ipc.rs::Operation`.
**Evidence:** [`devlog/2026-08-10-b45-directory-service-split/`](../devlog/2026-08-10-b45-directory-service-split/index.md)

### B44 — generation and recovery policy still crosses the universal root dispatcher

**Status:** Resolved 2026-08-10.
**Was:** `HealthConfirm`, `RecoveryReconstruct`, `GenerationTransact`, and
`GenerationReceive` entered the universal root dispatcher, coupling policy
clients to root's global request ABI after B35 made the durable boot
selector authoritative.
**Exit condition (observed):** all four labels are gone from
`slime-root/src/ipc.rs::Operation`, so a client is denied by seL4 lookup
with no root-side path at all; `just sel4_generation_check`, `just
sel4_boot_selection_check`, `just sel4_rollback_check`, `just
sel4_recovery_plane_check`, and `just sel4_transfer_check` all pass.
**Evidence:** [`devlog/2026-08-10-b44-policy-labels-deleted/`](../devlog/2026-08-10-b44-policy-labels-deleted/index.md)

### B43 — block and durable-store clients still transact through root operation labels

**Status:** Resolved 2026-08-10.
**Was:** `BlockTransact` and `StoreTransact` shared the universal
dispatcher, coupling block-IO latency and failure scope to unrelated
clients; a block request needed no declared service capability, only a
label.
**Exit condition (observed):** neither label exists in
`slime-root/src/ipc.rs::Operation`; block requests reach the console
thread on the per-process console endpoint that owns the device tables;
`just sel4_device_check`, `just sel4_storage_check`, `just
sel4_store_check`, `just sel4_rollback_check`, `just
sel4_recovery_plane_check`, `just sel4_transfer_check`, and ten further
planes pass, with multi-device selection asserted exactly.
**Evidence:** [`devlog/2026-08-10-b43-block-service-endpoint/`](../devlog/2026-08-10-b43-block-service-endpoint/index.md), [`devlog/2026-08-10-b43-block-device-renumbering/`](../devlog/2026-08-10-b43-block-device-renumbering/index.md)

### B41 — console and debug traffic still enters the universal root dispatcher

**Status:** Resolved 2026-08-10.
**Was:** `DebugWrite` and console/input-adjacent control shared the same
badged root endpoint and dispatcher as lifecycle, storage, and fabric
traffic, so a noisy client consumed the highest-priority root service loop
and a console defect shared the system-wide dispatcher fault domain.
**Exit condition (observed):** neither `DebugWrite` nor `InputRead` exists
in `Operation`; every process holds a console capability at a declared
slot, minted write-plus-reply and never receive; `just
sel4_root_boot_check`, `just sel4_input_check`, `just sel4_dango_check`,
and thirteen other plane gates pass.
**Evidence:** [`devlog/2026-08-10-b41-console-endpoint/`](../devlog/2026-08-10-b41-console-endpoint/index.md), [`devlog/2026-08-10-b41-second-dispatcher-blocker/`](../devlog/2026-08-10-b41-second-dispatcher-blocker/index.md)

### B42 — spawn and lifecycle control use ambient task IDs and the universal dispatcher

**Status:** Resolved 2026-08-10.
**Was:** `spawn` returned a numeric `task_id` sent across a process
boundary to wait for termination — a name anyone could forge by counting,
not authority.
**Exit condition (observed):** no Zutai wire record or public runtime type
exposes a bare task id; the spawn service keys its live table on the
supervision slot; a stale handle now refuses rather than answering twice;
`just sel4_spawn_check`, `just sel4_supervision_check`, `just
sel4_reclamation_check`, and `just sel4_dango_check` pass;
`check-lifecycle-identity.py` refuses reintroduction.
**Evidence:** [`devlog/2026-08-10-b42-lifecycle-identity/`](../devlog/2026-08-10-b42-lifecycle-identity/index.md)

### B40 — child CSpaces are fixed four-slot shells rather than admitted authority

**Status:** Resolved 2026-08-10.
**Was:** every child CNode had four compiled-in slots while actual
authority stayed in a root-side `CapabilityTable`, so the kernel could not
enforce the v5 plan's declared per-process CSpace layout.
**Exit condition (observed):** `just sel4_capability_layout_check` boots a
twenty-instance graph, requires every child's CSpace to match the admitted
plan, and refuses six classes of injected mutation; `just sel4_boot_check`,
`just sel4_root_boot_check`, `just sel4_component_graph_check`, `just
sel4_reclamation_check`, `just contracts_check`, `just generation_check`,
and `just test_sel4_root` (140) pass.
**Evidence:** [`devlog/2026-08-10-b40-native-child-cspaces/`](../devlog/2026-08-10-b40-native-child-cspaces/index.md)

### B39 — Generation v5 must describe the exact seL4 object and authority plan

**Status:** Resolved 2026-08-10.
**Was:** generation v4 declared logical objects and grants `slime-root`
reinterpreted, unable to prove the process/thread topology, kernel
objects, mappings, CSpace bindings, scheduling, fault policy, spawn
templates, or dynamic reserve an admitted graph would consume; init also
selected its scenario graph through build flags.
**Exit condition (observed):** `just contracts_check` and `just
generation_check` prove every binding/object reference resolves and two
isolated builds are byte-identical; `just sel4_boot_check` reaches the
supervisor's terminal record with the full twenty-instance graph; no
product code admits v4; every `SLIME_SEL4_*_CHECK` build-flag branch is
gone.
**Evidence:** [`devlog/2026-08-10-b39-generation-v5-checker-cutover/`](../devlog/2026-08-10-b39-generation-v5-checker-cutover/index.md)

### B34 — generation component records conflate executable catalogue entries with initial instances

**Status:** Resolved 2026-08-10.
**Was:** `slime-root` constructed and activated every loadable component in
the generation while `init` also spawned the graph it owns, so the full
C8.10 image ran a root-launched copy and an init-spawned copy of the same
fabric/workers, and the format had one `Component` record for two
different concepts (executable vs. required-at-boot instance) with no
launch-owner or autostart field.
**Exit condition (observed):** a generation-format cutover separates
`Executable` from `Instance` records; a fixture can carry executable-only
images without creating tasks; every declared initial instance is
constructed exactly once by its declared owner; `just sel4_boot_check`
observes the single graph's complete healthy-idle chain.
**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md), [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md)

### B35 — BootState does not select the generation the seL4 product boots

**Status:** Resolved 2026-08-10.
**Was:** the generation admitted by `slime-root` was selected at build time
via `SLIME_GENERATION` and compiled into the root ELF, so the
generation-management/rollback/recovery planes could mutate durable
`BootState` sectors that the next seL4 boot never read to choose which
generation to launch; the generation also retained an inert, never-loaded
`kernelObject` placeholder.
**Exit condition (observed):** a minimal immutable seL4 boot selector
reads the granted boot device, selects and updates the two `BootState`
slots, verifies release/target/generation/object closure, and launches the
selected generation; one QEMU campaign stages a pending generation,
reboots into it, durably consumes failed attempts, returns to known-good
when exhausted, and promotes only after health confirmation; the unused
`kernelObject` is removed.
**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md), [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md)

### B36 — the full-graph gate stops at a non-unique component idle marker

**Status:** Resolved 2026-08-10.
**Was:** `check-sel4-boot-plane.py` treated the generic fabric line "idle:
parked on control endpoints" as the whole system's terminal marker, so
with B34's duplicate graph the checker stopped on the wrong instance
before init's supervision transfer and the graph's real outcome.
**Exit condition (observed):** one supervisor-emitted terminal record
binds the generation identity/instance-set digest and required/live/idle
counts with zero failed instances; `just sel4_boot_check` reaches that
unique record only after every causal chain and fails on any required
nonzero exit; gate-control proves an injected early duplicate idle line
cannot truncate or pass the check.
**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md)

### B37 — dependency activation and non-bootstrap slot ABI are implicit contracts

**Status:** Resolved 2026-08-10.
**Was:** generation dependencies were decoded and validated but the seL4
launch path never consulted them — root activated every task in
component-table order, dependency barriers lived as imperative
spawn/yield sequences in `init`, and non-bootstrap slot numbers were
inferred from grant iteration order, an undocumented shared ABI.
**Exit condition (observed):** dependencies and capabilities bind to
explicit instance records; the builder rejects cycles and unsatisfied
barriers and emits a fixture-checked per-instance capability layout that
boot and spawn both generate from; root activates the declared DAG;
permuting grant declarations leaves local bindings unchanged; a QEMU graph
proves activation occurs only after each declared barrier.
**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md), [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md)

### B38 — task reclamation cannot reuse root CSlots or untyped memory

**Status:** Resolved 2026-08-10.
**Was:** `ObjectAllocator` advanced root CSlots and untyped watermarks
monotonically; task cleanup revoked capabilities but never returned slot
indices or a task's TCB/CNode/page tables/frames to allocatable pools, so
a long-running component manager could exhaust boot-lifetime resources
through bounded spawn/exit cycles alone.
**Exit condition (observed):** each task or task group gets a derived
untyped arena owning its CNode/TCB/VSpace/frames, revoked on death so the
parent can be retyped again, plus a free-list for emptied root CSlots; a
live QEMU stress graph completes more spawn/exit cycles than the prior
watermarks permitted with bounded, stable live counts and no surviving
alias.
**Evidence:** [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md)

### B33 — seL4 cutover review findings

**Status:** Resolved 2026-08-09.
**Was:** the post-cutover static review recorded CUT-001 through CUT-077
across capability isolation, lifecycle cleanup, shared-memory aliases,
storage, userspace services, gate integrity, CI/profile policy, and
project records; several were merge blockers and several gates could pass
without the current artifact or evidence.
**Exit condition (observed):** every finding was re-grounded and repaired,
including separating the capability-subset proof from the fabric control
protocol and dropping init's retained endpoint copies after spawn; focused
root/host tests, supervision, QoS, root-boot, gate-control, and
layout-resource checks pass, along with formatting, Clippy, Python lint,
and dependency policy.
**Evidence:** [`devlog/2026-08-09-b33-cutover-review-remediation/`](../devlog/2026-08-09-b33-cutover-review-remediation/index.md)

### B31 — six oracle properties blocked `kernel/` deletion

**Status:** Resolved 2026-08-09.
**Was:** two deletion audits found six acceptance properties that would
have disappeared with the frozen custom-kernel oracle, plus orchestration
coupling across the workspace, Justfile, check scripts, component
transport, CI, and generation builder.
**Exit condition (observed):** complete component-wrapper admission moved
to `boot-contracts`; the seL4 root boot gate observes independent frame
accounting, exact task/shared-buffer reclamation, clean exit beside
deliberate fault isolation, and panic/fault failure markers; global gate
control proves missing/reordered/contradictory evidence turns every seL4
plane red; `kernel/`, its workspace membership, custom-kernel build/check
orchestration, legacy component syscall transport, and the custom
generation-builder path are removed together; `storage_nvme_read_check`
fails closed rather than being promoted into false product evidence.
**Evidence:** [`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md)

### B32 — three scenario receive spins were invisible to the root

**Status:** Resolved 2026-08-09.
**Was:** the call plane's terminal receiver and two operation-plane
receive paths used `yield_now()` on `ERR_WOULDBLOCK`, so the root could
neither name their endpoint wait nor distinguish a real dependency from an
iteration-budget spin.
**Exit condition (observed):** all three now call `wait(&[WaitSource::Endpoint(...)])`,
with a pre-existing operation teardown race fixed alongside it; `just
sel4_call_check` and `just sel4_operation_check` pass with every affected
timeout, peer-death, and unrelated-route marker present.
**Evidence:** [`devlog/2026-08-09-b32-parked-scenario-receivers/`](../devlog/2026-08-09-b32-parked-scenario-receivers/index.md)

### B29 — one block device per granule

**Status:** Resolved 2026-08-08 (B29, first use — distinct from the `ParkedReplies::wake` defect below).
**Was:** `slime-root` brought up at most one virtio block device, because
QEMU packs eight virtio-mmio transports per 4 KiB granule and
`DeviceRegion::remap` mapped the frame to a driver's standing window,
leaving nothing for a second disk on the same page; declared placement
also hardcoded device 0 and intersected the component's whole rights union
rather than the grant's own.
**Exit condition (observed):** `device::MappedGranule` lets a second
driver read/write its registers at its own offset through a borrow with no
remap/unmap authority; `just sel4_transfer_check` boots with two disks,
both ready, each held by its own capability, with the read-only one
byte-identical afterward; successive block grants now name successive
devices under the grant's own rights.
**Evidence:** [`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md)

### B30 — the dango plane launched no commands

**Status:** Resolved 2026-08-08 (B30, first use — distinct from the `release_trust_check` defect below).
**Was:** the dango plane never actually launched any of its declared
commands — the generation's dango-plane grants and fabric profile were
present but nothing in the boot path executed them, so the plane's
declared shell surface was inert.
**Exit condition (observed):** the dango plane launches and executes its
declared commands end to end on seL4; `just sel4_dango_check` observes the
commands running and their output reaching the console.
**Evidence:** [`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md)

### B25 — a spawn-granted endpoint moves on seL4 and copies on x86, so a parent cannot broker a later introduction

**Status:** Resolved 2026-08-08.
**Was:** `distribute_channel_ends` treated a spawn-granted endpoint as a move — reassigning the channel's holder to the child and dropping the parent's slot — where the retired kernel oracle copied it and left the parent's end usable, so a parent could not use an end it had granted at spawn to deliver a capability afterward.
**Exit condition (observed):** Endpoint authority carries `Side`, a spawn grant is a non-consuming narrowing copy, and transit binds to the receiving side rather than a task; `just sel4_call_check` passed 50 markers across ten causal chains, including three parent-vouched post-spawn supervision transfers.
**Evidence:** [`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md)

### B28 — a `retained` second route on one publisher stops a *different* publisher's parked role reply from ever being taken

**Status:** Resolved 2026-08-07.
**Was:** On the P5.4.5 QoS plane, `fabric-publisher` parked once in `recv` awaiting its role reply and never ran again, although both role capabilities were delivered to it.
**Exit condition (observed):** The cause was `MAX_GRAPH_ITERATIONS = 512` — the QoS plane needs more than 512 and fewer than 768 root round-trips — not a lost wake, stale capability, or scheduler defect. Bound raised to 2048; `just sel4_qos_check` passed fourteen markers across nine causal chains, and restoring 512 reproduces the `wedged waiter` signature.
**Evidence:** [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md)

### B12 — the component build's `--remap-path-prefix` names a path that does not exist

**Status:** Resolved 2026-08-07.
**Was:** `components/.cargo/config.toml` passed a hardcoded
`--remap-path-prefix` naming a checkout path that no longer existed, so
the flag was a no-op (later) or actively mangling paths (originally),
while the deferral's central fear — that fixing it would alter every x86
component ELF and the generation identities the oracle's gates assert
against — was never actually tested across ten separate deferral reviews.
**Exit condition (observed):** `components/.cargo/config.toml` now
computes `{ROOT}=.` via `--config` for triple targets, mirroring the
JSON-target branch's `RUSTFLAGS`; the x86 component ELFs were confirmed to
embed zero absolute source paths, so the flag had nothing to remap either
way, and the pre/post generation identities are byte-identical; a genuine
two-checkout build comparison remains unrun and is deferred to whenever
component debug info is enabled.
**Evidence:** [`devlog/2026-08-07-b12-component-remap/`](../devlog/2026-08-07-b12-component-remap/index.md)

### B30 — `release_trust_check` was red, unregistered, and its rotation refusals never reached Rust

**Status:** Resolved 2026-08-07 (B30, second use — distinct from the dango-plane defect above).
**Was:** `release_trust_check` could not run at all (missing imports
crashed it before any assertion), was absent from AGENTS.md's gate index
so a red gate went unnoticed, and its rotation refusals tested a
pure-Python reimplementation rather than the kernel's own decoder.
**Exit condition (observed):** `just release_trust_check` passes, is
listed in AGENTS.md's gate index, and each rotation continuity branch is
guarded by its own fixture routed through `apply_rotation` itself —
removing the replacement check fails with "version-skip", removing the
previous check fails with "stale-previous"; a `replacement.validate()?`
guard was investigated and confirmed unreachable-by-construction (covered
independently by the signature-entry check), so no fixture was shipped for
it and the finding is recorded rather than left as an untested gap.
**Evidence:** [`devlog/2026-08-07-b30-release-trust-gate/`](../devlog/2026-08-07-b30-release-trust-gate/index.md)

### B29 — `ParkedReplies::wake` never deleted the reply CSlot it counted as recycled — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07 (B29, second use — distinct from the block-device defect above). **Note:** no devlog entry exists for this defect; this backlog entry is retained in full as the sole record.

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

**Status:** Resolved 2026-08-07.
**Was:** `build_sel4_generation`'s manifest→flag loop set the selected
manifest's flag and popped every other manifest's flag in the same
iteration, so once two manifests declared the same flag a later table row
could pop what an earlier row set, with the wrong manifest winning
depending on table order.
**Exit condition (observed):** the loop now collects selected manifests'
flags into one set and every declared flag into another, setting the
first and removing only the rest, so a flag two manifests share survives
independent of row order; `just sel4_stream_check` passes with the
`sel4-qos` row present and both flags in effect, and all nine seL4 plane
gates pass with every image rebuilt.
**Evidence:** [`devlog/2026-08-07-p5-4-5-qos-clock/`](../devlog/2026-08-07-p5-4-5-qos-clock/index.md)

### B26 — the `[layout]` dump reported the grant's rights, so a too-permissive layout row was unobservable — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07.
**Was:** `main.rs`'s boot-layout dump printed each row's rights from the
installed capability (filled from the generation grant) rather than from
the boot-layout entry the row exists to freeze, so a layout declaring
strictly more authority than anything used was invisible — the one
direction of disagreement B10's containment check was blind to.
**Exit condition (observed):** `declared_layout_rights` resolves the
layout entry behind a bootstrap row and appends `declared=0x…` only when
it differs from the installed value; a previously-invisible
rights-widening injection now fails the gate, and re-blessing surfaced
three pre-existing, now-recorded containment differences on
`sel4-loan`/`sel4-sample`/`sel4-stream`.
**Evidence:** [`devlog/2026-08-07-b26-layout-declared-rights/`](../devlog/2026-08-07-b26-layout-declared-rights/index.md)

### B24 — `SharedBufferTable::quotas` never reclaimed, so `MAX_CHARGE_HOLDERS` was a lifetime bound — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07.
**Was:** B16's and B22's defect shape in a third table — `quotas` had no
free path anywhere (`declare_quota` only reused a slot for the same
`HolderId`), and because `construct_child` keyed it by task id with
`TaskTable::next_id` never rewinding, a spawn/reap graph presented a fresh
holder every time, so the 96 slots bounded the holders a boot could ever
construct rather than those live at once.
**Exit condition (observed):** `release_quota`, called from
`reclaim_dead_task` after charge settlement, directly releases a quota's
ceiling once nothing can be charged against it again (a quota has exactly
one holder, unlike B16/B22's derived sweeps); `just
sel4_supervision_check` observed 38 holders constructed and 38 releases
with `quotas=0` on the terminal accounting, fault-injected to show
disabling the release leaves `quotas=38`; the original exit condition (a
graph exceeding `MAX_CHARGE_HOLDERS`) was amended as unreachable, since
root CSlot non-reuse caps a boot near 52 tasks first.
**Evidence:** [`devlog/2026-08-07-b24-shared-buffer-quotas/`](../devlog/2026-08-07-b24-shared-buffer-quotas/index.md)

### B23 — `slime-root`'s unit tests were run by no gate — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07.
**Was:** 102 `#[test]` functions across 13 modules were compiled and run
by nothing — no Justfile target named the crate, and it could not have run
anyway since `main.rs` was unconditionally `no_std`/`no_main` with no
`lib` target and no `libtest` for its seL4 JSON target.
**Exit condition (observed):** the mechanism modules were split into a
`slime_root` library the binary links, so all 13 covered modules run on a
host target given `SEL4_PREFIX`; `just test_sel4_root` runs 102 tests
across 13 modules and asserts the count; the first run found three latent
test-bug defects (stale push call sites, a wrong ELF-header fixture
length, a stale qualified-fixture tail) — all test bugs, not production
bugs.
**Evidence:** [`devlog/2026-08-07-b23-slime-root-host-tests/`](../devlog/2026-08-07-b23-slime-root-host-tests/index.md)

### B22 — `ChannelTable` never reclaimed, so `MAX_CHANNELS` was a lifetime bound — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07.
**Was:** B16's exact defect shape in a second table — `channel.rs` derived
keys from a monotonic length and never freed an entry on task death, so
`MAX_CHANNELS` bounded the channels a boot could ever mint rather than
those live at once, and the downstream symptom (a refused mint) read as
broken components rather than one exhausted table.
**Exit condition (observed):** `channel::sweep` frees every entry no live
holder can name, checked against both the live graph and in-flight
`Transit` state (since a transfer drops the sender's entry before parking
it), paired with a monotonic `next_key` so a freed slot's key is never
reissued to a still-live capability; `just sel4_crossing_check` mints 33
pairs against a 32-entry table and still sends/receives on every live
channel including one parked mid-transfer, with three targeted fault
injections confirmed failing.
**Evidence:** [`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md)

### B21 — the toolchain was pinned by name, so each host resolved a different binary — **resolved 2026-08-06**

**Status:** Resolved 2026-08-06.
**Was:** `flake.nix` pinned the seL4 cross toolchain by name and
`build-sel4.py` passed that bare prefix to CMake for PATH resolution, so
`pkgsCross.aarch64-multiplatform.stdenv.cc` — a cross wrapper on
Darwin/x86_64-linux but a native wrapper with an empty `targetPrefix` on
aarch64-linux — silently selected a different, unwrapped compiler driver
and assembler on that host, correcting B20's recorded root cause
(frame-pointer flags, not toolchain identity).
**Exit condition (observed):** `CROSS_COMPILER_PREFIX` now exports an
absolute store path so every host runs the same driver and assembler;
`kernel.elf` rebuilt on aarch64-darwin and aarch64-linux is byte-identical
(unchanged from the recorded pin); `sel4_pin_check` fails if the bare form
returns; B20's frame-pointer flags are kept, since they close a separate,
confirmed-distinct `.debug_line` leak.
**Evidence:** [`devlog/2026-08-06-b21-cross-toolchain-binary-selection/`](../devlog/2026-08-06-b21-cross-toolchain-binary-selection/index.md)

### B16 — a supervision termination record was never reclaimed, so a long-lived graph exhausted the table — **resolved 2026-08-07**

**Status:** Resolved 2026-08-07.
**Was:** `Terminations` recorded how each child ended and never removed
the record (two parents may hold handles to one child), while
`MAX_RECORDS` bounded tasks alive at once even though `TaskId::next_id`
kept counting past reclamation, so a graph that spawned and reaped
repeatedly exhausted the table and every later status query on that child
answered `WouldBlock` forever, silently.
**Exit condition (observed):** `supervision::sweep` derives and reclaims
every record no live holder can name, reading both the live graph and
in-flight `Transit` state (a supervision handle mid-transfer is held by
neither); a residual full-table case now reports rather than silently
drops; `just sel4_supervision_check` observed 35 tasks created against
`MAX_RECORDS=32` with `freed=30 live=3` at the sweep, and two targeted
fault injections confirmed failing.
**Evidence:** [`devlog/2026-08-07-b16-supervision-records/`](../devlog/2026-08-07-b16-supervision-records/index.md)

### B20 — the prefix pin held for one platform at a time — **resolved 2026-08-06**

**Status:** Resolved 2026-08-06. **Note:** the root cause recorded here was later superseded by B21, which found the real mechanism was PATH-order binary selection, not per-platform wrapper policy; the frame-pointer fix below is kept for a separate, real `.debug_line` leak.
**Was:** B19 made `kernel_sha256` independent of the dev shell but not of
the platform — aarch64-darwin and aarch64-linux produced different kernel
hashes from the same checkout, `flake.nix`, and pinned seL4 source,
because Darwin's cross gcc-wrapper forced `-fno-omit-frame-pointer` where
aarch64-linux's native gcc did not.
**Exit condition (observed):** the build states its own frame-pointer
policy (`-fomit-frame-pointer -momit-leaf-frame-pointer`) rather than
inheriting the wrapper's default; `kernel.elf` built on all three tested
platforms (aarch64-darwin, aarch64-linux, x86_64-linux) is byte-identical
with all nine seL4 gates passing, fault-injected symmetrically by
reverting the flag string.
**Evidence:** [`devlog/2026-08-06-b20-cross-platform-kernel-identity/`](../devlog/2026-08-06-b20-cross-platform-kernel-identity/index.md)

### B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain — **resolved 2026-08-06**

**Status:** Resolved 2026-08-06.
**Was:** `sel4/pins.toml`'s `observed_prefix` pinned the dev shell's own
derivation hash rather than the toolchain, because
`configure_and_install_sel4` inherited the ambient environment and
nixpkgs seeds GCC's symbol/section naming from a shell-derivation-hash-
derived random seed, so adding or reordering an unrelated `flake.nix`
package silently changed `kernel.elf` byte-for-byte and was misreported as
toolchain drift.
**Exit condition (observed):** `sel4_build_environment` scrubs every
flag-carrying `NIX_*` variable and search-path variable and replaces the
shell's seed with a fixed one; `sel4_qemu_image_check` passes and adding
an unrelated package to `flake.nix` leaves `kernel_sha256` unchanged; a
second host (aarch64-linux) confirmed the property holds there too, at a
genuinely different hash traced to a real toolchain difference and opened
separately as B20.
**Evidence:** [`devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`](../devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/index.md)

### B18 — the seL4 stream gate was scheduling-dependent — **resolved 2026-08-06**

**Status:** Resolved 2026-08-06.
**Was:** `just sel4_stream_check` passed roughly one run in three, from two
independent causes invisible on x86's cooperative scheduler: a publisher
sending dead-code traffic on an already-finished route that later hit
`ERR_PEER_DEAD`, and `debug_write` issuing one syscall per byte so
unrelated printed lines could interleave mid-string and corrupt gate
markers.
**Exit condition (observed):** the dead-send is deleted, and
`Operation::DebugWrite` is now served by the root's single-threaded graph
loop so a printed line cannot interleave with anything; ten consecutive
`sel4_stream_check` runs pass with all other affected gates unchanged; two
plausible alternative fixes were tried and reverted because each broke a
different invariant, and are recorded as such.
**Evidence:** [`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md)

### B17 — the capability transfer's subset test had no coverage — **resolved 2026-08-05**

**Status:** Resolved 2026-08-05.
**Was:** `serve_cap_transfer`'s subset test (`rights & !source.rights !=
0`) had no fixture proving it live — deleting it left every marker in
P5.5.1's gate intact — because the entry's own analysis of which paths
could produce a narrower-than-kind capability was wrong; it checked only
`cap_transfer`'s outputs and missed that a plain spawn grant produces one
too.
**Exit condition (observed):** `sel4-stream.zti` grants `fabric-publisher`
a second endpoint end at send+transfer that it moves with recv restored,
passing every other transfer rule and computing zero against the per-kind
mask so only the subset test can refuse it, guarded on the component
actually using the granted end first; removing the subset test now fails
`sel4_stream_check`.
**Evidence:** [`devlog/2026-08-05-p5-5-2-stream-plane/`](../devlog/2026-08-05-p5-5-2-stream-plane/index.md)

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four — **resolved 2026-08-05**

**Status:** Resolved 2026-08-05.
**Was:** `slime-root`'s spawn read its grant array through a staged-message
bound of 64 bytes, four 16-byte records, against the retired kernel's
sixty-four — real x86 callers already declared six to nine grants, so a
component that worked on the retired kernel would fail to launch its
children after the cutover.
**Exit condition (observed):** a second staged-array bound (1024 bytes)
separate from the per-message bound lets a grant array be wider than a
message without widening messages themselves; `just sel4_spawn_check`
spawns a component with six grants — B15's own number and the
repository's largest real grant list — with all six ends moving correctly,
fault-injected by restoring the narrow reader.
**Evidence:** [`devlog/2026-08-05-p5-5-1-typed-fabric/`](../devlog/2026-08-05-p5-5-1-typed-fabric/index.md)

### B14 — `slime-root` ignores the generation's declared spawn budget

**Status:** Resolved 2026-08-05.
**Was:** the generation declared a per-component `spawnBudget` that
`serve_spawn` never read, so a component with a declared budget of 1 could
spawn until the global `MAX_TASKS` table filled — the only real limit was
a global size no generation named.
**Exit condition (observed):** `serve_spawn` now derives live-child count
from `TaskTable::live_children` (rather than a separately-tracked counter
that could drift) and refuses a spawn past the declared budget with
`ERR_OUT_OF_MEMORY`, recovering once a child is reclaimed; `just
sel4_sample_check` asserts both the refusal at budget=2 and the recovery
after both children exit, fault-injected on both arms; fixing task
reclamation on both death paths (not only the P5.1 fixture path) was
required to make the recovery arm true.
**Evidence:** [`devlog/2026-08-05-p5-3-4-sample-plane/`](../devlog/2026-08-05-p5-3-4-sample-plane/index.md)

### B13 — `slime-root` admits a shared-buffer allocation without resolving a factory capability

**Status:** Resolved 2026-08-05.
**Was:** `serve_buffer_create` ignored the caller's declared factory slot
and admitted an allocation against the holder's declared quota alone,
inverting the intended relationship (grant authorizes, budget bounds) into
ambient authority arriving through budget alone; the same discarded word
also let every region be created writable regardless of the caller's
request.
**Exit condition (observed):** the `SharedBufferCreate` arm now resolves
the factory slot and requires `RIGHT_BUFFER_CREATE` before admitting
anything, and reads the writable flag from the same word while decoding
it; the generation's `bufferCreate` grants are materialized into holders'
capability tables beside channel ends; `just sel4_loan_check` asserts a
refusal on an empty slot and on a slot holding unrelated authority,
identically, before fault injection showed no existing fixture had
covered the missing check at all.
**Evidence:** [`devlog/2026-08-05-p5-3-3-spawn-plane/`](../devlog/2026-08-05-p5-3-3-spawn-plane/index.md)

### B11 — test scaffolding is declared in the product boot generation

**Status:** Resolved 2026-08-01.
**Was:** the source manifest had one global component graph and health
policy that declared sixteen probes, scenario doubles, and a test-only
storage-writer as peers of product services with real capability grants —
selecting a fabric profile changed interposition only, never removing a
component, its authority, budget, or health edge from the authenticated
generation.
**Exit condition (observed):** a versioned Zutai `BootProfile` resolves
one profile to a closed component/object/grant/state/budget/health/fabric
graph before encoding; `default` is the scaffolding-free product profile,
with `test`/`visibility`/`unified` declaring their verification
participants explicitly; `product_boot_check` boots a healthy 45-slot
product generation naming none of the seventeen test-only components, and
every probe-dependent gate passes on its own profile.
**Evidence:** [`devlog/2026-08-01-b11-product-boot-profiles/`](../devlog/2026-08-01-b11-product-boot-profiles/index.md)

### B10 — init's capability layout is a positional convention, so boot paths are selected at kernel compile time

**Status:** Resolved 2026-08-01.
**Was:** `launch_init` wrote init's capability vector at fixed indices
rather than resolving named grants, so a full capability vector (61 of 64
slots occupied) could not admit a new participant set without squatting on
another profile's slots — the escape hatch was compile-time
`option_env!`/`generation.number` selection, producing a different kernel
binary per gate rather than one artifact that passes the whole suite; this
blocked P1's architecture-neutral AArch64 requirement outright.
**Exit condition (observed):** a `contracts/boot-layout/v1` resource
declares which slot holds which role, name, and rights per generation
number, and `launch_init` offers each minted capability to a placer under
that name rather than writing an index; `boot_layout_check` boots all
eighteen distinct profiles matching every pre-change fixture; one kernel
binary, built with every combination of check flags, now hashes
identically where three distinct binaries existed before; three latent
pre-existing layout defects surfaced and were fixed as part of the change;
52 component-side `option_env!` sites and the recovery-init path were
explicitly scoped out.
**Evidence:** [`devlog/2026-07-31-boot-layout-baseline/`](../devlog/2026-07-31-boot-layout-baseline/index.md), [`devlog/2026-08-01-boot-layout-resolution/`](../devlog/2026-08-01-boot-layout-resolution/index.md)

### B9 — terminated tasks are never reaped, so their frames never return

**Status:** Resolved 2026-07-28.
**Was:** `task::terminate` marked a task Terminated and reclaimed its
shared buffers but never removed it from the scheduler or freed its
address space's user-half page tables, image, or stack frames, so every
spawn permanently consumed its image and stack pages and a repeated
spawn/exit workload drained the frame allocator monotonically — measured
at 13 frames leaked per cycle.
**Exit condition (observed):** `vmm::free_user_half` walks and frees the
user-half page tables before `AddressSpace::drop` releases the PML4, and
`reap_terminated` gives the scheduler a deferred reclamation point run
from `schedule_next`; the boot probe reports frame-conserving spawn/exit
cycles with zero drift under `just dango_check`, `just test` passes 185
assertions including five new reclamation cases, and fault injection
(removing the free call, inverting reclaim/release order) fails both the
live probe and the harness tests.
**Evidence:** [`devlog/2026-07-28-b9-task-frame-reclamation/`](../devlog/2026-07-28-b9-task-frame-reclamation/index.md)

### B8 — budget validation bounded each holder but never the aggregate

**Status:** Resolved 2026-07-26.
**Was:** `SharedBufferBudget::validate_against` checked each holder's
quota against fixed kernel ceilings but never summed holders, so a budget
could promise more holders their full per-holder ceiling than the global
table could ever hold at once, degrading a declared quota into
first-come-first-served.
**Exit condition (observed):** `validate_against` now sums `byte_pages`,
`buffer_count`, `mapping_count`, and `loan_count` with saturating adds and
rejects any total past the kernel ceiling, plus adds the missing
per-holder mapping/loan bounds; `cargo test -p boot-contracts --lib`
passes 24 tests including the new aggregate and per-holder cases, and
raising a real manifest past the aggregate ceiling fails the boot closed.
**Evidence:** [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../devlog/2026-07-26-b7-b8-budget-hygiene/index.md)

### B7 — the `RIGHT_MAP` rename never reached the manifest vocabulary

**Status:** Resolved 2026-07-26.
**Was:** the kernel constant was renamed to `RIGHT_BUFFER_MAP` but the
manifest key stayed the generic `map`, so generation authors kept writing
a generic name for object-specific shared-buffer authority.
**Exit condition (observed):** the builder key is renamed to `bufferMap`
with no wire or identity change (the bit value is unchanged and no
fixture referenced the old key); `just generation_check` produces two
byte-identical builds.
**Evidence:** [`devlog/2026-07-26-b7-b8-budget-hygiene/`](../devlog/2026-07-26-b7-b8-budget-hygiene/index.md)

### B6 — the retained-v2 "still boots" claim was proven only as decode

**Status:** Resolved 2026-07-26.
**Was:** C7.1's exit condition claimed a retained v2 known-good artifact
"still decodes and boots", but no v2 generation was ever booted, and
investigating why found the boot arm is unconstructible from this tree at
all — each generation embeds and boots its own kernel, so a staged v2
manifest would pair with a v3-era kernel, a configuration that has never
existed.
**Exit condition (observed):** the provable, load-bearing part (stage-0
admission) is covered instead — two new boot-contracts tests pin
retained-v2 stage-0 admission and the 32-bit v2 authority-manifest hash
width, the second guarding a real hazard (losing the version branch would
fail every retained v2 release while every gate stayed green); C7.1's
status now claims decode + release authorization + admission and states
why the boot arm cannot be staged.
**Evidence:** [`devlog/2026-07-26-b6-retained-v2-rollback-scope/`](../devlog/2026-07-26-b6-retained-v2-rollback-scope/index.md)

### B5 — no C7 gate exercised the syscall layer or real components

**Status:** Resolved 2026-07-26.
**Was:** no test or component ever reached a real `SYS_SHARED_BUFFER_*`
syscall — the gates called `SharedBufferTable` methods on locally
constructed tables, with "two isolated components" standing in as bare
integer constants and "peer death" as a direct function call.
**Exit condition (observed):** the four missing loan wrappers complete the
nine-syscall surface, and two real components (`sample-lender`,
`sample-receiver`) are added with generation-granted factory/channel/
supervise capabilities; `sel4_sample_plane_live_check` asserts an ordered
transcript moving a two-page payload through real syscalls plus six
denial arms, and a first draft exposed a real ordering property (a lender
exiting before the receiver maps) that is now asserted rather than raced.
**Evidence:** [`devlog/2026-07-26-b5-live-sample-plane/`](../devlog/2026-07-26-b5-live-sample-plane/index.md)

### B4 — the C7 shared-buffer plane was dormant on the live boot path

**Status:** Resolved 2026-07-26.
**Was:** nothing in a running system could allocate a shared buffer — no
generation declared a budget resource, no manifest granted `bufferCreate`,
the kernel never minted a `SharedBufferFactory`, and `slime_rt` had no
syscall wrapper, so C7.3's exit condition held only inside the kernel test
harness.
**Exit condition (observed):** the budget is emitted as a
digest-authenticated `KIND_RESOURCE` object with per-holder quotas and
`bufferCreate` grants declared in the manifest, a transferable
`SharedBufferFactory` minted at boot, the five missing `slime_rt` wrappers
added, and a bounded create/map/write/seal/unmap/release self-check run at
dango and spawn-service startup; a normal boot decodes exactly one budget
object and asserts two distinct non-DENY holder quotas with an absent
component denied.
**Evidence:** [`devlog/2026-07-26-b4-live-shared-buffer-budget/`](../devlog/2026-07-26-b4-live-shared-buffer-budget/index.md)

### B3 — C7.5 wedged every full-graph boot (kernel-stack overflow)

**Status:** Resolved 2026-07-26.
**Was:** every boot launching the full component graph hung after C7.5
instead of draining its ready queue; bisection isolated the change to
C7.5 and raising the QEMU timeout did not help, so it was a real hang
rather than a slow boot.
**Exit condition (observed):** root cause was a kernel-stack overflow, not
the suspected reclamation logic — `SharedBufferTable` grew to 10520 bytes
and was built lazily on first touch inside `task::terminate`'s 32 KiB
guard-page-less stack, corrupting adjacent memory silently; replacing the
`LazyLock` with a const-initialized static (already valid, since
`SharedBufferTable::new()` was already const) removes the stack temporary
entirely, and a compile-time size assertion now guards the class; the
affected gates reach their success lines and exit at the stock 32 KiB
stack.
**Evidence:** [`devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`](../devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/index.md)

### B2 — scheduler has no `Blocked` task state (busy-poll pathology)

**Status:** Resolved 2026-07-24.
**Was:** `TaskState` had only Ready/Running/Terminated, so a task waiting
on input or IPC poll-and-yielded while staying Ready, keeping the ready
queue non-empty and `on_idle` (the only exit path) unreachable — every
non-scripted full-graph boot wedged at the dango prompt, masked only by a
scripted default-Escape input.
**Exit condition (observed):** a `Blocked(BlockReason)` state and a
multi-source, non-blocking-recv-based `SYS_WAIT` syscall let userspace
sweep its sources then wait instead of yield; waiter registration lives on
each wake source with deferred wakes drained under the scheduler lock in a
fixed order to close the lost-wakeup race; a non-scripted gen-1 boot parks
console/dango/spawn-service as idle-blocked consuming no CPU and QEMU
exits Success with no scripted Escape, verified across every wake source's
own gate.
**Evidence:** [`devlog/2026-07-24-boot-check-hangs/`](../devlog/2026-07-24-boot-check-hangs/index.md)

### B1 — `generation_cmd_check` negative scenarios corrupted the wrong generation

**Status:** Resolved 2026-07-24.
**Was:** the fixture builder in `check-generation-commands.py` corrupted a
fixed directory index rather than the candidate generation staging
actually validates, so once a component-image change shifted the
identity-sorted bootstore directory's order, the corruption landed on the
untouched known-good generation and staging succeeded when the gate
expected a rejection — the originally recorded root cause (init aborting
on a rejecting exit) was wrong.
**Exit condition (observed):** the candidate entry is now selected by
`identity != known_good` read from BootState rather than by fixed index;
`just generation_cmd_check` passes success/bad-closure/bad-release with
rejected staging leaving both BootState slots unchanged.
**Evidence:** [`devlog/2026-07-24-generation-cmd-check-wrong-target/`](../devlog/2026-07-24-generation-cmd-check-wrong-target/index.md)
