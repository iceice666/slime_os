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

_Opened 2026-08-17 by a structural audit of the whole tree at `35a95b2`, run
after B56 closed the backlog and the C8 track closed. Every gate was believed
green; the audit looked for architectural debt rather than failing checks. Seven
read-only scouts partitioned the tree by ownership and every load-bearing
measurement was then re-verified directly — three scout claims were rejected on
measurement and are recorded in the devlog rather than opened here. Evidence:
`devlog/2026-08-17-structural-audit/`. B67 was found afterwards, while running
B57's own verification sweep, and had been red before either fix._

_B57 and B58 were the two real defects with wrong observable semantics, and B67 —
found while running B57's verification sweep — was a pair of negative controls
that could not fail. B59, the highest-leverage structural item, is resolved too:
the syscall ABI is now one contract. B66, B60, and B64 followed. All eight are in
the resolved log below. B61, B62, B63, and B65 remain._

### B61 — `just run` boots a test fixture, and the product dispatch path is untestable

**Status:** Open. **Class:** Unmasked architectural debt. **Depends on:** none.

**Problem:** `slime-root/src/main.rs` carries two generations of boot mechanism,
and the *default* build selects the older test-only one. Neither dispatch loop
is reachable from any host test.

**Evidence (2026-08-17).** Traced the default build end to end:
`just run` → `sel4_qemu_image_check` → `build-sel4.py` with no plane flag →
`variant = FIXTURE_VARIANT` (`build-sel4.py:1336`) → `SLIME_ROOT_FIXTURE=1`
(`:835-836`) → `slime-root/build.rs:28-30` sets `cfg(slime_root_fixture)` →
`main.rs:751`'s `#[cfg(not(slime_root_fixture))]` excludes the product
`launch_instance_graph` branch from the image entirely.
`check-sel4-root-boot.py:95-116` confirms the intent: it asserts
`SLIME_ROOT native fixture staged task=0 role=clean-exit` and
`fabric graph=absent`.

The legacy path is a second, independent recv/decode/label-match/reply loop
parallel to the product `serve_instance_graph`. Measured per function:

```
serve                          5337-5400   64 lines
serve_request                  5403-5505  103
serve_fault                    5507-5579   73
stop                           5581-5590   10
report                         5594-5625   32
classify_probe                 5650-5668   19
resume_past_probe              5684-5704   21
setup_shared_region            5713-5745   33
write_pattern_through_scratch  5752-5772   21
read_word_through_scratch      5775-5791   17
report_buffer_phase            5800-5864   65
                                    total 458 lines
```

`slime-root/src/lib.rs` exposes 22 modules to `just test_sel4_root` and states
that `main.rs` "is deliberately not part of this crate's testable surface." So
both dispatch loops, the spawn-grant preflight, the capability-transfer
handlers, the buffer/loan lifecycle handlers, and the healthy/wedge decision —
all of `main.rs`'s 5864 lines — are reachable only through a booted QEMU image.

**Not a dispatch-loop defect.** The product loop itself was checked and is
coherent: `service_for_root_label` (`main.rs:2460-2472`) is a label→service
lookup over `boot_contracts`-derived constants, and the healthy/wedge decision
(`:3213-3265`) counts generation-declared required instances without naming any
plane. The debt is the undead second loop and the missing test seam, not
plane-special-casing.

**Proposed fix:** Move the product dispatch — `serve_instance_graph`, the
service handlers, the spawn preflight, the healthy/wedge accounting — into
`lib.rs` modules so `just test_sel4_root` can exercise them, leaving `main.rs`
as boot sequencing plus the seL4-specific glue that genuinely cannot run on
host. Then decide the fixture path's fate: it has its own gate
(`sel4_root_boot_check`), but being the *default* variant means `just run` does
not boot the product, which should change regardless.

**Exit condition:** `just run` boots a product generation graph, not the
two-fixture proof; the product dispatch loop and at least the spawn-preflight
and healthy/wedge decisions have host unit tests counted by `just
test_sel4_root`; `just sel4_root_boot_check` still passes for the fixture plane
it guards; `just sel4_boot_check`, `just fmt_check_all`, and `just lint_all`
pass.

### B62 — the `.zti` fixtures are copy-paste: three 1882-line files differ by one line

**Status:** Open. **Class:** Unmasked architectural debt (B55's staleness
mechanism). **Depends on:** none.

**Problem:** `contracts/generation/v1/schema.zt` has no `import`, `include`, or
`inherit` construct, so every plane fixture is a full standalone copy that must
be hand-renumbered. There are 30 `sel4-*.zti` fixtures totalling 16978 lines.

**Evidence (2026-08-17).** Pairwise line similarity over all 435 fixture pairs;
nine exceed 85%:

```
99.9%  sel4-fault.zti (1882)      vs sel4-traffic.zti (1882)
99.9%  sel4-saturation.zti (1882) vs sel4-traffic.zti (1882)
99.9%  sel4-fault.zti (1882)      vs sel4-saturation.zti (1882)
99.8%  sel4-matrix-unsatisfiable.zti (1069) vs sel4-matrix.zti (1069)
94.4%  sel4-boot.zti (1709)       vs sel4-traffic.zti (1882)
88.3%  sel4-qos.zti (718)         vs sel4-stream.zti (714)
86.7%  sel4-reclamation.zti (128) vs sel4-supervision.zti (135)
```

`diff sel4-traffic.zti sel4-fault.zti` is one hunk: `generation = 36` versus
`generation = 40`. Two 1882-line files differing by a single integer.
`check-sel4-fault-plane.py`'s own docstring already says the image is
"`sel4-traffic.zti` with `generation` changed and nothing else."

**The correct pattern already exists in the repository:**
`scripts/build/boot_layout.py:188-262` composes `BASE_LAYOUT` with numbered
`OVERRIDE_N` tuples precisely to avoid renumbering by hand. That mechanism is
absent one level up, at the manifest.

**Why this is B55's mechanism:** B55's first defect was a fixture holding 3
supervision rows after the derivation rule moved to 6. A base manifest with a
declared delta cannot hold a stale copy of a table it does not restate.

**Proposed fix:** Add base-plus-delta composition at the `.zti` level, so a
plane fixture declares only its difference from a shared base. The three
1882-line traffic/fault/saturation fixtures should collapse to one base plus
three short overrides, and `sel4-matrix-unsatisfiable` to one override over
`sel4-matrix`.

**Exit condition:** No two `sel4-*.zti` fixtures exceed 90% line similarity; the
traffic/fault/saturation trio is one base plus deltas; every affected plane gate
(`just sel4_traffic_check`, `sel4_fault_check`, `sel4_saturation_check`,
`sel4_matrix_check`, `sel4_boot_check`) passes with byte-identical rebuilt
generations, and `just contracts_check` and `just generation_check` pass.

### B63 — 31 plane gates reimplement a harness that already exists and nobody imports

**Status:** Open. **Class:** Unmasked debt (verification-code duplication).
**Depends on:** B62 makes the expectation-fixture half cheaper.

**Problem:** `scripts/check/` holds 31 `check-sel4-*-plane.py` gates totalling
15192 lines. Two shared libraries exist for exactly their mechanics and are
almost unused.

**Evidence (2026-08-17).** Measured:

| Fact | Count |
|---|---|
| plane gates / total lines | 31 / 15192 |
| gates launching QEMU themselves | 30 |
| plane gates importing `scripts/lib/harness.py` | **1** |
| plane gates calling `harness.run_qemu` | **0** |
| plane gates calling `sel4_gate_markers.match_marker_contract` | **2** |
| definitions of `boot()` | 30, in 23 distinct bodies |

The near-byte-identical helper set — `load_pins`, `profile_text`,
`profile_integer`, `report_transcript`, `sha256_file` — is 1210 lines across the
31 files; `load_pins` and `profile_text` are byte-identical in 25 of the 28
files that define them. `scripts/lib/sel4_gate_markers.py`'s
`match_marker_contract` is consumed only by `check-sel4-qos-plane.py`,
`check-sel4-trace-plane.py`, and the meta-gate.

**Expectations are code, not data.** `check-sel4-boot-plane.py:198-260` holds
`EXPECTED_INIT_CHILDREN`, `EXPECTED_ROLES`, `EXPECTED_ROLE_HOLDERS`, and
`EXPECTED_PROVISIONED_EDGES` as hand-edited Python literals inside a 623-line
executable. The blessable-fixture alternative is already built and in use one
file over: `check-sel4-boot-layout.py` compares
`contracts/boot-layout/v1/fixtures/*.layout` byte for byte and regenerates via
`--bless` (`just sel4_boot_layout_bless`). Markers have no equivalent.

**Marker truth is duplicated once.** `check-sel4-gate-controls.py` correctly
single-sources the regex text through `chains_from_gate`, but its `GATES` tuple
(`:74-142`) hand-pins a per-gate marker count in all 32 rows, whose comments
record a running history of manual updates across B46, B50, C8.13, C8.14, and
B55 — while `marker_count(chains_from_gate(gate))` sits unused in the module it
already imports.

**Proposed fix:** Route every plane gate through `harness.run_qemu` and
`sel4_gate_markers.match_marker_contract`; move the marker/chain and expected-
count tables into blessable fixtures under `contracts/`, mirroring boot-layout;
derive `GATES`'s count instead of pinning it.

**Exit condition:** No plane gate defines its own `boot()` or its own copy of
the five shared helpers; marker and chain expectations live in blessed fixtures
with a `--bless` path; `check-sel4-gate-controls.py` derives every marker count;
`just sel4_gate_control_check` still rejects every mutation class it rejects
today, and the full set of plane gates passes unchanged.

### B65 — 41 of 52 component binaries exist to drive one gate each

**Status:** Open. **Class:** Unmasked debt (per-gate accretion). **Depends on:**
none.

**Problem:** `components/bins/Cargo.toml` declares 52 `[[bin]]` targets. About
11 ship in a real generation (`console`, `dango`, `echo-agent`,
`powerbox-chooser`, `init`, `spawn-service`, `sel4-filesystem-service`,
`sel4-generation-manager`, `sel4-generation-client`, `sysinfo`,
`fabric-service`); the remaining ~41 exist to satisfy one plane gate each. The
call plane alone owns six (`fabric-call-{client,client-b,client-b-restart,server,time,worker}`)
and the operation plane six more.

**Evidence (2026-08-17).** The reuse pattern is already understood and applied
inconsistently: C8.12's matrix plane reused `fabric-publisher`/`fabric-subscriber`
through a `matrix_main()` branch (`components/bins/src/bin/fabric-publisher.rs:118-121`)
rather than adding binaries, while the call and operation planes each added a
full set. Every fixture binary is a `no_std`/`no_main` target carrying its own
link script, boot-layout slot expectations, and Cargo block.

`init.rs` shows the same accretion at the dispatch level: 2295 lines, of which
21 `drive_*_plane` launchers are 874 lines (38%), selected by a hand-written
`match startup_arg` (`:229-395`) that every new plane must edit.

**Proposed fix:** Collapse the call/operation client-server-time-worker families
into parameterized participants selecting their role from the generation's boot
action, as `fabric-publisher` already does; move `drive_*_plane` into
`components/bins/src/planes/` modules, one per plane family, leaving `init.rs`
as the dispatch table plus shared helpers.

**Exit condition:** The call and operation planes are driven by at most three
binaries between them; no `drive_*_plane` body remains in `init.rs`; every plane
gate that used the collapsed binaries passes unchanged, and `just
sel4_boot_layout_check`, `just fmt_check_all`, and `just lint_all` pass.

## Resolved
### B64 — the format-coexistence answer existed in code but was written down nowhere, and one retained schema was unguarded

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt (bears on
roadmap invariant 7). **Depends on:** none.

**Problem as opened:** generation admission is an equality test against one
pinned version, while the rollback and recovery components exist to boot an
*older* selectable generation. The audit judged the two irreconcilable and
additionally reported five dead schema trees.

**Both halves of that were partly wrong, and checking is what showed it.**

*The trees are not dead.* `scripts/check/check-contracts.py` type-checks
`contracts/generation/v2` and `v3` (and runs their `check-invalid-layout.zt`
negative controls, asserting a wire-layout mismatch is rejected),
`contracts/component/v1` at `:125-126`, and `contracts/kernel-image/v1` in the
same sweep. Four of the five are live gate inputs. `check-generation-v5.py`
states the policy the audit missed: the v4 generator "is still on disk because
the format's history is part of the contract"; what must not survive is a
*producer*. Deleting them would have removed real negative controls.

Only `contracts/generation/v4` was genuinely untouched by any gate — the one
retained version that could have stopped parsing with nothing noticing. It is now
in the `check-contracts.py` sweep, and all three retained versions type-check.

*The answer already existed in `slime-root`.* Two mechanisms, neither documented:
`Generation::decode` (`boot-contracts/src/generation.rs:591-600`) refuses a
v2/v3/v4 magic with `UnsupportedVersion` and a foreign one with `BadMagic` — a
deliberate distinction between "an older Slime generation" and "not one". And
`boot_selector::select` (`:144-165`) consumes the pending attempt and commits the
BootState *before* reading or decoding the candidate's bytes, so an undecodable
pending generation exhausts its declared attempts and falls back to known-good
rather than retrying forever or taking the last selectable root with it.

So the repository had already chosen option (b) — format bumps are not
rollback-compatible by *migration*, they are rollback-*safe* by refusal. The
defect was that this was inferable only by reading two files, and no gate
observed it.

**Fix.** `roadmap/README.md`'s invariant 7 now states the rule, names both
mechanisms, and points at the gate. `just sel4_boot_selection_check` gained an arm
that stamps a pending generation's header to the v4 magic and version, gives it
one declared attempt, and requires the root to refuse it (`SLIME_ROOT FATAL boot
selection rejected: Generation`) and the next boot to already be known-good, with
only BootState sectors touched. `boot_refused` was added because the existing
`boot` helper treats any root fatal as a failed run — correct for every arm whose
candidate is supposed to start, wrong for one whose candidate cannot be decoded.

**Why this arm is not redundant with the existing rollback arm.** That one proves
retry exhaustion for a candidate that *runs* and reports itself unhealthy. An
undecodable candidate never runs, so it can never report anything; the protection
has to come from the selector spending the attempt first, which nothing exercised.

**Regression guard, and proof it bites.** Neutralizing the restamp so the pending
generation stays decodable fails the arm: the candidate boots, fails at runtime,
and the transcript shows `SLIME_GRAPH FAIL required instance init exit status=1`
instead of the refusal. Reverted. Neutralizing only the magic still passes,
because the version word alone is sufficient to refuse — worth knowing, and why
the arm stamps both.

**Exit condition (observed):** the format-coexistence rule is documented as an
architectural invariant with its mechanisms named; a pending generation in a
superseded wire format is observed being refused without consuming the known-good
root, on a real boot; no retained schema version is unguarded. `just
sel4_boot_selection_check`, `just contracts_check` (now including
`generation/v4`), and `just ruff` pass.

**Correction to the audit.** The structural audit's B64 entry claimed five dead
schema trees on the evidence of a `grep` for generator references. That evidence
was insufficient: it did not check `check-contracts.py`, which consumes four of
them as gate inputs. Recorded here rather than silently narrowed.

### B60 — authority-derivation policy lived in the builder, and one slot number had two independent sources

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt (B55's
mechanism). **Depends on:** none.

**Problem:** `contracts/generation/v1/schema.zt` declared data *shape* only.
Which grants constitute a control plane, and which component a plane's controls
terminate at, lived in `build-generation.py` functions that no schema
constrained. Separately, one control slot had two independent sources: the
fixture pinned an integer per binding while the broker recomputed it at runtime
as `FABRIC_FIRST_CONTROL_SLOT + position(component)`, and the only thing
asserting they agreed was a comment. B55's root cause was exactly this shape — a
fixture froze 3 supervision rows while the Python rule computed 6, and only a
full boot could detect it.

**Fix, in three parts.**

*The cross-check (preventive).* `_assert_declared_control_slots` refuses a
manifest whose pinned control slots disagree with the order the brokers compile
against, at build time. Two scoping rules were established by measurement rather
than assumed:

- Only the **holder's** binding is compared. An endpoint grant installs both
  ends, so it has two bindings: the client's, numbered in the client's own small
  namespace (slot 0 for its single control), and the holder's, which is the
  indexed table the broker walks. Comparing the client's compares two unrelated
  numberings — the first draft did, and reported `pins slot 0 but call derives 2`.
- Only a holder owning **one** plane is compared. Each plane numbers from
  `FABRIC_FIRST_CONTROL_SLOT` independently, because C8.10's route workers are
  separate tasks with separate tables — slot 2 in the call worker and slot 2 in
  the operation worker name different objects. `valid.zti`'s reference
  `fabric-service` holds stream, call, and operation controls in *one* table, so
  it must lay the planes out consecutively and cannot satisfy the per-plane rule.
  Asserting it against that manifest demanded a contradiction — the same mistake
  B56 found in a gate that swept every profile through a rule only some could
  satisfy. Caught before landing, by running `data_fabric_profile_check`.

*The holder.* `_control_sources` now **reads** the holder from the grants and
checks that every control in one plane terminates at the same component, instead
of selecting it with `"fabric-call-worker" if fabric_profile_name == UNIFIED_…
else "fabric-service"`. The operation plane's two grant families are additionally
checked to land on one holder, since they share one worker's control table.

*The membership.* `FabricProfile` gained `streamControls`, an ordered list the
profile declares. `FABRIC_BOOT_STREAM_CONTROL_GRANTS` and
`FABRIC_MATRIX_STREAM_CONTROL_GRANTS` — two byte-identical Python tuples the
builder chose between by comparing the profile's *name* — are deleted; the six
profile-bearing fixtures declare their own seven-entry plane, and a profile
declaring none keeps the single-broker default so every earlier gate's layout
stays byte-for-byte. Order is authority-bearing (a broker resolves a slot as
`FIRST_CONTROL_SLOT + position`), which is now stated at the schema field.

**Regression guard, and proof it bites.** Perturbing `sel4-boot.zti`'s
`fabric-op-time-control` from slot 5 to 9 and building fails with
`fabric-op-worker's binding for fabric-op-time-control pins slot 9 but the plane
derives 5; the fixture and the broker's FABRIC_FIRST_CONTROL_SLOT + index must
agree`. Reverted. A B55-class divergence now fails the build rather than the boot.

**Exit condition (observed):** a fixture whose pinned control slots disagree with
the derived order fails the build with a named error, observed by perturbing one
slot; no control-plane membership or holder selection is decided by a Python
string comparison; the resolved profile is byte-identical, so this is a pure
refactor. `just contracts_check` (31 manifests), `just generation_check` (two
isolated builds byte-identical), `just data_fabric_profile_check`, `just
sel4_boot_layout_check` (25 plane layouts), `just sel4_boot_check`,
`sel4_matrix_check`, `sel4_traffic_check`, `sel4_fault_check`,
`sel4_stream_check`, `sel4_visibility_check`, `sel4_call_check`,
`sel4_operation_check`, `just ruff`, and `just fmt_check_all` all pass.

**Not closed by this item.** Two derivations named in the audit remain in Python:
supervision-table membership (the ring ∪ proxy ∪ matrix-denied-probe set
comprehension) and notification-slot naming by `removeprefix`/`rpartition` string
surgery. Both are now guarded from the *slot* direction — a divergence between a
derived row and a pinned one fails the build — but neither rule is schema-declared.
They are narrower than the two this item fixed and want their own entry if they
bite again.

### B66 — `ipc.rs` carried two retired-mechanism constants, one of them load-bearing

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (B46 residue).
**Depends on:** none.

**Problem:** `slime-root/src/ipc.rs` declared two constants describing a root
wait-set mechanism B46 deleted. One was dead; the other was live, fed generation
admission, and duplicated a ceiling another contract already declared.

**Evidence (2026-08-17).**

- `CHANNEL_CAPACITY = 1` — "Compatibility value for generation fabric
  admission. Native rendezvous supplies backpressure; the root owns no channel
  queue." Zero call sites repo-wide.
- `MAX_WAIT_SOURCES = 9` — "Compatibility value for generation graph
  admission." Live: `slime-root/src/generation.rs:245` passed it as the
  wait-source ceiling to `FabricGraph::validate_against`.

**Root cause.** The number was already declared, once, in the contract that owns
it: `contracts/fabric-graph/v1/schema.zt`'s `maxIngressSources = 9`, generated
into `boot_contracts::fabric_graph::MAX_INGRESS_SOURCES`. `build-generation.py`
was already importing that generated constant to bound each worker's computed
demand. So `ipc.rs`'s copy was a third spelling of a value the builder and the
contract already agreed on — and it sat in a module whose stated job is the
bounded IPC envelope, which has nothing to do with fabric wake sources.

**Fix.** `CHANNEL_CAPACITY` deleted (landed with B59, which rewrote the same
block). `MAX_WAIT_SOURCES` is now a re-export of
`boot_contracts::fabric_graph::MAX_INGRESS_SOURCES`, so admission reads the one
declaration; the alias is kept because the root's admission call site reads more
clearly in terms of the ceiling it is enforcing.

**Stale naming corrected alongside it.** Nine comments, error messages, and a
schema note described this ceiling as "one `SYS_WAIT` set" — a syscall the seL4
cutover deleted. `SYS_WAIT` no longer exists, so the wording named a mechanism a
reader cannot find, in `build-generation.py`, `check-fabric-manifest.py`,
`check-data-fabric-profile.py`, `contracts/data-fabric-profile/v1/schema.zt`, and
the generated `default_fabric_profile.rs` (fixed at its generator and
regenerated). The builder's refusal now reports the actual numbers:
`worker <name> needs <n> wake sources, above the declared ceiling of <max>`. The
one surviving mention is a deliberate historical note in
`contracts/fabric-graph/v1/schema.zt` recording that the name predates the
cutover.

**Exit condition (observed):** `CHANNEL_CAPACITY` no longer exists; the
wait-source ceiling has exactly one declaration
(`contracts/fabric-graph/v1/schema.zt`, generated to one constant) consumed by
both the builder and root admission. `just test_sel4_root` (118/118), `just
data_fabric_profile_check`, `just generation_check`, `just contracts_check`,
`just sel4_boot_check`, `just ruff`, `just fmt_check_all`, and `just lint_all`
pass.

### B59 — the syscall ABI had no single source: 97 rights declarations, two label tables, three error tables

**Status:** Resolved 2026-08-17. **Class:** Unmasked architectural debt.
**Depends on:** none. **Subsumed:** B57's remaining duplication.

**Problem:** Four number tables crossing the root/userspace process boundary
were hand-authored in one place and manually re-typed in others, with nothing
forcing agreement. `docs/capability-matrix.md` claimed "Rights numbering is
generated-contract truth, not prose"; that sentence was false.

**Evidence (2026-08-17).** Counted mechanically before the fix:

| Table | Names | Declaration sites |
|---|---|---|
| `RIGHT_*` | 23 | **97**, across root, `boot-contracts`, and ~14 userspace components |
| operation labels | 22 | 2 full `mod *_labels` copies plus prose in `docs/syscall-abi.md` |
| `ERR_*` | 5 | 2 code copies plus the same doc |
| spawn-grant record | 1 | `GRANT_RECORD_BYTES` and `SPAWN_GRANT_RECORD_BYTES`, joined by a doc comment reading "Matches ..." |

**This class had already caused a defect.** `slime-root/src/console.rs:292-299`
records a numbering disagreement between the two crates that produced silently
garbled keystrokes — no compile error, only a runtime misdecode.

**Fix.** A new contract, `contracts/syscall-abi/v1`, declares the operation
labels, status codes, message bounds, and the spawn-grant record layout, and
generates `components/proto/src/syscall_abi.rs`. Both crates consume that one
module: `components/runtime/src/syscall.rs` re-exports it, and `slime-root`
imports the same label modules and status codes (`ipc::slime_status` now returns
the generated `ERR_*`). `slime-rt` gained a `slime-proto` dependency to reach it,
which is acyclic — `slime-proto` is `no_std` with no dependencies of its own.

Rights were consolidated onto B57's generated vocabulary rather than duplicated
into the new contract: 69 `u64`/`Rights` declarations became imports from
`boot_contracts::generation`, and the 23 `u32` declarations in the powerbox/fs
components — whose protocols carry a 32-bit rights field — became narrowing
aliases (`RIGHT_X as u32`) over the same generated constants.

The census afterwards: **97 hand-written sites became 1**, and that one is
`RIGHT_BUFFER_ALL = u64::MAX` in `slime-root/src/main.rs`, a "no ceiling"
sentinel rather than a named right.

Two doc claims were corrected instead of left aspirational.
`docs/capability-matrix.md`'s provenance paragraph now describes what is actually
generated and distinguishes the vocabulary (one source) from enforcement
(deliberately several predicates over it). `roadmap/README.md`'s invariant 4 now
states that both couplings are gated rather than trusted.

`components/runtime`'s `SpawnGrant` lost its `#[repr(C)]`: it is an in-memory
type, and the transport encodes it field by field into the generated record
offsets. The attribute suggested its field order was the ABI when the generated
offsets are.

**Regression guards.** `components/proto/tests/syscall_abi.rs` pins all 23
labels, the 6 status codes, the record layout, and the message bounds, and
asserts no two operations share a label and that only `ERR_SUCCESS` is
non-negative. Sharing one module stops *drift*; the freeze test stops a silent
*renumbering*, which would invalidate every component image built against an
earlier generation. The contract's own validator additionally rejects a duplicate
label, a duplicate status code, an operation in an undeclared service, and a
grant record whose fields do not exactly fill it.

`docs/syscall-abi.md`'s operation table is *verified* rather than generated: its
rows carry operand layouts and result conventions the ABI declaration does not
model, so generating it would have deleted real documentation. `just
contracts_check` now fails when the doc omits a declared label or documents one
the contract does not declare.

**Both guards were verified to bite.** Renumbering `EXIT` from 3 to 7 in the
contract and regenerating makes `operation_labels_are_frozen` abort. Changing the
doc's label 32 row to 99 makes the doc check fail with
`syscall-abi.md does not document declared operations: 32 (\`DERIVE\`)`. Both
mutations were reverted.

**Exit condition (observed):** each of the four tables has exactly one
definition, and it is generated; `grep "const RIGHT_"` outside
`boot-contracts/src/generated/` returns only narrowing aliases and the
`u64::MAX` sentinel; the label and error tables exist once. `just
contracts_check` (including the new syscall-ABI check: "documents all 23 declared
operations"), `just generation_check` (two isolated builds byte-identical), `just
test_host` (12 suites), `just test_sel4_root` (118/118), `just
architecture_contract_check`, `just sel4_root_boot_check`, `just sel4_boot_check`
(30 markers, 5 chains, 21 slots, 19 tasks), `just sel4_boot_layout_check`, `just
sel4_gate_control_check`, `just sel4_capability_layout_check`, `just
sel4_dango_check`, `sel4_powerbox_check`, `sel4_filesystem_check`,
`sel4_directory_check`, `sel4_visibility_check`, `sel4_matrix_check`,
`sel4_stream_check`, `sel4_spawn_check`, `just fmt_check_all`, `just lint_all`,
and `just ruff` all pass.

**Partly closed here:** B66's dead `CHANNEL_CAPACITY` was deleted in the same
edit, since it sat in the `ipc.rs` block whose message bounds moved to the
contract. B66's live half — the wait-source ceiling — remains open.

### B67 — two negative controls picked declared slots, so neither could fail

**Status:** Resolved 2026-08-17. **Class:** Gate defect (negative controls that
proved nothing). **Depends on:** none.

**Problem:** `just sel4_capability_layout_check` failed with `the audit accepted a
mutated CSpace: a capability was installed into an undeclared slot
(--cfg slime_b40_mutate_extra)`. The gate exists to prove `audit_child_cspace`
refuses each of B40's six perturbations. Two of the six were not perturbing what
they claimed.

**Evidence (2026-08-17).** Found while running B57's verification sweep, and
reproduced at `beff860` — before B57 and B58 landed — by stashing every working
change, so nothing in that session caused it.

**Root cause, defect 1 (`extra`).** The mutation chose its victim slot by
restating a subset of the predicate it was trying to violate
(`slime-root/src/task.rs:1060-1064`): it excluded `{0, service, fault, tcb}` =
`{0, 1, 2, 3}`, while the audit's declared set (`:1075-1078`) is
`{service 1, tcb 2, fault 3, CHILD_SLOT_CNODE 4, console 32}`. `find` therefore
returned slot **4** — `CHILD_SLOT_CNODE`, which the audit *declares* — so the copy
landed where occupancy was expected and the audit correctly stayed silent. The arm
never created an undeclared occupancy. `console` was missing from the exclusion
list too, latent only because 4 was selected first.

**Root cause, defect 2 (`wrong_slot`), found behind the first.** Fixing `extra`
advanced the gate to the next arm, which then failed differently: not accepted,
but `the mutation was not refused as a CSpace mismatch`. Booting it directly
showed why:

```
SLIME_ROOT FATAL SLIME_GRAPH FAIL instance init construction failed:
  Mint { slot: 4, error: DeleteFirst }
```

`wrong_slot` diverted the fault capability to `fault.wrapping_add(1)` = slot 4 =
`CHILD_SLOT_CNODE`, which is already occupied. The mint failed during
*construction*, so the audit never ran, and the refusal the gate observed came
from the wrong mechanism. Same root cause as defect 1 — slot arithmetic that does
not consult the declared set — reached only once the first was fixed.

**Fix.** `ChildSlots` now owns the predicate. `declares(slot, expect_tcb)` is the
single source for "the plan declares this slot", and `first_undeclared(...)`
returns the lowest slot above null it leaves empty. The audit walk, the `extra`
arm, and the `wrong_slot` arm all go through them, so a mutation cannot name a
declared slot. Slot 0 is excluded from `first_undeclared` deliberately and the
reason is recorded at the definition: the audit does require it empty, but
occupying a child's null slot perturbs the null-capability invariant as well as
the layout, which would leave the refusal ambiguous about which property caught it.

**Regression guard, and proof it bites.** The arms were verified non-vacuous by
weakening the audit and observing the gate fail, rather than by trusting that it
passes:

- Making the audit ignore undeclared occupancy (`if occupied && !declared`) →
  `extra` reported accepted.
- Blinding the audit at exactly the slot both arms target →
  `extra` reported accepted again. `extra` and `wrong_slot` share that victim
  slot, so a single blind spot fails the earlier arm first; both therefore
  demonstrably depend on the check.
- Making the audit ignore a declared slot left empty (`if !occupied && declared`)
  → `missing` reported accepted, confirming that half of the predicate is load
  bearing for its own arm.

Every weakening was reverted and the gate re-run clean.

**Exit condition (observed):** `just sel4_capability_layout_check` passes — "all 6
negative mutations refused", each named individually. `just test_sel4_root`
(118/118), `just sel4_boot_check` (30 markers, 5 chains, 21 slots, 19 tasks),
`just sel4_boot_layout_check` (25 plane layouts), `just sel4_root_boot_check`,
`just fmt_check_all`, and `just lint_all` pass on the same tree.

### B57 — `RIGHT_ALL` had two definitions, and the wider one admitted an undefined rights bit

**Status:** Resolved 2026-08-17. **Class:** Defect (admission accepted what no
contract defined). **Depends on:** none.

**Problem:** The set of valid capability rights was computed two ways. The
builder built it as an enumerated union over the 24 named rights plus
`RIGHT_TRANSFER`; `boot-contracts/src/generation.rs` and
`scripts/check/check-generation.py` built it as a bit-width mask
`(1 << 26) - 1`. The two disagreed by exactly one bit, and the wider spelling
was the one used for admission.

**Evidence (2026-08-17).** Computed both spellings from source:

```
python  RIGHT_TRANSFER | sum(RIGHT.values()) = 0x3fdffff
rust    (1 << 26) - 1                        = 0x3ffffff
bits set in rust but not python: [17]
```

Bit 17 is the hole the builder's table left between `spawn = 1 << 16` and
`supervise = 1 << 18`. It had no name in any table and no use anywhere. Because
admission masked with the bit-width spelling, a grant carrying bit 17 passed
every rights check that existed: the `& !RIGHT_ALL` tests for grants, mappings,
and minted bindings in `boot-contracts/src/generation.rs`, and the oracle's
matching checks in `check-generation.py`.

**Root cause.** A bit-width mask is not the same predicate as a vocabulary. The
rights numbering is not dense — it reserves gaps — so `(1 << 26) - 1` asserts
"below the highest bit" where the intended claim was "one of the rights a
contract names." `docs/capability-matrix.md` already claimed rights numbering was
"generated-contract truth"; nothing was generated, so the two spellings drifted
with no compiler or gate able to notice.

**Scope, measured.** The builder could not emit bit 17: both dynamic rights
lookups reject an unknown right by name, and `validate_capability_rights` masks
per capability kind. So no `.zti` fixture could produce it. The defect was that
*admission* did not enforce what the *builder* did — the asymmetry B40's mutation
series exists to close, for a bit B40 never enumerated.

**Fix.** The rights vocabulary is now declared once, in
`contracts/generation/v5/schema.zt`, as a list of `{name, bit, manifest}`
records. `gen_rust.zt` renders from it: the individual `RIGHT_*` constants, the
`GENERATION_RIGHT_BY_MANIFEST_NAME` table the builder resolves `.zti` grant
spellings through, and `RIGHT_ALL` as a fold over the declared bits rather than a
width mask. Three restatements were deleted in favour of the generated names:
`boot-contracts/src/generation.rs`'s hand-written `RIGHT_TRANSFER`/`RIGHT_EXEC`/
`RIGHT_SPAWN`/`RIGHT_ALL` (its `capability_rights_valid` masks now read
`RIGHT_BUFFER_MAP` rather than `1 << 9`), `build-generation.py`'s 24-entry
`RIGHT` dict, and `check-generation.py`'s four constants.

`RIGHT_ALL` is now `66977791` (`0x3fdffff`) on both sides, and bit 17 is refused
by every validator that masks with it.

**Regression guard.** `right_all_is_a_union_of_named_bits_and_excludes_the_gap_at_17`
in `boot-contracts/src/generation.rs` recomputes the union from the 25 generated
constants, asserts it equals `RIGHT_ALL`, asserts bit 17 is clear, asserts
`RIGHT_ALL != (1 << 26) - 1`, and asserts all nine capability kinds reject a
lone bit 17. The guard was verified to bite: rewriting the generated `RIGHT_ALL`
back to `67108863` makes it fail (`panic = "abort"`, so the harness aborts),
and restoring the generated value makes it pass.

**Exit condition (observed):** `RIGHT_ALL` has one definition, computed as a
union of named rights, consumed by `boot-contracts` and `check-generation.py`
alike; `just test_host` (207 + 20 + 19 …), `just test_sel4_root` (118/118),
`just contracts_check` (31 seL4 manifests encode SLIMEG5 v5), `just
generation_check` (two isolated builds byte-identical), `just
architecture_contract_check`, `just sel4_root_boot_check`, `just sel4_boot_check`
(30 markers, 5 chains, 21 slots, 19 tasks), `just ruff`, `just typos`, `just
fmt_check_all`, and `just lint_all` all pass.

**Not closed by this item.** B59 remains open: the syscall label table, the
error table, and the spawn-grant record are still hand-synchronized, and 97
`RIGHT_*` declaration sites outside `boot-contracts` still restate bits the
schema now owns. B57 fixed the *predicate* that admitted an undefined bit; B59 is
the remaining duplication.

### B58 — `check-architecture-contract.py` hand-copied three generated header offsets

**Status:** Resolved 2026-08-17. **Class:** Unmasked debt (Zutai-rule violation
with a known prior drift). **Depends on:** none.

**Problem:** `object_payload` read the v5 generation header with three literal
byte offsets — `112`, `200`, `368` — under a comment that admitted the coupling
and admitted it had already drifted once: "Generated v5 header offsets. Keep
these in lockstep with `scripts/lib/boot_contracts.py`; notification topology
added two section offsets after minted bindings."

**Evidence (2026-08-17).** All three literals already had generated names in
`scripts/lib/boot_contracts.py`, which is stamped `@generated`, and which the
file already imported from:

```
112 = GENERATION_HEADER_OBJECT_COUNT_OFFSET
200 = GENERATION_HEADER_OBJECT_OFFSET_OFFSET
368 = GENERATION_HEADER_STRING_OFFSET_OFFSET
```

No literal lacked a generated name, so this was a genuine violation rather than
a layout Zutai cannot own. The failure mode it invited is worse than a crash: the
next header field addition shifts these offsets, and the check would read a
wrong-but-decodable location and report a wrong answer.

**Fix.** The three literals now read through the generated names, which were
added to the file's existing explicit `from boot_contracts import (...)` list.
The lockstep comment is gone, because there is no longer a second copy to keep in
step.

**Exit condition (observed):** No numeric header offset remains in
`check-architecture-contract.py`; `just architecture_contract_check` passes
(including its 181 boot-contracts unit tests), and `just ruff` passes.

### B56 — `data_fabric_profile_check` asserted a contradiction and had been red since B55

**Status:** Resolved 2026-08-17. **Class:** Gate defect (a check that could not
pass). **Depends on:** none.

**Problem:** `just data_fabric_profile_check` failed with `fabric graph: invalid
control grant fabric-call-client-control`. C8.9's exit condition was therefore
unobserved, and had been since B55 landed — the failure reproduces at `ea40190`
and at every commit back to `e2f4833`, so no work in this session caused it.

**Evidence (2026-08-17).** Found while running the full C8 regression suite for
C8.15's parent close. Resolving each declared profile of the reference manifest
individually isolated it precisely:

```
default      OK
visibility   OK
unified      FAIL: fabric graph: invalid control grant fabric-call-client-control
```

**Root cause.** B55 (`e2f4833`) gave each plane's control grants a *per-plane
holder*: under the `unified` profile they must terminate at
`fabric-call-worker`/`fabric-op-worker`, because a bounded route worker
authenticates a client by the control endpoint the request arrived on and
`grant_crosses_spawn` forbids handing a worker that endpoint afterwards. Every
other profile declares no worker instance and its controls terminate at
`fabric-service`.

A manifest carries exactly one grant list. So a manifest declaring both kinds of
profile cannot satisfy both rules, and `valid.zti` is exactly that: it declares
`default`, `visibility`, and `unified`, with its nine control grants targeting
`fabric-service`. Retargeting them to the workers was measured and reverted — it
simply moves the failure to `default` and `visibility`.

The defect was therefore in the gate, not the fixture: `check-data-fabric-profile.py`
swept *every* declared profile through `resolve_fabric_profile`, which asks the
reference manifest for single-broker profiles to also resolve a worker-holder
profile. That is a contradiction, not a property. The real full-graph fixtures
(`sel4-boot.zti`, `sel4-traffic.zti`, `sel4-fault.zti`, `sel4-saturation.zti`)
declare `unified` *alone* and target the workers, so they resolve it correctly.

**Fix.** The sweep now resolves the single-broker profiles and states why
`unified` is excluded, with the exclusion guarded: a manifest that declared no
single-broker profile at all fails rather than silently checking nothing.
`unified` keeps stronger coverage than resolution anyway — four gates boot it.

**Exit condition (observed):** `just data_fabric_profile_check` passes. `just
sel4_boot_check`, `sel4_traffic_check`, `sel4_fault_check`,
`sel4_saturation_check`, `sel4_fabric_aggregate_check`, `just contracts_check`,
`just generation_check`, `just ruff`, and `just typos` all pass on the same tree.

### B55 — the full-graph boot plane refused its own first spawn, then five more defects behind it

**Status:** Resolved 2026-08-15. **Class:** Regression of a claimed exit
condition. **Depends on:** none.

**Problem:** `just sel4_boot_check` stopped at
`SLIME_GRAPH spawn refused task=0 slot=2 ungranted`, before any fabric role was
provisioned. C8.10's exit condition — one generation booting every C8 role
simultaneously — was therefore unobserved on the tree the roadmap recorded that
gate as complete against.

**Root cause.** The native-seL4-IPC cutover (`c8fc792`) rewrote `init.rs`'s
full-graph launcher for the new capability model but left it spawning every
child with an empty grant vector (`&[]`), and left `fabric-service` spawning
the two bounded route workers itself — a shape the new model cannot express:
a worker's control endpoints are generation-declared native Endpoints the root
installs before any task runs, so a fabric-spawned worker starts with an empty
control block, and the worker's participant-supervision handles name tasks
only `init` ever holds. Six more defects were latent behind that first one,
each masking the next and none previously exercised because the gate never
advanced far enough to reach them:

1. `sel4-boot.zti`'s minted supervision table still had 3 rows
   (`fabric-service-supervision-{0,1,2}`) after B46 widened the derivation
   rule to one row per ring participant plus declared proxy (6 rows), shifting
   every call/op slot above it by 3.
2. `fabric_boot::receive_role` decoded any message on the control endpoint as a
   capability transfer with no magic check; a QoS `EVENT_MATCHED` record
   interleaved by `refresh_matches` between a two-route subscriber's own
   replies was misread as a denial.
3. `fabric-observer`/`fabric-probe`/`fabric-proxy` selected their boot arm on
   `startup_arg != 0`, but `construct_child` (`slime-root/src/main.rs`) always
   passes a spawned child `0` by design — only the root-launched bootstrap
   instance carries the boot-action argument. The three fell through to their
   standalone-plane logic every boot, undetected because the graph never
   reached them before defect 1 stopped it.
4. `fabric-service::notification_slots` failed hard when a participant's
   (route, direction) pair had no declared ready/credit Notification, even
   though the profile resolver's own comment documents that a boot-mode role
   provisioned without ever driving samples legitimately declares none.
5. `fabric-service::boot_graph`'s provisioning sweep required every registered
   stream client to answer before returning, but the declared interposition
   proxy holds a real control endpoint and, under boot, parks without ever
   contacting the broker — so the sweep never completed and the worker's own
   idle marker never printed.
6. `slime-root`'s dispatch loop declared any run that exhausted
   `MAX_GRAPH_ITERATIONS` with a live task a wedge (`fatal!`), which is correct
   for every other plane's exit-based graph but wrong for this one: its
   declared success state is every required task parked forever, so it always
   eventually exhausts the bound after certifying healthy.
7. `fabric-observer` requested 2 narrowed capabilities per edge;
   `provision_edge` only ever delegates 1 (a v2 ring loan carries data and
   credit in one region), so its second `receive_role` blocked forever.

A structural mismatch in `check-sel4-boot-plane.py` itself compounded these:
`boot()` stopped reading at the first `SLIME_GRAPH healthy` line, but that
record fires the instant every declared task *exists*, in the same dispatch
loop that also services every task's own provisioning IPC — causally *before*
the twenty instances' own request/reply traffic, not after. The gate could
never have observed its own required markers even once every code defect
above was fixed.

**Fix.**
- `drive_boot_plane` (`init.rs`) now spawns all nineteen children itself,
  including both route workers, each with the exact grant vector its fixture
  declares; `fabric-service` no longer spawns anything.
- `sel4-boot.zti`'s call/op control grants target their worker directly
  (`_control_sources` in `scripts/build/build-generation.py` gained a `holder`
  parameter), the minted-binding table now has one row per real handle (14,
  not 6, with the two stale worker-executable mints removed since both
  workers are ordinary grant-bound spawns now), and the missing `nav-backup`
  grant was added.
- `receive_role` checks `CAPABILITY_TRANSFER_MAGIC` and drains anything else.
- The three components read `fabric_boot::active()` instead of `startup_arg`.
- `notification_slots` returns a `NOTIFICATION_ABSENT` sentinel instead of
  failing when a pair is legitimately undeclared.
- `boot_graph` pre-marks the declared proxy answered before the provisioning
  sweep, since it is graph-declared silence rather than a per-request
  discovery.
- The `MAX_GRAPH_ITERATIONS` fatal is now also gated on `!healthy_emitted`.
- `fabric-observer` requests 1 role, matching every other single-route stream
  participant.
- `check-sel4-boot-plane.py`'s `boot()` reads through a quiet settling period
  after the healthy record instead of stopping at it; its `CHAINS`,
  `EXPECTED_INIT_CHILDREN`, `EXPECTED_ROLES`, and `EXPECTED_IDLE_WITHOUT_ROLE`
  now match the restored one-spawning-parent composition, and the five racy
  cross-task stream markers moved to order-independent membership checks
  (`EXPECTED_ROLE_HOLDERS`/`EXPECTED_PROVISIONED_EDGES`) rather than asserting
  one specific scheduling interleaving as a causal chain.
`check-sel4-gate-controls.py`'s boot-plane-specific transcript synthesis and
pinned marker count were updated to match.

**Exit condition (observed):** `just sel4_boot_check` passes — 30 markers
across 5 causal chains, init's 21-slot layout, 19 composition tasks reaching 5
checked roles plus 10 declared role-less idles, none exited, stable across
repeated boots. `just sel4_boot_layout_check` (24 plane layouts, including
the unchanged 21-slot boot layout), `just sel4_gate_control_check` (28 gates
reject 1082 mutations), `just contracts_check`, `just generation_check`,
`just sel4_root_boot_check`, `sel4_stream_check`, `sel4_qos_check`,
`sel4_call_check`, `sel4_operation_check`, `sel4_visibility_check`,
`just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, and
`just typos` all pass with the same tree.


### B53 — dango echoed a line one byte past the message bound

**Status:** Resolved 2026-08-14. **Class:** Defect. **Depends on:** none.

**Problem:** `just sel4_dango_check` runs the first scripted command to
completion and then `dango` exits 1, before the second script line is read. The
plane's claim — a scripted session launching commands through the spawn service —
is half proven: one command demonstrably works end to end, and the session does
not continue.

**Evidence (2026-08-14).** B50's fixture conversion took this plane from *failing
to admit* to running its whole first command, observed directly:

```
dango> $(sysinfo)
resolved:profile
[spawn-service] request
[spawn-service] spawning child
SLIME_GRAPH capability exported task=2 id=1 kind=supervision rights=0x40000
SLIME_GRAPH capability imported task=3 id=1 kind=supervision rights=0x40000
SLIME_GRAPH supervision collected task=3 child=4 kind=0
spawn-request:accepted
result:exit:0
SLIME_GRAPH component exit task=3 status=1
```

So the scripted key source, the command profile, the spawn RPC, the child launch,
and the supervision handoff are all working: `sysinfo` ran and exited 0, and
dango collected its outcome through an imported handle. `console` (task 1) is
still alive at that point and never exits.

**Ruled out.** The script is present and being read — `$(sysinfo)` is echoed
character by character, so `input_script`'s generation-30 entry is found and
`input_read` works. Both ends of `dango-console-rpc` are installed
(`native endpoint task=1 slot=33`, `native endpoint task=3 slot=34`), and
`console`'s binding at declared slot 0 pairs with dango's at slot 1. The console
path itself is *not* the fault: the reprinted `dango> ` prompt reaches serial
after dango exits, so the send that emitted it succeeded and `console` drained
it. Making the spawn RPC transferable changes nothing, because the leg that
would need it — the `with-cwd` line that delegates a derived view — is on a later
script line the run never reaches; that change was measured and reverted rather
than kept.

**Root cause (2026-08-14).** `dango.rs` sized its line buffer
(`MAX_LINE_BYTES = 128`) independently of the transport bound it echoes through
(`MAX_MSG = 64`). The second scripted line is 65 characters, so
`console(&line[..len])` was refused with `ERR_INVALID_ARG` before the kernel saw
it, and the session ended one byte past the bound. A buffer larger than one
message owns the chunking too; `console()` now writes in `MAX_MSG` chunks.

Two further B46 residues sat behind it. `spawn-service` read the working-directory
capability from `received_caps[0]`, which since the cutover carries only native
Endpoint handles — every other kind arrives as an export the receiver claims — so
the `with-cwd` leg was refused as a bad request; it now claims it with
`capability_import`, and `valid_request` checks the declared role against what
actually arrived. And nothing told either service the session was over: the spawn
protocol has carried `REQUEST_FLAG_SHUTDOWN` all along and no one sent it, while
`console` exits on a close message no one sent. A native Endpoint reports no peer
death, so the shell that owns both edges now closes both.

**What made it findable.** Three hypotheses were wrong — the input read, the
script keying, and the spawn RPC's transferability — and each cost a build-and-boot
to disprove. Every exit site in `dango.rs` was a bare `slime_rt::exit(1)`, so a
session that ran its command correctly and then stopped produced a transcript
showing only success. Naming each exit (`[dango] fail: <reason>`, over
`debug_write` rather than `console`, since the console path was the fault) found
the real call on the first run after it landed. The transferability change was
measured and reverted: the leg needing it was on a line the run never reached.

**Exit condition observed.** `just sel4_dango_check` passes: all four scripted
lines run, `$(sysinfo)` and the `with-env`/`with-cwd`/`with-stdin` composition
both complete, `$(inject)` is denied at resolution, `$(echo a b c)` is a parse
error, and the session closes on the scripted escape. `just fmt_check_all`,
`just lint_all`, `just test_sel4_root`, and `just test_host` pass. See
[the devlog entry](../devlog/2026-08-14-b53-b54-last-two-planes/index.md).

### B54 — the stress plane borrowed a component that never ends

**Status:** Resolved 2026-08-14. **Class:** Defect. **Depends on:** none.

**Problem:** `just sel4_stress_check` boots the 23-instance graph, stages every
declared instance, and then stops at `the graph never reclaimed to zero live
tasks`. B49's claim is that the largest admissible graph stays bounded *and*
tears down; the second half is unobserved.

**Evidence:** Failing before B50's fixture conversion and after, but at different
depths. At `9a5f044` it failed on a missing `SLIME_ROOT plan slots required=…
available=…` marker — it never got as far as the budget check. It now reports
`budget: the graph plans 3078 root CSlots of 3222 free` and
`construction: all 23 declared instances were staged`, so admission and
construction are green and the failure is in teardown.

**Root cause (2026-08-14).** All 21 stress instances ran `sample-worker`, which
exists to prove B47's two-threads property: its main thread blocks in
`recv_blocking` on a loopback endpoint and its *worker thread* is what sends.
`sel4-stress.zti` declares neither `extraThreads` nor any binding, so there was no
second thread to send and no endpoint installed — every instance parked on a
receive with no possible sender, and the graph never drained. Observed as 21 ×
`[sample-worker] main thread running` with no `component exit` for any of them.

B49's claim is the *number* of instances the root's CSpace admits, so the
component only has to terminate. The 21 instances now run `supervision-child`,
which exists to run and end. Declaring the extra thread and endpoint on all 21
instead would add 21 TCBs, 21 IPC buffers, and 21 Endpoints to a plane whose whole
point is to sit at the ceiling — changing what the gate measures to keep an
accident.

**Exit condition observed.** `just sel4_stress_check` passes: 23 instances staged
and the graph reclaimed to zero live tasks. `just fmt_check_all` and
`just lint_all` pass. See
[the devlog entry](../devlog/2026-08-14-b53-b54-last-two-planes/index.md).

### B46 — logical ChannelTable, Transit, ParkedReplies, and WaitSet duplicate seL4 IPC

**Status:** Resolved 2026-08-13. **Class:** Unmasked architectural debt.
**Depends on:** B39–B45.

**Problem:** Slime channels are root-owned queues with userspace-managed
blocking, wait sets, reply slots, peer death, and up-to-four-cap transit. Every
message crosses root twice and `slime-root` re-proves atomicity and lifetime
properties already supplied by Endpoints, Reply objects, and Notifications.

**Evidence:** `slime-root/src/channel.rs`, `transit.rs`, and `parked.rs` own the
compatibility mechanism; `Send`, `Recv`, `Wait`, `EndpointCreate`,
`CapTransfer`, and `TransferWindowBind` remain universal root operations.

**Fix:** Cut synchronous RPC to Endpoint `Call`/`ReplyRecv`, rendezvous messages
to Endpoint send/receive, and buffered asynchronous streams to a new
`contracts/fabric-stream/v2/` shared-ring contract with Notification badge bits
for availability and credit. Use real seL4 cap transfer, at most one capability
per IPC message; make bundle provisioning an explicit typed transaction. Delete
the logical channel, transit, parked-reply, and wait-set implementations in the
same cutover.

**Progress (2026-08-10).** Three of the seven named gates pass:
`sel4_channel_check`, `sel4_crossing_check`, and `sel4_visibility_check`, all
three of which were red. The remaining four — stream, QoS, call, operation —
now boot, spawn their whole participant set, and reach their own scenario
logic instead of being refused at admission.

Four classes of defect were found and fixed on the way, none of them the
cutover itself:

- **Undeclared run tokens.** Every fabric plane has init mint control
  endpoints, factories, supervision handles, and phase channels and hand them
  over at spawn, while `mintedBindings` was empty — so the preflight, which
  expects `parent_supplied + minted`, refused before any scenario ran. Same
  omission the probe planes and `sel4-generation` carried.
- **Control slots numbered wrongly.** The declared bindings sat at 0.., over
  the fabric's own `FACTORY_SLOT = 0` and `BUFFER_FACTORY_SLOT = 1`. Shifting
  them by two was necessary but not sufficient: `fabric-service` identifies a
  caller *by the slot the request arrived on*, against `FABRIC_CLIENTS`, which
  is built from explicit tuples rather than sorted. Numbering them
  alphabetically put the intruder in the publisher's slot, and the visibility
  broker answered it as the publisher — handing the unauthorized caller a
  populated route page, the exact leak that plane exists to refuse.
- **Markers with no emitter.** `check-sel4-crossing-plane.py` asserted a
  `kernel=` field the admission marker does not carry;
  `check-sel4-channel-plane.py` asserted four `SLIME_GRAPH channel end` lines
  nothing emitted. The second is emitted now, from the install path, and with
  it visible three more of that gate's assertions proved stale rather than
  merely unreachable.
- **A gate that could not pass.** `check-sel4-visibility-plane.py`'s
  `TERMINAL_MARKER` has no capture groups while `boot` calls `.group(1)` and
  `.group(2)` on it — dead code that only ran once the plane got far enough to
  exit cleanly.

**The channel plane's real defect is fixed.** A declared channel side read as
permanently held, so when its holder exited, `mark_dead` found nothing
abandoned, woke nobody, and the surviving peer blocked forever. Keyed on the
holder's own death — rather than retired at install, which also destroys the
exemption's real purpose of covering the window before an end is placed — both
`sel4_channel_check` and `sel4_component_graph_check` pass. The graph gate's
expectations were the stale half: init holds the consumer end of both declared
channels and exits, so both services observe `PeerDead` and exit 0, which is
what `console.rs` and `spawn-service.rs` are written to do via an
`ERR_PEER_DEAD` arm that was previously unreachable.

**The stream and QoS faults were one bug, and not in the fabric.**
`ActionList` is 147,464 bytes — `MAX_MAPPING_PAGES + MAX_FRAME_ANCHORS * 2`
slots of `Option<AdapterAction>` — built as a local and returned by value, so
a shared-buffer teardown put two copies on the root's 1 MiB stack from an
already-deep dispatch frame. The stream plane's loan teardown overflowed it and
faulted inside `build_actions`. The list lives on the heap now and
`execute_teardown` takes it by value so the return moves a pointer. This is
backlog B3's failure mode a third time in this repository.

Four more assertions on those two gates had never been reached: a `grants=13`
count stale since the stream fixture declared its minted capabilities, prose
spliced into a marker pattern, a `narrowed transfer role cannot widen` line no
component emits, and a per-component failure budget of "exactly one, from the
unconfigured instance" — a P5.2 rule from when the root launched every declared
instance. A v4 generation launches only root-owned autostart ones.

**All seven named gates pass (2026-08-10).** `sel4_channel_check`,
`sel4_crossing_check`, `sel4_stream_check`, `sel4_qos_check`,
`sel4_call_check`, `sel4_operation_check`, and `sel4_visibility_check`, every
one of which was red when this item was opened, alongside nineteen other plane
gates.

The call and operation deadlocks were fixture defects, not broker ones. Both
planes have init mint one control pair per participant at runtime and hand each
side out at spawn, while the fixtures declared those same edges as ordinary
grants — so the root *also* pre-created a channel per edge and installed its
ends at the very slots the minted ones were meant to occupy. Two disjoint
channel sets: the broker consumed one client request off a declared channel,
blocked waiting for a supervision handle init had sent over a minted one, and
init parked waiting for a plane that could not proceed. `minted = true` is
exactly this case and B39 added it; the eighteen bindings on those nine grants
moved to `mintedBindings`.

**The remaining work is the deletion itself**, which is the larger half of this
item: `channel.rs`, `transit.rs`, and `parked.rs` are 1,912 lines with 41 call
sites in `main.rs`, `WaitSet` is threaded through `graph.rs`, `task.rs`,
`supervision.rs`, and `shared_buffer.rs`, and `Send`, `Recv`, `Wait`,
`EndpointCreate`, `CapTransfer`, and `TransferWindowBind` remain universal
labels. Cutting them over means rendezvous messages to Endpoint send/receive,
synchronous RPC to `Call`/`ReplyRecv`, and buffered streams to a new
`contracts/fabric-stream/v2/` ring contract with notification badge bits —
every fabric component rewritten against it. The behavioural gates are green
first, which is the right order: they are what will show the cutover preserved
backpressure, bounded queues, timeouts, peer death, and cap-transfer
attenuation rather than merely compiling.

**Scope measured (2026-08-10).** The root-side figure understates it. The
components are the bulk: 378 call sites across 41 files under
`components/bins/src/` use `slime_rt::send`, `recv`, or `wait`, and every one
is a rendezvous, a synchronous RPC, or a stream read that becomes a different
primitive under the cutover.

Two findings that make the cutover *more* tractable than the item's text
suggests, both worth knowing before it starts:

- **The four-capability message bound is nearly free to give up.** seL4 carries
  one capability per IPC. Exactly one call site in the tree reads a second —
  `directory-probe.rs:153`, which treats `caps[1]` as an optional derived
  handle. Everything else already sends at most one, so "at most one capability
  per IPC message" costs a single component change rather than a transit
  redesign.
- **`WaitSet` is already gone.** It was deleted with the readiness cluster
  earlier in this run. What survives is vocabulary: `IpcError::WaitSetFull` and
  `WaiterConflict` now name ordinary table-full and double-insert conditions in
  `graph.rs` and `transfer_window.rs`. They cannot be renamed yet because
  `parked.rs` still raises both, and `parked.rs` has 78 references in
  `main.rs`.

That last point is the shape of the whole item: nothing in it is separable.
`channel.rs` also owns `LaunchedInstances`, which B51 made load-bearing for
respawn provenance and which has nothing to do with logical channels. The
deletion has to move that out, cut over every component, and land the v2 ring
contract in one change, because the labels, the tables, and the components all
reference each other. There is no partial state where the tree builds and the
gates mean anything.

**Two separable pieces landed (2026-08-10), and they were the only two.**

`LaunchedInstances` moved out of `channel.rs` into `launched.rs`. It was never
a channel concern — it answers "which declaration is this task" and "has this
declaration ever run", both of which outlive any IPC model — and leaving it
there would have meant rescuing it mid-deletion. Two of its couplings were
wrong on their own terms: it was sized by `MAX_CHANNELS`, which bounds
something unrelated, and `record` returned `ChannelError`, so a full instance
table reported `UnlaidSlot`. `channel.rs` no longer imports `MAX_INSTANCES`,
which is the evidence the move decoupled rather than relocated.

`contracts/fabric-stream/v2/` exists: schema, renderer, generator, generated
bindings, and a `contracts_check` guard that refuses a layout whose field order
disagrees with its schema. Both records are exactly 64 bytes. Three design
points are settled and recorded there — slots carry a `claimed` state between
empty and ready so a torn write is unobservable; sequences are absolute rather
than slot indices so a lagging subscriber counts drops instead of mistaking a
wrapped slot for a new sample; and `producer_state` lives in the ring so peer
death needs no root round trip. The badge carries only "something changed",
because a notification word coalesces and nothing that must not coalesce can
travel on it.

No component reads the ring yet, but the reader's side of the contract is
enforced: `valid_ring_header`, `valid_ring_slot`, `ring_slot_index`, and
`valid_ring_badge` in `components/proto/src/lib.rs`, with 20 tests. The
generated bindings are only a codec, and a ring is memory a peer writes — a
publisher with a stale mapping produces bytes, not an error. Four checks matter
and each is proven load-bearing by reverting it:

- the slot bound comes from provisioning, never from the header, so an inflated
  `slot_count` cannot walk a reader off its own mapping;
- `head - tail` is refused when inverted rather than subtracted, since unsigned
  subtraction yields a huge count the reader would try to consume;
- only `SLOT_READY` is readable, which is what makes a torn write unobservable
  rather than merely unlikely;
- a slot whose sequence is not the expected one is a wrap the reader fell
  behind on — structurally perfect, carrying a real sample from `slot_count`
  ago.

**The ring's own machinery is complete** — `components/proto/src/ring.rs`,
19 tests. `publish` refuses at capacity rather than overwriting, which is
backpressure with the unread samples still readable; `consume` returns the
sample at `tail + 1` or nothing; peer death is a header field, so a subscriber
learns a producer died without a root round trip, and marking a death never
relabels a clean end.

Two pieces were removed for being unearned. The `Lost` variant was
unreachable: one reader owns `tail` and `publish` refuses at capacity, so the
slot at `tail + 1` is the awaited sample or unwritten, and anything else is a
mapping the reader should refuse. BEST_EFFORT drops need a publisher that
overwrites, which is a policy above this cursor. The separate claim step was
redundant too — `WireRingSlot::encode` writes the whole slot and `head` is what
makes it visible, so removing it and re-running the suite proved nothing
depended on it. `SLOT_CLAIMED` stays in the contract because `valid_ring_slot`
must keep refusing it.

**Both replacement kernel objects now exist** (`slime-root/src/peer_endpoint.rs`
and `notification.rs`, 8 tests).

The multi-source wait turned out to be the real obstacle, not the message
transport. `fabric-service::park_on_controls` parks across up to nine endpoints
at once and seL4 receives on exactly one, so `wait` has no Endpoint equivalent
at all — the kernel object for "several sources, one waiter" is a Notification
with badge bits. Nothing in the tree could hold one: notifications existed only
for IRQs, and `graph::Resource` had no variant, so a child could not be given
one. It has both now, plus `RIGHT_NOTIFY`, kept separate from `RIGHT_RECV`
because observing a signal is not permission to consume the message that caused
it.

The bit is authority rather than convention: each source gets a capability
minted with its bit as the badge and write rights only, so a holder sets
exactly that bit and cannot consume the wake. Bits are never reused within a
notification's life, because a signal already in flight from a released source
would otherwise arrive as the new one.

`peer_endpoint.rs` mints one capability per side with the rights that side
declares, so the *kernel* enforces direction on every invocation instead of the
root re-checking a rights word per message. `grant` on the producer and
`grant_reply` on the consumer ride along: without them an endpoint silently
cannot carry the one capability an IPC message may hold, or answer a `Call`.

**The endpoints are created at runtime**, one per declared channel, paired
with the channel each replaces — observed on the channel plane as
`SLIME_GRAPH peer endpoints created=2 channels=2`. Paired rather than swapped
because the cutover cannot land in one commit; `for_channel` makes a
component's migration a lookup instead of a second materialization pass.

The object comes from the root's global pool while the minted capabilities are
arena-owned, which is the lifetime the model already has: a declared channel
outlives both peers — that is what lets a service whose launcher exited keep
serving — but a capability belongs to its holder's CSpace. `sel4_stress_check`
still passes, so 48 more endpoints at the 23-instance ceiling fit inside the
budget B49 admitted.

**What remains is one indivisible change.** `slime_rt::wait` has 115 call
sites, `recv` 104, and `send` 93, across 41 component files. Each becomes a
different primitive: rendezvous to Endpoint send/receive, synchronous RPC to
`Call`/`ReplyRecv`, buffered streams to the ring above. The root side deletes
`channel.rs`, `transit.rs`, and `parked.rs` — 1,844 lines after
`LaunchedInstances` moved out — along with six universal labels, and `parked.rs`
alone has 78 references in `main.rs`.

There is no intermediate state where the tree builds and the gates mean
anything: a component half-migrated has neither a logical channel nor an
endpoint for the operation it is mid-call on, and the labels cannot be removed
while any caller uses them. The behavioural gates are green first, which is the
right order — they are what will show the cutover preserved backpressure,
bounded queues, timeouts, peer death, and cap-transfer attenuation rather than
merely compiling.

**The deletion landed (2026-08-12), and the gates it was ordered before have
not all come back.** `channel.rs`, `transit.rs`, and `parked.rs` are gone,
along with `Send`, `Recv`, `Wait`, `EndpointCreate`, `CapTransfer`, and
`TransferWindowBind`; every fabric component is rewritten against the v2 ring.
`just sel4_channel_check` and `just sel4_crossing_check` pass on native
Endpoint paths, and `just test_sel4_root` reports 118/118 with `lint_all` and
`fmt_check_all` clean. The other five named gates do not pass yet, so this item
stays open by its own exit condition.

Four defects in the cutover were found by running it, each of which made the
delegated-ring design unrunnable and none of which was the ring:

- **The export id was written over the descriptor's `status`.** Bytes 8..12 of
  a 64-byte `CapabilityTransfer` are the field every receiver reads to tell a
  grant from a denial, so a successful delegation arrived looking refused. The
  descriptor has no spare field, so the id stays off the wire entirely and a
  receiver claims the oldest finalized export addressed to it.
- **A logical capability could not be exported at all.** `serve_capability_export`
  required a kernel endpoint for every export, so shared buffers, loans,
  supervision handles, and directories were refused before any policy ran. The
  ticket is optional now; a logical kind crosses as a table entry.
- **A delegated buffer handle is unmappable by design.** `authorize` requires
  `region.owner == holder`, so a peer handed a handle is refused when it maps.
  A ring crosses as a *loan*, which is the primitive for exactly this — and
  loans were read-only, so they now record their own writability: a v2 ring has
  two peers advancing disjoint header fields of one unsealed region, while a
  C7.6 sample loan stays read-only over a sealed one.
- **An unsatisfiable spawn ordering.** The fabric loans a ring to each
  participant, needing their supervision handles first, while
  `fabric-publisher-b` loans its large sample back to the fabric, needing the
  fabric's. A loan may now name its receiver through a declared endpoint as
  well as a supervision handle; both are capabilities the generation fixed
  before either task ran, so the receiver is still not an ambient task id.

Two further defects were outside the fabric. A non-blocking receive that found
nothing was identified by requiring a zero message label alongside zero words
and zero capabilities, but seL4 leaves MR0 undisturbed when nothing arrives —
so a stale label from the thread's previous message made an empty poll fail the
shape check as a malformed 573-byte payload. And `FrameAliases::remove` dropped
the alias record without emptying its CSlot while the slot pool hands released
indices back for reuse, leaving a live capability where the allocator believed
there was none.

**Hand-numbered slots are the remaining defect class, and the count says so.**
Every failure above the ring was a declared slot disagreeing with a hardcoded
one: a probe endpoint on a minted factory's slot, supervision handles at 7/8
against a profile numbering four, probes displaced twice, and a component
`RING_SLOTS` constant against the depth the fabric actually formats. There are
four distinct slot namespaces — fixture `bindings[]`/`mintedBindings[]`, the
derived child CSpace regions, component-side constants, and root CSlots — and
only the last two are machine-assigned. A structural hazard makes the first
worse: endpoint and logical bindings share one declared-slot number space that
the decoder refuses duplicates in, yet map to disjoint CSpace regions at
runtime, so slot 1 as an endpoint and slot 1 as a factory collide at build time
for no runtime reason. Auto-allocating the declared namespace and resolving
component slots by role is tracked under B50, which already requires removing
fixed-slot constants.

**That blocker is root-caused and fixed (2026-08-12).** `fabric-subscriber`'s
ring loan was refused `CNode Copy: Destination not empty` at slot 1501, an index
the pool reported as freshly issued. A full sweep of the pool's 3,141-slot range
against real kernel occupancy found exactly one divergence — slot 1501, occupied
while the bitmap called it free, and already so at the *first* alias reserve, so
nothing was reused and the pool span was not overstated. The culprit is
`release_task_arena`: it revoked the arena's parent untyped and returned every
charged CSlot to the bitmap, which is only correct for slots holding objects
retyped *from that untyped*. This cutover added a second kind — `reserve_slot_in`
charges a bare CSlot to an arena while `peer_endpoint`/`notification`
`install_instance` mint into it from a **globally** allocated Endpoint or
Notification. No revoke of the arena parent reaches such a capability, so
`fabric-intruder`'s teardown handed back slot 1501 still occupied. Deleting each
recorded slot before releasing it restores `release_slot`'s documented
precondition. `sel4_stream_check` now maps that loan and reaches its own scenario
logic. See `devlog/2026-08-12-b46-arena-slot-occupancy/`.

**`just sel4_visibility_check` passes (2026-08-12).** Three of the seven named
gates are green now — channel, crossing, and visibility — and the third took
four defects, the last of them architectural.

Two were declarations. `sel4-visibility.zti` declared all 28 endpoint edges as
`mintedBindings` while init, post-cutover, holds no route capability and mints
nothing; they are ordinary `bindings` now, matching `sel4-stream.zti`, whose
`mintedBindings` carries only what init genuinely creates at runtime. And the
broker's route slots, hardcoded at 7..13, collided with the supervision handles
the resolved profile places at `FIRST_CONTROL_SLOT + len(clients)`. The broker
derives them now, which is R2's rule applied where the collision actually bit.

The third was the five participants, the cutover's unconverted half. The
pre-cutover broker moved a real capability per role with `cap_transfer`; the
post-cutover one sends the descriptor alone, because each edge is a generation
fact installed before any task runs. The components still called
`capability_import()` on a reply carrying nothing. They take the declared
endpoint now.

**The fourth is the one worth keeping.** The broker detected the proxy's exit by
waiting for `ERR_PEER_DEAD` on its control endpoint, and *that signal does not
exist on a native Endpoint*: it is a logical-channel concept, and
`sel4_transport::receive_native` cannot produce it, so a dead peer is
indistinguishable from a silent one. This is a real consequence of the cutover
and it will recur wherever a component waits on a peer that can exit. The answer
is the one the model already has — a **supervision handle**, which is how
`spawn-service` and `init::wait_clean` have always observed termination. The
builder grants one for every declared interposition proxy now, not only ring
holders, on the same reasoning the B46 comment already gives for publishers;
init spawns the proxy before the fabric so the handle can exist; and the
broker's waits go through one `await_exit`. Endpoints carry messages;
supervision capabilities carry death. No new mechanism was needed.

Two of the gate's own assertions were stale rather than unreachable, both from
markers this cutover renamed: `SPAWN_PATTERN` still matched `channels=` where
the root now emits `endpoints=` and `notifications=`, so it silently never
matched and the gate could not resolve init's task id to see its clean exit; and
`grants=13` predates the fixture declaring its route edges as grants. The same
stale `channels=` pattern is present in eight other check scripts and will need
the same correction as those planes are brought up.

**A native `send` always blocks, and the fabric was written as though it might
not (2026-08-12).** `sel4_transport::send` invokes `seL4_Send` and returns
`ERR_SUCCESS` unconditionally, so it can never answer `ERR_WOULDBLOCK` — and
every non-fatal `ERR_WOULDBLOCK` arm in `fabric-service` was unreachable code
resting on a guarantee the transport does not give. The stream broker deadlocked
on it three times over: pushing a QoS event to a subscriber that had moved on to
reading its ring, announcing `STREAM_END` to one that had already exited, and
finally on the reverse hazard — exiting first and reclaiming its shared-buffer
charges out from under two participants still executing against the loan
mappings, which faulted both on execute-at-null.

`slime_rt::try_send` is the missing primitive: `seL4_NBSend`, which discards
rather than blocks. It is best-effort by construction — the kernel reports
nothing either way — so it is correct only for advisory traffic. QoS events take
it outright; `STREAM_END` is re-offered every broker pass and the route retires
when the peer takes it *or* is gone, because a terminal event a subscriber is
genuinely waiting for cannot be dropped once. The teardown ordering is the same
supervision-handle answer peer death needed.

Two more shape errors surfaced behind those. A delegated capability is a
**root-recorded export claimed with `capability_import`**, not an in-message
capability: only a native Endpoint travels inline, so every `received[0]` read
for a loan was reading zero — the broker, both subscribers. And
`fabric-subscriber-b` multiplexes two routes over one control endpoint with two
sequential readers, so a destructive receive let whichever loop was running
consume the other route's terminal event. That last one first looked like a
design flaw needing either separate endpoints per route or a fixture change;
the wire settled it instead. Every record on that endpoint — stream event, QoS
event, sample descriptor — already carries `type_identity`, and the broker
already stamps each with its route's tag, so one reader owns the endpoint and
files each record under the route it names. Nothing in any contract, fixture,
or slot numbering changed.

**Two non-blocking peers never rendezvous (2026-08-13).** Demultiplexing alone
did not close the plane. `seL4_NBSend` delivers *only* to a receiver already
blocked on the endpoint and discards otherwise, while both subscribers polled
with a non-blocking `recv` and slept on a ring notification — so the fabric
re-offered terminal events forever to a reader that was never once visible to
the send. Each loop now blocks on the control endpoint once its ring is drained,
which is precisely when it has nothing else to wait on. This is the hazard
`try_send` carries by construction: it is correct only for traffic a peer is
genuinely waiting on, and any future use must pair it with a blocked receiver
or a supervision-handle fallback.

Two regressions the cutover had silently dropped also had to be restored before
the gate would pass honestly. `fabric-subscriber` no longer asserted *either* of
its authority denials; probing by asking the fabric for the publish side proves
nothing, because the request's `direction` is read only to be discarded and each
client is answered exactly once, so the role descriptor's declared direction and
its send-free rights mask are checked instead. And C8.5's `EVENT_PEER_DEAD`
existed in the contract with nothing emitting it — a publisher that exits
without `FLAG_LAST` leaves no trace on a native Endpoint — so it is now derived
from the publisher's supervision handle, the same answer the visibility plane's
peer-death gap needed.

`just sel4_stream_check` **passes**: `57 markers observed across 14 causal
chains`, with `QoS peer dead` observed and terminal accounting clean. Five of
its stale assertions were corrected: `grants=9` predates the cutover's
`grants=5 endpoints=7 notifications=12` split, three chains ordered participant
markers against broker lines that race them, and the root emits capability
accounting before loan accounting.

**The last three gates are blocked on a fixture shape, not on slot allocation
(2026-08-13).** `sel4_qos_check`, `sel4_call_check`, and `sel4_operation_check`
all still fail at `spawn refused … ungranted`. With B50/R2's clause (3) landed,
the count init offers is now the manifest's own, and the refusal is no longer
init guessing: those three fixtures genuinely declare capabilities as
`mintedBindings` that init must *create* and hand over, and nothing in
`drive_stream_plane` creates them. `sel4-qos.zti` asks for three
`fabric-publisher-probe-*` carriers where the working `sel4-stream.zti` declares
one ordinary `fabric-publisher-probe` **grant** the root materializes;
`sel4-call.zti` and `sel4-operation.zti` go further and still declare every
control endpoint as minted, which is the pre-cutover shape B46 replaced
everywhere else.

`sel4-qos.zti` is now converted, and it admits, boots, and runs its whole graph:
the three probe carriers became one ordinary `fabric-publisher-probe` grant, the
clock became a `fabric-publisher-b-clock` grant between the publisher and the
broker, and a `fabric-publisher-b-fabric-supervision` entry was deleted as dead
— `fabric-publisher-b` names the fabric by its *control endpoint* for the
upstream loan and holds no supervision handle, which its own source says. The
plane also needed the four ring-holder supervision handles B46 requires (it
declared only the two subscribers), quotas for `fabric-publisher` (it had none,
so its ring mapping was refused) and a wider one for `fabric-publisher-b`, and
the removal of `priority = 100` on `fabric-intruder`: under the cutover's
blocking IPC a low-priority task that must speak before the broker can proceed
simply starves. `just sel4_qos_check` went from 40 component markers to 79.

Three real defects in `fabric-service` surfaced behind that, all of them
cutover-era and none QoS-specific:

- **`in_flight` was never incremented.** Its own doc calls it "samples sent but
  not yet acked", but only decrements existed, so it could not leave zero — and
  every rule reading it (reliable retry accounting, retry exhaustion, and
  holding the terminal event until the queue drains) was unreachable code rather
  than an unmet condition. `deliver` now counts a RELIABLE delivery out and
  `drain_acks` clears the balance on the subscriber's credit signal, which is a
  level rather than a tally.
- **Retry exhaustion reported nothing when the queue was already drained.** An
  earlier lifespan expiry empties the history, and the emission was gated on a
  surviving frame — so the condition was invisible exactly when it was reached
  the hard way. It is a statement about the retries, and now reports as one.
- **QoS events were sent with `try_send` and dropped.** `seL4_NBSend` delivers
  only to a peer already blocked on the endpoint, and these are not advisory:
  the plane's contract is that the subscriber observes each declared condition.
  Undelivered records are retained and re-offered each broker pass, and the
  terminal event is held back while any is outstanding, since both race into the
  same endpoint and the end would otherwise retire the route first.

**`just sel4_qos_check` passes (2026-08-13).** Five of the seven named gates are
green: channel, crossing, stream, QoS, and visibility.

The remaining assertion was not the mailbox. `send_qos_event` used `try_send`,
and `seL4_NBSend` reports nothing either way — so the runtime answers
`ERR_SUCCESS` for "attempted" and the `ERR_WOULDBLOCK` arm that retained a
record for the next broker pass **could never run**. `flush_qos_events`,
`retain_qos_event`, and `qos_events_pending` were dead code standing in for
delivery that had already been dropped: the plane printed `QoS deadline missed`
with nothing arriving at the subscriber. A declared QoS condition is an
obligation, so it takes a blocking `send` — what the `EVENT_SAMPLE_LOST` path on
that same endpoint already used — guarded by the peer's supervision handle,
because a blocking send to a terminated task can never rendezvous and a native
Endpoint reports no peer death. The retain machinery is deleted with it.

The clock input needed the same answer: `time_dead` was set only by
`ERR_PEER_DEAD`, which no native Endpoint produces, so the broker waited forever
for a clock that had exited. It is derived from the clock peer's supervision
handle now. **This is the third distinct place peer death has needed a
supervision handle since the cutover** — publishers, proxies, and now the clock.
Endpoints carry messages; supervision carries death.

**`sel4-call.zti` is converted, and the plane runs most of its scenario.** The
four control edges and two phase barriers are ordinary grants; `mintedBindings`
carries only what init genuinely creates — the factories it holds and the three
supervision handles that cannot exist before their tasks — so init spawns the
participants first and hands the broker its declared set. A participant cannot
hold a supervision handle naming itself, so the scenario names the fabric as its
loan receiver through the declared control endpoint, which `serve_buffer_loan`
already accepts and which breaks the same spawn-ordering cycle the stream plane
hit.

Four defects behind it, none fixture-side, and all four are the same class:
**code written against `ERR_WOULDBLOCK` semantics that native IPC never
produces.**

- **Nine components parked on the wrong discriminator.** Every call and
  operation participant parked when `startup_arg == 0`, meaning "the boot plane
  gives me no work" — but the root delivers a nonzero boot action *only* to the
  bootstrap instance, so all nine parked on their own planes too.
  `fabric_boot::active()` is the discriminator the stream components already use
  and says what is meant.
- **A blocking forward deadlocked the broker.** Forwarding a second request
  while the server was still blocked sending its first reply left both waiting
  on each other. The server answers one call at a time, so `server_idle` now
  tracks reachability and a deferred forward waits in `Phase::Forwarding` — the
  retry path that phase already existed for. Cancellation is staged the same
  way, since the server is executing the very call being cancelled.
- **Two polling peers never rendezvous.** `recv_call` and the server loop polled
  with `yield_now` while their peer blocked in `send`. Both block now: each has
  nothing else to wait on, which is exactly when blocking is correct.
- **A delegated loan was read out of the message.** Only a native Endpoint
  travels inline, so every `caps[0]` read for a loan was reading zero — both
  broker paths and both scenario paths. It is claimed with `capability_import`.

One runtime defect surfaced alongside them: `receive_native`'s capability path
returned through `?` with `RECEIVE_SLOT_LIVE` still set, so every later receive
on that thread would answer `ERR_WOULDBLOCK` forever — a wedged caller reading
as a silent peer.

The stall was the broker never blocking. `seL4_NBRecv` takes a message only from
a sender *already blocked* on the endpoint, so a polling broker and a blocking
server never rendezvous however long either spins — yielding changes neither
side's state. When nothing has progressed and a call is outstanding, the
server's answer is the only event that can move the plane, so the broker now
waits for it in the kernel; everything else stays polled, because a multiplexer
must not block on a peer that may be blocked on it. The phase barriers had the
same shape and the same fix.

The plane now runs correlated replies, rejection, duplicate suppression, shared
payloads in both directions, cancellation, stale-session refusal, and
malformed-reply detection.

**What remains is one identified blocker, and it needs a Notification.**
Instrumenting both sides of the client handshake showed it completing —
`signal_client_b` returns, so client B received it — and the plane then stalls
in B's 24-request backpressure burst. The broker waits on the server when
nothing else has progressed, which is what unblocked the reply path, but that
wait is not sufficient in general: a client that blocks in `send` *after* the
broker's non-blocking sweep has passed it stays invisible until the next sweep,
and a broker already parked on the server never runs one. B's burst lands in
exactly that window.

A single Endpoint cannot express "wake me when any of these speak", which is
the whole reason `graph::Resource` gained a Notification variant during this
cutover and what `fabric-service`'s stream side already uses. The call and
operation brokers need the same treatment: every peer badged into one
Notification the broker waits on. That is a design change rather than another
blocking-semantics fix, and it is the honest remaining scope of these two gates.

**A second constraint, found by attempting the fix.** The exact deadlock is
`reject_terminal` → `pump_terminal` → a blocking `send` to a client that is
blocked sending its *next* request: both peers on one endpoint, in opposite
directions. `MAX_CALLS` is 4 and client B fires 24, so the refusal path is
reached by design — it is the arm the plane exists to prove.

Making that send non-blocking does not work either, and the reason is
structural: `seL4_NBSend` reports nothing, so the broker cannot tell delivery
from a drop and must never retire a terminal on its own word — the same defect
that made every QoS event vanish. Re-offering until taken needs a queue with no
bound that the plane respects, since a terminal may only be retired when its
client actually receives it.

So the two brokers need *both* halves: a Notification to learn which peer is
ready, and a receiver-confirmed retirement for terminals — a reply, an ack, or
a `Call`/`ReplyRecv` pair, which is what B46's own text prescribes for
synchronous RPC and what these two planes never adopted. The stream plane avoids
the question because its terminal events are re-offered against a ring the
subscriber drains, giving an independent liveness signal these planes lack.

**`Call`/`ReplyRecv` is genuinely missing from the runtime**, which is a real
gap against this item's own stated fix: `slime_rt` has `send`, `try_send`,
`recv`, and `recv_blocking`, and nothing that issues `seL4_Call`. It was written
(`call_endpoint`, `recv_call`, `reply` over `seL4_Call`/`seL4_Reply`, compiling
clean) and then reverted, because wiring it into client B's burst would change
what that arm proves: the 24 requests are deliberately fired *before* any
terminal is read — fire-and-forget is the backpressure being tested — and a
`Call` blocks on each answer, turning the burst into 24 sequential round trips.
The primitive is right for the correlated request/reply arms and wrong for this
one, so it belongs to the redesign rather than ahead of it. Unwired code is not
deliverable, hence the revert.

**The circularity is now proven by construction, not argued (2026-08-13).**
Three shapes were built and measured, and each fails for the same reason:

- Queue the terminal and deliver it from the main loop with a blocking send —
  the client is mid-burst, blocked in its own `send`, so the two wait on each
  other.
- Gate that delivery on a `client_quiet` flag set by a sweep that took no
  request — the client cannot go quiet without receiving a terminal, and the
  terminal cannot be delivered until it does. The flag never sets. Observed:
  two of the burst's requests refused, then the plane stops.
- Offer the terminal with `try_send` — it reports nothing, so the record is
  either retired undelivered or never retired at all.

Chunking the client's burst does not escape it either: with `MAX_CALLS = 4`, a
chunk of six stalls on its third send, because refusal begins before the client
has stopped offering. The scenario was written against logical channels, where a
send buffered and returned; rendezvous does not buffer, so "send N, then read N"
is not expressible for any N above the in-flight bound without the broker being
able to wait on *both* directions at once.

That is the Notification, and it is now a measured requirement rather than a
design preference. The gate's own backpressure chain asserted a marker from the
stream broker's ring path (`terminal delivery ring backpressured`) that no call
emitter produces; it now asserts `terminal delivery queued`, which is the call
broker's equivalent, so the chain can fail honestly once the mechanism lands.

**Both halves landed, and the plane went from 14 markers to 57 (2026-08-13).**

*The multi-source wait.* One Notification, every call peer badged into it at
its own slot, the broker holding the single wait capability — observed at
CSpace 64–67 with one waiter. A peer signals *before* its blocking send, so the
wake is already pending when the broker reaches its wait and the next sweep
finds that sender blocked. Two one-signaller rules had to go, in
`build-generation.py` and in `boot-contracts`: both required a notification
grant to have exactly one signal and one wait binding, making a grant a *pair*
and foreclosing the only arrangement that answers "wake me when any of these
speak". The rule is now one waiter, at least one signaller, and the declared
source among them; per-holder slot uniqueness already keeps the badges apart.

*Receiver-confirmed retirement.* `KIND_TERMINAL_ACK` is in
`contracts/fabric-call/v1/`, because a wire fact belongs in the contract. The
client names the request it settled and echoes the status, so an ack cannot
retire a record for another outcome, and matching on the id means an
out-of-order or repeated ack cannot drop a terminal the client has not seen.
Guessing was tried twice and *lost* ground both times — keyed on the client's
next request, 25 markers down to 23; keyed on an `offered` flag, down to 19 —
which is the third time in this cutover that inferring delivery from
`seL4_NBSend` has failed.

Three further defects fell out of running it:

- **An ack for a stale-session terminal was refused as a stale call**, which
  queued another terminal, which was acked, without end. An ack settles a record
  and never opens one, so it is handled before the session guard.
- **The broker parked on the wake while holding a terminal.** The client waiting
  for it is in `recv` and never signals, so the wake could not arrive and the
  terminal was the very thing that would let the client run. It yields instead
  whenever a terminal is owed. This alone took the plane from 33 to 57.
- **The burst had to be chunked.** Client B fired 24 requests before reading
  any; a native send rendezvous, and the broker cannot answer a client that is
  still sending. Four chunks of six preserve the property — six still exceeds
  the four calls admitted in flight — while letting the broker answer.

**Client B's whole scenario now passes**, through backpressure recovery and
"unrelated route intact". `just sel4_call_check` fails with client A waiting on
a terminal the broker is holding, and that is a **deadlock, not a budget**:
raising `MAX_GRAPH_ITERATIONS` from 32,768 to 262,144 stops the plane at exactly
the same point with exactly 57 markers. The earlier "starved" reading is
withdrawn.

One real defect was found on the way there. `retire_terminal` returned after its
first match, so a request id recorded twice kept one copy forever — and a client
never acks an id it has already passed, so that copy stayed the queue's minimum
and blocked every later terminal. With it fixed, instrumenting the minimum shows
the mechanism working: it advances 5, 10, 6, 7, 8, 9 as acks arrive.

**The deadlock is isolated to one interaction (2026-08-13).** Instrumenting the
offer target shows the ordering rule working exactly as intended — ids 6, 7, 8,
9 offered to client A's slot in sequence, each advancing as its ack arrives —
and the broker yielding 19 times without ever parking, so it is re-offering
continuously. Client A takes 6, 7, and 8, then stops.

The remaining state is: **client A blocked sending the ack for 8, while the
broker is mid-sweep offering 9.** Neither is receiving. Two shapes were measured
and both are wrong:

- Ack with `try_send` — 57 markers down to **19**. The ack is the only thing
  that retires a record and `seL4_NBSend` reports nothing, so a dropped ack
  leaves the broker re-offering a terminal the reader already took.
- Ack with a blocking send — the current state, and where the deadlock is.

That is the same "two peers, opposite directions, one endpoint" shape the rest
of this cutover kept producing, and it says the ack needs a path that is neither
lossy nor blocking. The obvious candidate is the one this item's own fix text
names and the runtime still lacks: `seL4_Call`, where request and reply are one
atomic operation and the reply capability names *this* caller, so the broker's
answer cannot be taken by another peer and the caller cannot miss it.

A latent instance of the same hazard was fixed on the way:
`expect_terminal_parked` polled with `yield_now` against a broker that offers
with `seL4_NBSend`. Client A never reaches that step, so it changed nothing
measurable, but it is the fourth polling receive found in this scenario file.

**`Call`/`ReplyRecv` is in the runtime and wired (2026-08-13).**
`slime_rt::call` over `seL4_Call` and `slime_rt::reply` over `seL4_Reply`, with
the terminal ack as the call site — the one place where both alternatives are
provably wrong rather than merely suspect. It lands now, rather than when it was
first written and reverted, because it is load-bearing here.

Marker count holds at 57, so it replaces a deadlocking ack with a sound one
rather than advancing the plane. Client A still stops after taking terminal 8,
which now means it is blocked in `Call` awaiting the broker's reply. The broker
answers from its client sweep and reaches that sweep every pass, so the next
question is whether the reply capability survives the path it takes: both
`recv` and `nb_recv` pass `()` as the reply authority, which under the non-MCS
configuration is the thread's implicit reply capability, and that is the
assumption to check first.

**Two more hypotheses refuted by measurement (2026-08-13).** Instrumenting the
ack shows 34 round trips completing and the **34th hanging** — client A takes
all four timeout terminals, acks three, and blocks in `Call` on the fourth.

- *The broker parks on the notification with nothing owed, so the ack finds no
  receiver.* Signalling the wake before the ack changed nothing, and replacing
  the park with a bare `yield_now` — removing it entirely — also changed
  nothing. Both stop at exactly 57 markers. The park is not the blocker.
- *`seL4_Reply` answers the wrong caller.* The kernel headers say it uses "the
  reply capability stored when the thread was **last called**", and the broker
  receives on five endpoints per sweep — but the reply is issued with no
  intervening receive, so the stored capability is still the ack's.

What is established: the ack reaches the broker (it retires records and the
queue's minimum advances), the reply path is structurally sound, and the broker
is neither parked nor starved. The remaining suspect is the reply itself — a
`seL4_Reply` issued after a `nb_recv` rather than a blocking `recv`, which is
the one asymmetry not yet tested in isolation.

That suspect is refuted too. Making the broker's client receive blocking —
`recv_blocking` in place of `nb_recv`, so the reply follows a blocking receive —
drops the plane from 57 markers to **3**: a multiplexer that blocks on one
client cannot serve the others, which is the same lesson the rest of this
cutover taught. `seL4_Reply` after a non-blocking receive is not the fault.

Every hypothesis raised so far is now measured and refuted: the iteration
budget, offer ordering, supervision polling, the notification park, reply
authority, and the receive discipline under it. The ack demonstrably reaches the
broker and demonstrably retires records; the 34th `Call` does not return. What
has *not* been instrumented is the broker's side of that specific exchange —
whether `pump_client` observes the 34th ack at all, as against observing it and
failing to answer. That distinction is one marker inside the ack branch, and it
is the next thing to place.

**That observation was made, and it narrows the fault to the answer
(2026-08-13).** A marker inside the ack branch shows the broker seeing **all 34
acks**, including the one whose `Call` never returns. So the ack is delivered
and the broker does reach its handler — the fault is in the reply, not the
delivery.

One ordering defect was found and fixed there: the branch retired the record
before replying, putting `retire_terminal` and anything it logs between the
receive and the answer. `seL4_Reply` sends to "the reply capability stored when
the thread was last called", and `debug_write` is a root round trip, so an
intervening log would consume it. Replying first is correct regardless of
whether that window is currently reachable — and it is not the fault either:
57 markers with the reordering, with and without the diagnostic present.

**And that settled it the other way: the ack path is sound end to end.** A
marker after the `Call` shows all **34 returning**, not 33 — the "34th hangs"
reading was an artefact of the last marker being the last line before the root's
fatal. Client A completes every ack and moves on, so nothing in the
ack/reply/retire mechanism is at fault.

What remains is arithmetic: 52 terminals are queued and 34 acked, so **18 are
never taken** — and the plane stops with client A past its timeout loop. The
budget is not the constraint either (262,144 iterations stop at the same 57
markers, retested against this state). So a terminal client A still needs is
being offered to a client that is no longer reading it, or is queued for a
client index that does not match the reader. That is a question about *which*
records exist and for whom, which the queue-minimum instrumentation can answer
directly and which no further guess should precede.

**Three more measurements, and the loop is exonerated (2026-08-13).**

- *The broker is starved of loop passes.* Refuted, and the earlier evidence for
  it was an artefact: spin counters at 5, 10, 12, 14, 16, 20, and 25 all fire in
  the **full** QEMU transcript. They appeared absent only because the gate's tail
  truncates. The broker loops freely.
- *Terminals are owed to an exited client.* Refuted by instrumenting the queue's
  provenance: every pending record belongs to client **0**, which is still
  running. Client B's records are all taken before it exits. A
  `reclaim_dead_clients` pass keyed on the supervision handle was written for
  this and never fired once — reverted, since a guard that no run reaches is not
  earned. (It remains the right shape if a client ever *does* exit owing
  terminals; `seL4_NBSend` cannot distinguish "busy" from "gone", so only
  supervision can.)
- *`inFlightCalls = 4` is too small for the scenario.* Refuted and instructive:
  raising it to 8 *loses* the retry-exhaustion arm, because that bound is
  precisely what makes request 10 exceed the in-flight limit. The constant is
  load-bearing for the property under test, not incidental.

The queue's contents are now known exactly: client 0 holds terminals 4–9 in
`calls` and 10 in the overflow queue; client 1 holds ids 100–123 across both.
Client A takes 4, 5, and 10, then waits for 6 while the broker offers 6. Both
peers are at the same request id on the same endpoint, sessions match
(`client_session` is the identical expression on both sides), and neither
advances. That is the whole remaining fault, and it is now stated in terms that
can be checked directly rather than inferred.

**The blocker is `server_idle`, and releasing it is not the fix (2026-08-13).**
Client A's timeout arm sends ids 6–9 with payloads 106–109, which
`handle_inline` deliberately returns `None` for — the server is *supposed* never
to answer them. `server_idle` is cleared by forwarding and set only when the
server sends something back, so a request it never answers leaves it false
forever and every later forward is deferred behind it. The transcript agrees:
four forwards total, the last being id 10.

Releasing the server when a call times out is the obvious repair and it
**loses** two markers, 57 to 55, reproducibly. So the deferral is load-bearing
somewhere else, and the right fix has to distinguish "the server owes nothing on
this call" from "the server is free" — which the current single flag cannot
express, because it conflates a per-call obligation with a per-peer state.

A measurement hazard worth recording alongside it: a `git checkout` immediately
followed by a gate run reported 19 markers where three consecutive clean runs
report 57. That was a stale build, not variance. Every comparison in this
investigation is now taken from at least two consecutive runs.

**The blocker is fixed, and the metric that hid it was wrong (2026-08-13).**
Releasing the server when *its own* call times out advances the plane to
`[fabric-call-server] injected peer death` — the peer-death arm, several stages
past where it had been stopping — with three forwards instead of one.

That change was rejected twice before on raw marker count, which *falls* from 57
to 55. Comparing the distinct marker **sets** rather than the totals shows why:
the missing lines are repeated `terminal delivery queued` and `stale call
rejected` from a broker spinning on work it could not progress. A count that
rewards spinning is the wrong measure, and two earlier reverts this session were
made on it.

`server_idle` also became `server_call: Option<u64>` so the distinction is
expressible at all: a bare boolean cannot tell a timeout on the call the server
holds — which releases it — from a timeout on any other, which does not.

The plane now reaches ten distinct stages and stops with the server exited and
no `call peer death propagated`. Checking the supervision handle before
forwarding, on the theory that the broker was blocked in `send` to a dead
server, changes nothing — so it is blocked somewhere else, and that guard was
reverted rather than kept unearned.

**The block point is now exact: `pump_time`'s retry forward (2026-08-13).**
Instrumenting each loop stage shows the broker entering `pump_time` after the
server's exit and never reaching the next loop head. `pump_time`'s receive is
non-blocking, so the block is the `forward` it calls on the retry path — a
blocking `send` to a server that has exited, which a native Endpoint never
reports.

Three repairs were measured and all are wrong:

- *Check the supervision handle before the send.* No change. The server is
  still alive at the moment of the send — this plane's last request is what
  makes it exit — so no check placed before the send can see it.
- *Offer with `try_send` instead.* Ten stages down to **five**: forwards are
  dropped whenever the server has not yet reached `recv`, which is most of the
  time.
- *Release the server on any timeout.* Already committed; it is what got the
  plane to the peer-death arm, and does not address this.

So the forward needs what the ack needed: a path that is neither lossy nor
blocking. `slime_rt::call` now exists and has exactly that shape — the caller
blocks on a reply that names it, and a server that exits mid-call leaves the
kernel to fault the call rather than hanging it. That is the next thing to try,
and it is the same conclusion the terminal ack reached independently.

**Isolated to one send, with a stage-count metric that survives scrutiny.**
Instrumenting the forwarded request id shows the broker blocking on **id 11** —
the request whose payload makes `fabric-call-server` call `exit(0)`. The forward
itself *completes* (`call forwarded` is emitted, so the server took the
message); the server then exits, and the broker never reaches its next loop
head. Per-stage markers place the block inside `pump_time`, which is the only
stage after `pump_replies` (confirmed to complete), and whose sole blocking
operation is the `forward` on its retry path.

The timeout release is confirmed load-bearing by the honest metric: **10 stages
with it, 9 without**. Marker totals said the opposite, which is why they were
abandoned.

No available primitive fixes the retry forward. `send` blocks against a peer
that is exiting; `try_send` drops forwards whenever the server has not yet
reached `recv`, costing five stages; a liveness check before the send cannot
help because the server is alive at that instant; and `seL4_Call` does not model
this protocol, where the server replies asynchronously on its own endpoint and
some requests are answered by nothing at all. Non-MCS seL4 offers no bounded
send.

So the remaining fix is protocol-shaped rather than primitive-shaped: either the
server must not exit while a request it has taken is unanswered, or the broker
must not hold a forward it cannot retract. That is a scenario-and-broker change
together, and it is the honest remaining scope of this gate.

Two further repairs measured and reverted, both aimed at the same window:
observing the server's death at the top of `pump_time` before its retry loop,
and limiting the loop to one forward per pass so a timeout that releases
`server_call` cannot be followed by a retry in the same iteration. Neither
moves the stage count off 10, which means the block is not the *second* forward
in a pass — it is the pass that forwards id 11 itself, and no reordering within
`pump_time` can help because the server is alive when that send is issued and
gone before it completes.

That exhausts the repairs available above the transport. The send that hangs is
correct by every local rule: the server is live, the endpoint is right, the
message is well-formed, and `server_call` is clear. It hangs because
`seL4_Send` has no way to fail when its receiver exits mid-rendezvous, and
nothing in the non-MCS syscall surface expresses "send, but give up if the peer
dies". Either the scenario must not model peer death as an exit while holding a
taken request, or the plane needs an MCS kernel where a timeout can bound the
send.

Three candidates were measured and reverted rather than argued away: ordering
the offers by request id (a real correctness property, kept, but moves nothing
alone and regresses with a `terminal mismatch` when applied without the rest);
polling `supervision_status` only while the server owes an answer (a root round
trip per pass, so it looked like a drain — no change); and the iteration budget
above. One measured change was kept for its own sake: `pump_terminal` announced
every re-offer, and each `debug_write` is a root round trip, so the diagnostic
was spending the graph's shared budget on an unchanged condition — 138 markers
down to 52.

Three cheaper explanations were tested and refuted rather than argued away: both
clients reach the barrier and block; the generation installs the edge
symmetrically (`fabric-call-client` slot 4 / CSpace 37, `fabric-call-client-b`
slot 1 / CSpace 34, both `send`+`recv`); and making a blocking receive refuse
instead of reporting `ERR_WOULDBLOCK` did not move the stall, so receive-guard
contention is not involved.

**The final plane now passes (2026-08-13).** `sel4-operation.zti`'s native
control endpoints and phase barriers are ordinary declared grants; only the
shared-buffer factory and four supervision handles remain minted. The broker
waits on one multi-source Notification, retires mandatory deliveries only on a
receiver acknowledgement, observes peer death through supervision, and admits
one server-bound request at a time until the server's Zutai-defined
`KIND_SERVER_IDLE` fence arrives. The explicit fence replaced an empty-receive
heuristic that could not distinguish "no record" from "the single-threaded
server is ready". The QEMU gate reaches 53 markers across all 15 causal chains;
all six participant tasks and init exit cleanly.

**Exit condition observed (2026-08-13).** `channel.rs`, `transit.rs`,
`parked.rs`, `WaitSet`, and the migrated universal labels no longer exist.
Backpressure, bounded queues, timeouts, peer death, cap-transfer attenuation,
unrelated-route progress, and buffered-stream recovery passed all seven named
native Endpoint/Notification gates in one ordered run: `just
sel4_channel_check`, `just sel4_crossing_check`, `just sel4_stream_check`,
`just sel4_qos_check`, `just sel4_call_check`, `just sel4_operation_check`, and
`just sel4_visibility_check`.

Record: [`devlog/2026-08-13-b46-native-ipc-completion/`](../devlog/2026-08-13-b46-native-ipc-completion/index.md).

### B50 — the logical capability and universal syscall compatibility model remains deletable residue

**Status:** Resolved 2026-08-14. **Class:** Unmasked architectural debt.
**Depends on:** B39–B49.

**Problem:** Even after native replacements land, leaving `GraphTables` as an
authority database, the universal `Operation` dispatcher, public task IDs,
generic cross-kind `u64` rights, name-only grants, or compile-time plane flags
would preserve two competing authority/IPC models and invite fallback drift.

**Evidence:** The handoff identifies these as the retained custom-kernel model
implemented above seL4. B39–B49 each have a narrower removal boundary; this
item is the final repository-wide proof that no compatibility path survived.

**Fix:** Delete the global logical authority database, universal operation ABI,
public task identity, generic rights vocabulary where seL4 cap rights or typed
policy now apply, name-only grants, fixed-slot constants, and all product graph
selection flags. Remove obsolete tests, fixtures, comments, and generated
bindings rather than aliasing them.

**One clause is now checked (2026-08-10).** "Every fixture uses v5" is
asserted by `just contracts_check` through `scripts/check/check-generation-v5.py`,
which builds all 25 seL4 manifests and reads the magic and version word out of
the bytes the root decodes. Checked by building rather than reading, because a
manifest's own `formatVersion` field is the *manifest* schema's version and
says nothing about the wire format — the two are one word apart in a fixture.
The guard also pins the builder to a single version constant, since a second
one is how a v4 producer survives a cutover. Verified by setting
`GENERATION_VERSION = 4` and observing the refusal.

**The rest of this item is not startable yet, by its own terms.** B50 depends
on B39–B49 and is "the final repository-wide proof that no compatibility path
survived" — deleting `GraphTables`, the universal `Operation` dispatcher,
public task identity, generic `u64` rights, name-only grants, fixed-slot
constants, and the plane selection flags.

B47 and B49 are resolved, so the remaining dependency is B46 alone — and it is
a real one, not a formality. `GraphTables` has 25 references in `main.rs` and
is the table `DirectoryDerive` and the main dispatch loop both write; the
universal labels it would delete are the ones `channel.rs`, `transit.rs`, and
`parked.rs` implement. The 41 `SLIME_*_CHECK` compile-time plane flags are the
one clause that looks separable, and is not: each selects a scenario inside a
component that B46 rewrites against a different IPC model, so deleting them
first would mean rewriting the same 41 sites twice.

Deleting the model while B46 holds it up would be removing the proof rather
than the residue.

**The `fixed-slot constants` clause is now scoped, and it is four namespaces
rather than one (2026-08-12).** B46's cutover produced six consecutive
slot-collision failures, and every one was a hand-written number disagreeing
with another hand-written number. The clause covers:

1. **Declared slots in fixtures** — `bindings[].slot` and
   `mintedBindings[].slot`, hand-assigned per manifest across 25 fixtures. This
   is the one to auto-allocate: make `slot` optional in
   `contracts/generation/v1/schema.zt` and have `build-generation.py` assign it
   deterministically per namespace, ordered by grant name.
2. **Child CSpace regions** — endpoints at `33+n`, notifications at `64+n`,
   authority mirrors at `95+n`. Already derived from (1); nothing to do.
3. **Component-side constants** — `CONTROL_SLOT`, `FACTORY_SLOT`, and the
   `RING_SLOTS` depth that B46 already replaced with a profile lookup. These
   cannot simply become generated constants: one binary serves several
   manifests, so `fabric-publisher` runs in the stream, QoS, visibility, and
   boot graphs with different edge sets, and a name-sorted assignment would give
   it a different number per plane. They must resolve by role, as
   `supervision_slot_for(component)` and `FABRIC_FIRST_CONTROL_SLOT + index`
   already do.
4. **Root CSlots** — `ObjectAllocator::reserve_slot`. Already a bitmap
   allocator, and the namespace B46 left one open defect in.

A structural hazard belongs to (1) specifically: endpoint bindings and logical
bindings share one declared-slot number space, which the decoder refuses
duplicates in, yet they map to *disjoint* CSpace regions at runtime. So an
endpoint at declared slot 1 and a factory at declared slot 1 are rejected as
colliding despite landing at CSpace 34 and 1. That is authoring friction with no
runtime meaning, and per-namespace assignment removes it.

The frozen boot-layout fixtures that `just sel4_boot_layout_check` pins
byte-for-byte must keep accepting explicit slots, so auto-allocation has to be
opt-in per manifest rather than a global renumbering.

**Clause (1) is done (2026-08-13).** `slot` is now optional on
`InstanceBinding`, `MintedBinding`, and `NotificationBinding` in
`contracts/generation/v1/schema.zt`, and `assign_declared_slots` in
`build-generation.py` fills every omission at the one point the manifest is
decoded, so every consumer downstream still sees a concrete number. Explicit
slots are reserved *before* any assignment and never moved, which is what keeps
auto-allocation opt-in per binding and leaves the byte-pinned boot-layout
fixtures encoding exactly as before. Omitted slots take the lowest free number
in grant-name order, so the result is a function of the manifest alone.

The structural hazard this clause named is gone with it: capability bindings and
minted bindings share one namespace per holder because both land in the child's
capability table, while notification bindings get their own, so an endpoint at 0
and a factory at 0 no longer collide over a number neither runtime region shares.

Verified by removing seven hand-written slot numbers from `sel4-stream.zti` --
every holder whose numbering was pure drift -- and observing a **byte-identical
`build/slime-sel4-stream.elf`** (`md5 6eff83ca…`) with 44 explicit slots as with
51. The builder reproduces the hand-written numbering exactly, and
`just sel4_stream_check` passes either way.

Clause (3) moved with it. Init's spawn-grant count came from a hardcoded list
that was the stream graph's, so every other plane's spawn was refused with
`declared-count requested=0` for a number init had no way to know. The builder
now emits `FABRIC_MINTED_GRANTS` -- per instance, the total
`preflight_spawn_grants` checks against, by one shared rule
(`declared_spawn_grant_counts`) rather than a second implementation of the
root's -- and init reads it. That also retired the
`SLIME_FABRIC_VISIBILITY_CHECK` branch selecting the fabric's grant set: stream
declares five and visibility six, and the same binary now picks by manifest
rather than by build flag. A manifest declaring no fabric graph emits this one
table too, so `init.rs` compiles against every graph.

**A concrete deletion is identified and scoped: `endpointCreate`
(2026-08-13).** The cutover removed the `EndpointCreate` operation, so the root
has no resource to install for that right — but the grant survives in eleven
fixtures, and `declared_resource` refuses it with `SLIME_GRAPH FAIL binding
init-endpoint-factory names no installable resource`. It is the single cause of
at least three red plane gates (`sel4_input_check`, `sel4_spawn_check`,
`sel4_supervision_check`, all failing on that exact marker) and of
`generation_check`'s `sel4_component_graph_check` arm. No component reads it:
`ENDPOINT_FACTORY_SLOT` appears only in generated boot-layout constants, never
in a call site. It is exactly the "generic rights vocabulary where seL4 cap
rights now apply" this item names.

**It is done (2026-08-13).** All nineteen grants and their bindings are gone
from the eleven fixtures, along with the projection assertion in
`check-boot-layout-resource.py` that required the role to be present. The
boot-layout fixtures are re-blessed: 24 files, a net 29-line deletion, which is
what removing a dead slot from a frozen transcription looks like. The
`endpoint-factory` *layout role* stays: it is a numbered entry in a generated
contract (`ROLE_ENDPOINT_FACTORY = 1`), so removing it renumbers every role
after it across `boot_layout.py`, `boot-contracts`, and the generated bindings —
a separate deletion with a much larger blast radius and no gate depending on it.

`just contracts_check`, `just sel4_boot_layout_check`, `just test_sel4_root`,
`just lint_all`, and `just fmt_check_all` pass, as do the six plane gates that
were already green.

**What it did *not* unblock, and why that was worth learning.** The three gates
this was expected to fix now fail one layer deeper, on a *different*
pre-existing cause: `spawn preflight … reason=declared-count requested=0
bindings=0 minted=N`. Their fixtures declare minted bindings init must create
and hand over, and post-cutover init mints nothing — the same shape
`sel4-call.zti` had.

Converting them is not mechanical, and `sel4-spawn.zti` shows why. Its seven
minted bindings are orphans with no grant behind them at all, so declaring them
as ordinary grants admits and boots the graph — and then `console` and `sysinfo`
block forever waiting for a launch context, because `check-sel4-spawn-plane.py`
asserts `grants=1` and six respectively at the *spawn marker*. That plane's
claim is that a parent hands a child its capabilities at spawn; making them
declared moves the same capabilities to the generation and quietly deletes the
property under test. So the fix is init supplying them, not the fixture shedding
them, and each plane needs that judgement made against what its gate asserts.

**Exit condition:** Exact-source guards find no deleted model symbols or build
flags; every surviving syscall is either a direct seL4 primitive or a narrowly
owned root mechanism with a declared v5 capability; every fixture uses v5.
`just test_sel4_root`, `just contracts_check`, `just generation_check`, all
affected `just sel4_*_check` targets, `just sel4_gate_control_check`, `just
fmt_check_all`, and `just lint_all` pass after the deletion.

**It is done (2026-08-14).** The last clause was `mintedBindings` of kind
`endpoint`, and the judgement each plane needed turned out to be one judgement:
such a binding is *unsatisfiable*, not merely unused. The record defers object
identity while fixing owner, holder, slot, and rights ceiling, which was right
when a component could create a channel — and post-cutover an endpoint is a
generation-owned seL4 Endpoint the root materializes into both declared ends, so
no party can supply one. `preflight_spawn_grants` counted all 63 of them across
eleven fixtures in the total a parent must satisfy, which is why ten gates
refused every spawn.

Six probe planes declare their run token as an ordinary grant with a loopback for
the idle instance, so arrival rather than presence discriminates — their
`startup_arg == 0` guard had been unreachable since the cutover, and every
spawned probe took the idle path. The spawn plane keeps its claim by crossing
six narrowed *transferable directory views* instead of endpoint halves: the gate
asserts the grant count at the spawn marker, so the authority had to be something
a parent both holds and may pass on. The generation and filesystem services learn
a peer's death from a supervision handle or a declared close edge. `sel4-boot.zti`
converted mechanically — its 41 bindings were the two ends of 16 control edges
the manifest already named.

With no fixture declaring one, `CapabilityGrant.minted` had no producer, so the
field and `GRANT_MINTED` are deleted from the source schema, the builder, the
decoder, and the independent checker. `flags` still refuses unknown bits.
`channel_aliases` went with it: it existed only to publish `SERVICE_SPAWN_SLOT`,
a generated constant no component reads.

Two root defects surfaced, both pre-existing and both invisible until a plane
exercised them. `preflight_spawn_grants` excluded self-loop grants from the count
while `declarations_below` numbered them in the ordering, so any child holding
both a self-loop capability and a minted binding was unspawnable — one shared
`grant_crosses_spawn` now answers both questions. `supervision_derive` copied its
source's rights, and a spawn returns `RIGHT_SUPERVISE` alone, so a derived handle
could never be delegated; derivation is the "I intend to hand this on" operation
and now adds `RIGHT_TRANSFER`.

Three components still read a transferred capability out of the
received-capability array, which since B46 carries only native Endpoint handles;
they claim the export with `capability_import`. And the supervision plane's B25
derive scenario — dropped silently during the cutover, leaving its gate asserting
a marker no boot emitted — is restored, now proving a derived handle outlives both
its task and the source handle it came from.

**Twenty-four gates pass**, including all three exit-condition gates
(`generation_check`, `sel4_gate_control_check`, `contracts_check`) and every
affected plane gate. Ten went from admission-refused to green:
`sel4_spawn_check`, `sel4_generation_check`, `sel4_filesystem_check`,
`sel4_directory_check`, `sel4_input_check`, `sel4_storage_check`,
`sel4_store_check`, `sel4_rollback_check`, `sel4_recovery_plane_check`,
`sel4_transfer_check`; `sel4_supervision_check` went from a missing derive marker
to green. `just fmt_check_all`, `just lint_all`, `just test_sel4_root`, and
`just test_host` pass. See
[the devlog entry](../devlog/2026-08-14-b50-minted-endpoint-deletion/index.md).

**Two failures are outside this item and recorded rather than folded in.**
`sel4_dango_check`'s fixture is converted and observed working — its control
endpoints and the supervision handle cross correctly, `sysinfo` runs through the
profile, the `dango>` prompt appears — and then `dango` exits 1 in its
scripted-input loop, which is a separate defect. `sel4_stress_check` failed at
HEAD before this change and still fails, now one layer deeper: the plan-budget
marker it never reached is satisfied and it stops at `the graph never reclaimed to
zero live tasks`.

### B48 — all child execution shares one fixed priority and no scheduling authority

**Status:** Resolved 2026-08-12 with the MCS-only clauses explicitly deferred.

**Problem:** Every child used one fixed priority and the generation's schedule
records did not affect running TCBs. MCS-only budget, period, passive-server
donation, and timeout-fault features were also unavailable on the selected
AArch64 kernel configuration.

**Resolution.** Priority is authenticated per-thread generation data, bounded
at 254 by both builder and root, and applied to boot and spawn TCBs. The sample
plane runs a worker at priority 100 below its main thread and proves the main
thread completes while the worker spins 200M iterations without yielding. The
QoS graph also observes its declared priority split.

The MCS half is explicitly deferred rather than silently claimed. Upstream
`deps/sel4/CAVEATS.md` states that functional-correctness proofs for MCS on
AArch64 are in progress. Enabling it would weaken this repository's assurance
boundary, so `KernelIsMCS` stays off, `budget_us` and `period_us` stay zero, and
timeout endpoints are not claimed. The decision and revisit condition are
recorded in `devlog/2026-08-12-b48-mcs-assurance/`.

**Observed exit.** `just sel4_qos_check`, `just sel4_sample_check`, and the
platform-timer assertions in `just sel4_root_boot_check` pass under the selected
priority-only scheduling configuration. `just devlog_check` passes with the
assurance decision indexed. The deferred MCS clauses remain a named follow-up,
not an unobserved completion claim.

**Devlogs:** `devlog/2026-08-10-b48-declared-priority/`,
`devlog/2026-08-10-b48-per-thread-priority/`, and
`devlog/2026-08-12-b48-mcs-assurance/`.

### B49 — resource ceilings are reactive tables rather than an admitted object budget

**Status:** Resolved 2026-08-10.

**Problem:** Static table constants bounded tasks, capabilities, channels, and
transit according to the largest graph seen so far. The generation could not
prove before activation that its objects fit.

**The stress graph found the defect immediately.** A 48-instance generation was
admitted and then died at instance 39 with `SlotsExhausted`, 38 children
already running — the exact failure admission exists to prevent. Two
independent undercounts:

- **No aggregate.** Every check was per-instance. Nothing summed them, so a
  graph of N processes that each individually fit was admitted regardless of N.
- **The quota omitted its largest term.** The builder excluded the image's
  frames on the grounds that the root maps them from its own untyped. But root
  CSlots are the resource that runs out, so an object the root allocates is
  precisely the one that must appear in the budget. A process declared 6
  objects and cost 81 slots.

**Resolution.** The builder derives each process's frame count from the pages
its ELF actually loads, plus one IPC-buffer/window pair per thread, and counts
the VSpace. `admit_total_slots` sums every quota against the allocator's real
free-slot count before any component starts, refusing
`PlanExceedsRootSlots`. Per-class admission covers six classes rather than
three, extracted into `admit_resource_quota` where a test can reach it.

**Exit condition met.** `just sel4_stress_check` boots the 23-instance
graph — the largest this root's CSpace admits — constructs all 23, and
reclaims every one at `3084` of `3180` slots. One instance more is refused
before activation: `PlanExceedsRootSlots { required: 3219, available: 3180 }`.
`just contracts_check`, `just generation_check`, `just sel4_reclamation_check`,
and `just sel4_boot_check` pass, with `test_sel4_root` at 149.

**Not covered:** IRQs and untyped size classes remain unmodelled, and no plane
declares either. `MAX_TASKS`/`MAX_CHANNELS`/`MAX_TRANSIT` are still watermarks;
the CSpace check is now the binding constraint.

**Devlog:** `devlog/2026-08-10-b49-object-budget/index.md`.

### B47 — package, process, thread, service instance, and lifecycle are one Task model

**Status:** Resolved 2026-08-10.

**Problem:** One `Task` meant image instance, CSpace/VSpace owner, single TCB,
service identity, scheduling unit, and lifecycle identity at once, which forced
every component to be single-threaded.

**Resolution.** A process runs up to `MAX_CHILD_THREADS` threads sharing one
CSpace and VSpace, each owning a TCB, stack, IPC buffer, transfer window, and
schedule. The format half landed first (`f93a55b`, `8e49b5e`): the builder
indexed threads as processes, `validate_plan` required equal counts, and the
per-thread check demanded `main_thread == index`. The runtime half followed.

**The real obstacle was the IPC buffer, not the TCB.** Components build for
`aarch64-sel4-minimal`, which declares no `has-thread-local`, so `sel4`'s
buffer slot is one process-wide static. Five transport sites reached it; each
now branches, with non-main threads supplying their own through `Cap::with` —
the answer B41 reached in the root. `WINDOW_BASE`/`WINDOW_LEN` became
per-thread arrays for the same reason.

**A thread's identity lives in `TPIDR_EL0`,** because the kernel
context-switches it and no two threads can observe each other's value. It must
be set in the register context, not through `seL4_TCB_SetTLSBase`: seL4 counts
that register in the general-purpose set, so a later `WriteRegisters`
overwrites a separately invoked TLS base with zero.

**Exit condition met.** `sample-worker` declares `extraThreads = 1` and both
threads print; the plane refuses a transcript missing either line. Two
mutations were observed to fail — never resuming the worker, and giving it the
main thread's index (which faults, rather than silently sharing a buffer).
`just test_sel4_root` (146), `just sel4_spawn_check`, `just
sel4_supervision_check`, `just sel4_reclamation_check`, and `just
sel4_boot_check` all pass, alongside the full 31-plane sweep.

**Devlog:** `devlog/2026-08-10-b47-runtime-threads/index.md`.

### B52 — the loan plane never launches the receiver it loans to

**Status:** Resolved 2026-08-10.

**Problem:** `just sel4_loan_check` failed at `[init] loan plane fail: loan`
with `SLIME_GRAPH loan refused class=absent-or-ambiguous`. A loan names its
receiver as the unique live holder of the channel's other end, and
`sample-receiver` was declared and never spawned. The strand arm had the same
defect one arm later against `console`.

**Not caused by the v5 cutover:** verified red at `8745d18~1`.

**Exit condition observed:** `just sel4_loan_check` passes — "a sealed subrange
was loaned to a receiver named by capability, mapped read-only, returned once,
and reclaimed; all four declared quota classes refused at ceiling+1 without
disturbing an unrelated holder". `just sel4_sample_check`, `just
sel4_spawn_check`, and `just sel4_reclamation_check` pass, along with the other
28 plane gates and `contracts_check`, `sel4_boot_layout_check`, and
`sel4_gate_control_check`.

**The two peers needed different answers.** `sample-receiver`'s channel end is
a binding init holds and can pass, so init spawns it with that one grant —
which is what `drive_loan_plane`'s docstring said the cutover lacked "until
P5.3.3", and now has. `console`'s two ends are the opposite: init holds the
*producer* side of each and cannot hand over a consumer end it does not have,
so console is root-owned autostart instead. Spawned-but-idle is also exactly
what the strand needs: a deterministic queue nobody collects, rather than an
absent peer the root refuses before recording the loan at all.

The unsealed probe moved after the receiver spawn. It loans deliberately to
prove an unsealed region is refused, and with no receiver it was refused
`absent-or-ambiguous` instead — the right outcome for the wrong reason,
passing vacuously.

**Four of the gate's own assertions had never run**, because it failed before
reaching them. Its shared-buffer budget parse required `holder` as the first
field while Zutai renders record fields alphabetically, so it matched nothing
and would have reported "declares no budget entries" for any fixture at all.
Its quota parse still read the `component=` field, renamed to
`instance=`/`executable=`. The admitted grant count and the `quotas declared=`
count both moved with this change.

The last of those found a real asymmetry rather than a stale pin: the boot path
declared its quotas silently and printed only the aggregate, so a per-instance
ceiling could be wrong in the generation and invisible in every transcript. It
emits the same per-instance record the spawn path does now.

### B51 — the spawn preflight cannot tell a respawn from a first launch

**Status:** Resolved 2026-08-10.

**Problem:** `spawn_preflight` checked a request against the child instance's
declared bindings plus minted bindings, which assumes one spawn per
declaration. A respawn — the same instance, after the first died and was
collected — is that declaration launched again, and the root could not tell:
`task_for_instance` answers liveness and `release_by_task` clears it, which is
right for liveness and destroys provenance.

**Exit condition observed:** `just sel4_sample_check` passes, with the plane's
third spawn admitted and the respawned child reaching a clean exit. `just
sel4_spawn_check`, `just sel4_reclamation_check`, and `just
sel4_component_graph_check` pass, along with twenty-three further plane gates.
The gate-control mutation the exit condition asks for is a respawn carrying one
grant instead of none: it is refused on the declared count, verified by making
the plane do exactly that.

**A respawn brings nothing, not merely fewer.** The first shape allowed at most
the declared count, which is unsound — declaration matching is positional, so
request N binds to the declaration with the Nth-lowest destination slot, and a
partial request installs the caller's first capability at another
declaration's slot under that declaration's rights ceiling with no error. An
empty request has no such ambiguity, and a full one is checked exactly as a
first launch is.

`LaunchedInstances` keeps a per-instance bitmap that outlives collection, which
answers the question the preflight actually asks. B51's own text suggested a
restart policy declared on the instance; that is the better long-term answer,
belongs with B47's lifecycle model, and would have invented a contract field
B47 may shape differently.

`sample-receiver` exits 0 when its peer slot holds nothing. It is a `required`
instance, so the throwaway retry's deliberate emptiness was otherwise fatal; a
component with no channel has nothing to verify.

**Three assertions on that gate had never been reached:** init's shared-buffer
factory pinned at slot 14 where the fixture declares 4, `quota` markers naming
a `component=` field that is now `instance=`/`executable=`, and B14's budget
probe, which the root answers `instance-live` before any ceiling is consulted —
as `init.rs`'s own comment already recorded.

Record: [`devlog/2026-08-10-b51-respawn-provenance/`](../devlog/2026-08-10-b51-respawn-provenance/index.md).

### B45 — directory, filesystem, and store services still depend on universal root IPC

**Status:** Resolved 2026-08-10.

**Problem:** Directory inspection, derivation, and commit, and store requests,
reached clients as operation labels on the root endpoint, so capability
provenance was checked in a global software table rather than expressed by
holding a service endpoint.

**Exit condition observed:** `just sel4_directory_check`, `just
sel4_filesystem_check`, `just sel4_store_check`, `just sel4_powerbox_check`,
and `just sel4_dango_check` all pass. `DirectoryInspect`, `DirectoryCommit`,
and `StoreTransact` are absent from `slime-root/src/ipc.rs::Operation` — the
last of those left with B43. Attenuation, provenance, malformed requests, and
service death remain observable: the directory gate records two commits, a
stale refusal, and a scoped-commit refusal across three derivations, and the
powerbox gate observes exactly one capability crossing at rights `0x80004`
with a widening request denied.

**`DirectoryDerive` did not move, and should not have.** It is the only writer
of the caller's `GraphTables` entry, and the main dispatcher writes that same
entry on `cap_drop` and on a spawn's result. Two threads writing one task's
capability table is a data race, not a decoupling. The alternatives were a lock
— which would make the second dispatcher block on the first, the exact coupling
B41 removed — or moving `GraphTables` wholesale, which is B50's item, since the
global authority database is what forces the choice. `ScopeTable` stays with
derive for the same reason and is read from the console thread; a scope, once
interned, is never mutated or freed, so a concurrent read observes the old
table or the new one and never a torn scope. That is weaker than the device
tables' exclusive ownership and is recorded as such.

The handlers, `Namespaces`, `DisplayPath`, and the directory rights moved into
a new `slime-root/src/directory.rs`. `RIGHT_TRANSFER` now comes from
`boot-contracts`, its canonical definition, instead of being restated in the
binary beside a comment saying it was restated.

**Two of the five gates were red on arrival**, both for the omission the
generation plane had: init mints an endpoint pair as each child's run token,
and `mintedBindings` was `[]`, so the spawn preflight saw one requested grant
against zero declared and refused before either plane ran a scenario.
`sel4_powerbox_check` — red since the start of this backlog run — and
`sel4_filesystem_check` are green.

Record: [`devlog/2026-08-10-b45-directory-service-split/`](../devlog/2026-08-10-b45-directory-service-split/index.md).

### B44 — generation and recovery policy still crosses the universal root dispatcher

**Status:** Resolved 2026-08-10.

**Problem:** `HealthConfirm`, `RecoveryReconstruct`, `GenerationTransact`, and
`GenerationReceive` entered the universal root dispatcher, coupling policy
clients to root's global request ABI after B35 made the durable boot selector
authoritative.

**Exit condition observed:** all four labels are gone from
`slime-root/src/ipc.rs::Operation`, so a client is denied by seL4 lookup
because there is no root-side path at all — the strongest form of the property
the exit condition asks for. `just sel4_generation_check`, `just
sel4_boot_selection_check`, `just sel4_rollback_check`, `just
sel4_recovery_plane_check`, and `just sel4_transfer_check` all pass, with no
dispatcher fallback anywhere behind them.

**The fix was removal, not endpoints.** None of the four was reachable. Three
answered `UnsupportedOperation` from `Mediation::Unavailable`. The fourth had a
real handler arm that never ran: boot promotion happens from the supervisor's
idle path once every required instance parks, not from a component asking for
it. That was measured — a `debug_println!` in the arm, then a full
`sel4_boot_selection_check`, zero hits — rather than reasoned about. Building a
service for four operations nobody invokes would have been the worse answer.

`Mediation::Unavailable` went with them, since B43's `StoreTransact` had been
its last other member, and `check-sel4-component-graph.py`'s assertion inverted
accordingly: it asserted each unmediated plane stayed unmediated, and now
asserts none remains and that all seven retired labels refuse rather than
resolve. Deleted alongside: `recovery.rs` (in no manifest at all),
`generation-list.rs`, `generation-manager.rs`, init's `SLIME_TRANSFER_RECEIVER`
branch (nothing set the flag), and the root-endpoint `transact` helper whose
last caller left with B43.

**`sel4_generation_check` was red on arrival** and is green now, for the same
two reasons the probe planes were: `mintedBindings` was empty, so the spawn
preflight saw one requested run token against zero declared; and the plane's
two idle markers had no emitter without a second root-owned copy of each
executable. Both are declared.

Record: [`devlog/2026-08-10-b44-policy-labels-deleted/`](../devlog/2026-08-10-b44-policy-labels-deleted/index.md).

### B43 — block and durable-store clients still transact through root operation labels

**Status:** Resolved 2026-08-10.

**Problem:** `BlockTransact` and `StoreTransact` shared the universal
dispatcher, so root was both IPC broker and driver dispatcher and unrelated
clients shared latency and failure scope. A block request needed no declared
service capability, only a label.

**Exit condition observed:** neither label exists in
`slime-root/src/ipc.rs::Operation`. Block requests reach the console thread on
the per-process console endpoint, which owns the device tables — so a component
without that capability has no path to a device at all, and the service loop
makes no progress on a block client's behalf because it never sees the request.
`just sel4_device_check`, `just sel4_storage_check`, `just sel4_store_check`,
`just sel4_rollback_check`, `just sel4_recovery_plane_check`, and `just
sel4_transfer_check` all pass against that path, along with ten further planes.
Read-only device authority is verified byte-identical from the host, and the
transfer gate now asserts that no write was ever *served* on the source rather
than only that one was refused. Multi-device selection is asserted exactly —
requests reached both device 0 and device 1 under their own indices — and the
assertion was proven load-bearing by pinning the handler's index to 0 and
observing the plane fail to complete.

**The tables moved with the handler.** Whoever answers block requests is the
driver, so leaving `BlockDevices` with the main dispatcher and passing a borrow
would have split that authority across two threads and needed a lock the root
does not have. `BlockDevices` and `MAX_BLOCK_DEVICES` moved into `device.rs`,
the handler and the block rights constants into `console.rs`, and
`serve_instance_graph`'s device parameter is now selector-only — that variant
launches no components and never constructs the second thread.

**The two labels left in opposite directions.** Block requests moved because a
slow disk must not hold up lifecycle, supervision, or fabric traffic.
`StoreTransact` was deleted because it never had a handler: it answered
`UnsupportedOperation` from `Mediation::Unavailable`, which is ABI surface for
an operation the root does not perform. A durable store is userspace policy
built over block authority, which `sel4-store-probe` and
`sel4-filesystem-service` already do; its two remaining clients predated the
seL4 cutover, appeared in no seL4 manifest, and could only ever fail, so they
were deleted rather than ported.

**One endpoint, three kinds.** A dedicated block endpoint would need a second
blocking receive and so a third thread. The console endpoint already carries a
Call kind with reply authority and a per-process badge, so a third label costs
nothing. Labels 6, 7, and 17 are left as holes, as label 5 was.

Record: [`devlog/2026-08-10-b43-block-service-endpoint/`](../devlog/2026-08-10-b43-block-service-endpoint/index.md),
with the earlier renumbering defect at
[`devlog/2026-08-10-b43-block-device-renumbering/`](../devlog/2026-08-10-b43-block-device-renumbering/index.md).

### B41 — console and debug traffic still enters the universal root dispatcher

**Status:** Resolved 2026-08-10.

**Problem:** `DebugWrite` and console/input-adjacent control shared the same
badged root endpoint and dispatcher as lifecycle, storage, and fabric traffic.
A noisy client therefore consumed the highest-priority root service loop and a
console defect shared the system-wide dispatcher fault domain.

**Exit condition observed:** neither `DebugWrite` nor `InputRead` exists in
`slime-root/src/ipc.rs::Operation`. Every process holds a console capability at
a declared slot, minted write-plus-reply and never receive, so a component
without one faults rather than falling back — denial is a missing CPtr.
`just sel4_root_boot_check`, `just sel4_input_check`, and `just
sel4_dango_check` pass, along with thirteen other plane gates. The control the
exit condition asks for is
`ipc::tests::no_console_operation_is_reachable_on_the_universal_abi`, verified
by reintroducing `DebugWrite` with its `from_label` arm and observing the
refusal.

**What made it possible.** The obstacle was never scheduling or capabilities —
it was the `sel4` crate's ambient IPC-buffer slot. There is one per address
space and `recv_with_mrs` holds it borrowed for as long as it blocks, so a
second thread using it deadlocks on the borrow rather than on the endpoint.
Three routes were tried and abandoned: a thread-local target (whose images lose
their `PT_TLS` header in the loader), `non-thread-local-state` (which selects
the token guarding the slot, not the number of slots), and `set_ipc_buffer` on
the new thread (which contends for the same slot). The answer is `Cap::with`: a
capability can carry its own invocation context, so the console thread names
its buffer on every invocation and touches no ambient state. Two call sites
needed it — the receive, and mapping the caller's staged window.

**Shape of the result.** One endpoint carries both kinds, distinguished by
label. Input returns a value where a write is one-way, but a second endpoint
would need a second blocking receive and so a third thread; console output and
input are both "the terminal", so one queue between them is honest and the
loop uses `ReplyRecv` to answer a read and wait for the next message in one
syscall. `ScriptedInput` moved to the console thread whole rather than being
shared: its per-task cursor is session state nothing else touches. The thread
has its own scratch page, since staging maps a caller's frame at
`ScratchPage::addr()`.

Retired labels are holes, not renumbered: a component built against the old ABI
is refused, where renumbering would have it silently invoke whichever operation
moved into the slot. Label 5 is the exception — the P5.1 fixture child was
using it to collect the root's directive, never to write to a console, so it is
`Operation::FixtureDirective` under a name that says what it does.

Records: [`devlog/2026-08-10-b41-console-endpoint/`](../devlog/2026-08-10-b41-console-endpoint/index.md)
and [`devlog/2026-08-10-b41-second-dispatcher-blocker/`](../devlog/2026-08-10-b41-second-dispatcher-blocker/index.md).

### B42 — spawn and lifecycle control use ambient task IDs and the universal dispatcher

**Status:** Resolved 2026-08-10.

**Problem:** `spawn` returned both a numeric `task_id` and a supervision slot,
and the spawn protocol sent that number across a process boundary to wait for
termination. A numeric task id is not authority — it is a name anyone can forge
by counting — so lifecycle identity was ambient.

**Exit condition observed:** no Zutai wire record or public runtime type
exposes a bare task id. `task_id` is gone from `contracts/spawn/v1/schema.zt`,
from the generated `WireSpawnReply`, and from `slime_rt::Spawned`, with no
compatibility shim. The spawn service keys its live table on the supervision
slot it handed back, and dango waits on the handle it holds, so spawn, wait,
and health all work through the capability alone.

Handle coverage: derived (attenuated), transferred, parked in transit across a
sweep, retained across the crossing, and — added here — stale. Collecting an
outcome consumes the record, and the same handle then refuses rather than
answering twice from a stale table, which is the distinction a reusable number
could not make.

`just sel4_spawn_check`, `just sel4_supervision_check`, `just
sel4_reclamation_check`, and `just sel4_dango_check` all pass.
`scripts/check/check-lifecycle-identity.py`, wired into `just contracts_check`,
refuses the reintroduction: it matches task-id-shaped *declarations* in
schemas, generated protocol Rust, and the runtime's public surface. A comment
explaining the ban is not a breach, and the root's own in-memory `TaskId` is
out of scope because it crosses no boundary. Verified by reinstating `task_id`
in the spawn schema and observing the refusal.

**Gate repairs this required.** B34 renamed the markers reporting the
executable/instance split and ten plane gates still asserted `components=N`,
failing on the first marker so everything behind it went untested. Three
assertions could never have matched — spliced prose in the marker text, a
field the staged record never had, and a frozen `activated` count that only
ever covered root-launched instances. Two markers had no emitter at all:
`factory placed` now comes from the boot graph's binding install, and
`channel copied` from the parent's channel-end copy. `spawned … channels=N`
counted only generation-declared re-installs and now counts minted ends too.

Closure record: [`devlog/2026-08-10-b42-lifecycle-identity/`](../devlog/2026-08-10-b42-lifecycle-identity/index.md).

### B40 — child CSpaces are fixed four-slot shells rather than admitted authority

**Status:** Resolved 2026-08-10.

**Problem:** Every child CNode had four slots — null, root service endpoint,
own TCB, and fault endpoint — with those slots compiled in, while actual
authority stayed in a root-side `CapabilityTable`. The v5 plan already declared
each process's CNode size, its own TCB and fault bindings, and its service
binding, and the root ignored all of it, so the kernel could not enforce the
declared layout.

**Exit condition observed:** `just sel4_capability_layout_check` boots the
twenty-instance graph and requires every child's CSpace to match the admitted
plan, then rebuilds the root once per injected mutation and requires each to be
refused — missing, extra, wrong type, wrong slot, aliased, and wrong rights. A
mutation that still boots is the gate's failure condition. `just
sel4_boot_check`, `just sel4_root_boot_check`, `just sel4_component_graph_check`,
`just sel4_reclamation_check`, `just contracts_check`, `just generation_check`,
and `just test_sel4_root` (140) all pass on the same layout.

**What the kernel can be asked.** seL4 exposes no "read this slot", so each
property needed its own probe and one could not be answered at the slot at all.
Occupancy is a self-`Move`: `ensureEmptySlot` runs before the source lookup
(`deps/seL4/src/object/cnode.c:93`), so occupied answers `DeleteFirst`, empty
answers `FailedLookup`, and neither mutates. Type is a `tcb_suspend` on a
root-side copy, refused with `InvalidCapability` for any non-TCB. Rights and
identity are *not* observable — `maskCapRights` masks silently and never
reports back — so both are checked at `InstallLedger::record`, the single
chokepoint every child install passes through.

**Service-slot pin.** Making the slot plan-driven newly created drift against
`ROOT_SERVICE_SLOT`, the constant every component's runtime resolves the root
endpoint from: a plan naming another slot would build clean, admit clean, pass
an audit that validates against that same plan, and produce children whose
first syscall invokes an empty slot. The root (`ChildSlots::validate`) and the
host checker both pin it until the runtime reads the slot from the boot layout.

**Not covered.** The P5.1 fixture paths construct tasks outside any plan and
keep the four-slot shell, now passed explicitly as `ChildSlots::SHELL` rather
than inherited.

Closure record:
[`devlog/2026-08-10-b40-native-child-cspaces/`](../devlog/2026-08-10-b40-native-child-cspaces/index.md).

### B39 — Generation v5 must describe the exact seL4 object and authority plan

**Status:** Resolved 2026-08-10.

**Problem:** Generation v4 declared logical objects and grants that
`slime-root` reinterpreted, so it could not prove the process/thread topology,
kernel objects, mappings, CSpace bindings, scheduling policy, fault policy,
spawn templates, or dynamic reserve the admitted graph would consume. `init`
also selected its scenario graph through `SLIME_GENERATION_NUMBER` and
`SLIME_*_CHECK` build flags.

**Exit condition observed:** `just contracts_check` and `just generation_check`
pass, proving every binding and object reference resolves, every
authority-bearing grant maps to a planned capability or is explicitly deferred,
and two isolated builds are byte-identical. `just sel4_boot_check` passes: the
full graph — twenty declared instances across five routes, split into three
bounded route workers — comes to rest at the supervisor's terminal record with
every required instance parked and none completed or failed, selected only by
the generation's authenticated `bootAction`. No product code admits generation
v4: `MAGIC_V4` survives solely to reject it, with
`rejects_v4_product_generations` proving so.

**What the format gained.** Ten plan record types (`Process`, `Thread`,
`KernelObject`, `Mapping`, `CapBinding`, `ServiceBinding`, `Schedule`,
`FaultPolicy`, `SpawnTemplate`, `ResourceQuota`) plus two deferral records for
authority whose object does not exist until runtime: a `MintedBinding`, and a
`CapabilityGrant` marked `minted`. Both fix the edge, its endpoints, the
destination slot, and an exact rights ceiling before activation, deferring only
object identity — which is intrinsic, since the object's creator runs after
admission. A relationship needing identity pinned uses an ordinary grant
against a concrete object.

**Boot-graph selection.** The root delivers the authenticated `bootAction` in
the bootstrap thread's first C parameter and `init` composes from it before any
build flag is read. Every `SLIME_SEL4_*_CHECK` branch is gone; an unimplemented
action is a boot failure rather than a fallthrough. `init`'s copy of the action
numbering is pinned to the contract by a const-assert per variant.

Audit and closure record:
[`devlog/2026-08-10-b39-generation-v5-checker-cutover/`](../devlog/2026-08-10-b39-generation-v5-checker-cutover/index.md).

### B34 — generation component records conflate executable catalogue entries with initial instances

**Status:** Resolved 2026-08-10.

**Problem:** `slime-root` constructs and activates every loadable component in
the generation, while `init` also receives those executable capabilities and
spawns the graph it owns. The full C8.10 image therefore runs a root-launched
copy and an init-spawned copy of the same fabric, workers, and participants.
The first copy has no matching spawn-time composition: its fabric service is
refused when it tries to spawn its route workers, and that graph exits nonzero.
The generation format has one `Component` record for two different concepts —
an executable available to spawn and an initial instance that must exist at
boot — and has no launch-owner or autostart field with which to distinguish
them.

**Evidence:** `just sel4_boot_check` failed on 2026-08-09. Continuing the same
image past the checker's early terminal showed root-launched fabric task 16
report `spawn refused ... ungranted`, then the root-launched graph exited with
status 1; init task 19 subsequently transferred supervision and continued a
second graph. `slime-root/src/main.rs::launch_component_graph` walks every
`Admission::loadable_plans()` entry and activates them all.

**Fix:** Introduce a clean generation-format cutover separating
`Executable` records from `Instance` records. Initial instances explicitly
declare their executable, launch owner (`root` or another instance), autostart
state, dependency barrier, health policy, quota, and capability bindings. Root
launches only root-owned autostart instances; executable catalogue entries are
inert until an authorized spawn. Do not retain a runtime v1 compatibility shim.

**Exit condition observed:** A fixture can carry executable-only images without creating
tasks; every declared initial instance is constructed exactly once by its
declared owner; the full graph contains no duplicate component identities or
unintended nonzero exits; and `just sel4_boot_check` observes the single graph's
complete healthy-idle chain. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B35 — BootState does not select the generation the seL4 product boots

**Status:** Resolved 2026-08-10.

**Problem:** The generation admitted by `slime-root` is selected at build time
with `SLIME_GENERATION` and compiled into the root ELF through `include_bytes!`.
The generation-management, rollback, and recovery planes can correctly mutate
durable BootState sectors, but the next seL4 boot never reads those sectors to
choose which generation to launch. The generation also retains a required
`kernelObject` whose seL4 payload is an inert placeholder that is validated but
never loaded.

**Evidence:** `slime-root/build.rs` states that the root task admits generation
bytes compiled into it; `scripts/build/build-sel4.py::build_application` builds
a distinct root ELF per manifest. `just sel4_generation_check` proves authority
and disk transitions but boots an image that already embeds generation 27, so
it cannot prove that the committed selection controls a later boot.

**Fix:** Add a minimal immutable seL4 boot selector that reads the
explicitly granted boot device, selects and updates the two BootState slots,
verifies release/target/generation/object closure, and launches the selected
runtime generation. Move seL4 kernel, loader, and boot-selector identity into
the signed boot bundle or release record; remove the unused generation
`kernelObject` in the same format cutover.

**Exit condition observed:** One QEMU campaign stages a pending generation, reboots into
that exact generation, durably consumes failed attempts across fresh boots,
returns to known-good when exhausted, and promotes only after health
confirmation. Changing only the root build's embedded bytes cannot satisfy the
gate. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B36 — the full-graph gate stops at a non-unique component idle marker

**Status:** Resolved 2026-08-10.

**Problem:** `check-sel4-boot-plane.py` treats the generic fabric line
`[fabric] idle: parked on control endpoints` as the whole system's terminal
marker and terminates QEMU immediately. Any fabric instance can emit it. With
B34's duplicate graph, the checker stops on the wrong instance before init's
supervision transfer, later component exits, and the actual graph outcome.

**Evidence:** Both `just sel4_boot_check` and
`python3 scripts/check/check-sel4-boot-plane.py --no-build` exited 1 with the
same missing init marker immediately after the first fabric-idle line. Manually
continuing the identical image produced the missing init marker only after the
first graph had reported multiple status-1 exits.

**Fix:** Define one supervisor-emitted terminal record binding the
generation identity or instance-set digest, required/live/idle counts, and zero
failed instances. Collect serial until that record or a failure marker; treat
every required component's nonzero exit as failure. Extend gate-control
mutations with an early duplicate fabric-idle line and a later failed instance.

**Exit condition observed:** `just sel4_boot_check` reaches the unique supervisor terminal
only after every causal chain, fails on any required nonzero exit, and the gate
control proves that an injected early component-idle line cannot truncate or
pass the check. Closing B36 by hiding B34's duplicate graph is forbidden. Audit
record: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md).

### B37 — dependency activation and non-bootstrap slot ABI are implicit contracts

**Status:** Resolved 2026-08-10.

**Problem:** Generation dependencies are decoded and structurally validated but
the seL4 launch path does not consult them; root stages component-table order and
activates every task. Actual dependency barriers live as imperative spawn/yield
sequences in `init`. Bootstrap slots have an authenticated layout resource, but
other component slots are inferred from grant iteration order, making manifest
ordering an undocumented ABI shared by the builder, root, and binaries.

**Evidence:** `boot-contracts/src/generation.rs` validates dependency bounds and
self-reference, while `launch_component_graph` uses only
`Admission::loadable_plans()`. `slime-root/src/channel.rs` documents that
non-bootstrap channels and executables take positional slots; prior Dango and
powerbox fixes already found boot/spawn and multi-kind ordering disagreements.

**Fix:** Bind dependencies and capabilities to explicit instance
records. The builder rejects cycles and unsatisfied dependency barriers, emits a
fixture-checked per-instance capability layout, and generates each component's
startup bindings from that same data. Root activates the declared DAG rather
than component-table order; grant order grants no ABI meaning.

**Exit condition observed:** Cyclic, missing, and impossible dependencies fail the build;
permuting grant declarations leaves every component's local bindings unchanged;
boot and spawn use the same generated layout; and a QEMU graph proves activation
occurs only after each declared dependency barrier. Audit and closure records: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md) and [`devlog/2026-08-10-b34-b38-model-cutover/`](../devlog/2026-08-10-b34-b38-model-cutover/index.md).

### B38 — task reclamation cannot reuse root CSlots or untyped memory

**Status:** Resolved 2026-08-10.

**Problem:** `ObjectAllocator` advances root CSlots and ordinary-untyped
watermarks monotonically. Task cleanup revokes and deletes each task's
capabilities but records those slots only as reclaimed; it returns neither slot
indices nor the task's TCB, CNode, page tables, and frames to allocatable pools.
A long-running component manager can therefore exhaust boot-lifetime resources
through repeated bounded spawn/exit cycles even when simultaneous live usage
never exceeds its generation budget.

**Evidence:** `slime-root/src/object_allocator.rs` explicitly states that slots
are never reused, and `CleanupRecord::revoke` states root CSlots are not returned
to the allocator. seL4 resets an untyped cap's free index when it has no children,
but the root does not allocate tasks from reclaimable per-task untyped subtrees.

**Fix:** Give each task or task group a derived untyped arena that owns
its CNode, TCB, VSpace objects, and ordinary frames; revoke the arena on death so
the parent can be retyped again. Add a free-list or bitmap for emptied root
CSlots. Keep device untyped and DMA ownership on their separate monotonic path.

**Exit condition observed:** A live QEMU stress graph completes more spawn/exit cycles
than the current root CSlot and untyped watermarks permit, with bounded and
stable live slot/object/byte counts, no capability alias surviving reclamation,
and successful reuse after clean exit, fault, and construction unwind. Audit
record: [`devlog/2026-08-09-b34-b38-sel4-model-audit/`](../devlog/2026-08-09-b34-b38-sel4-model-audit/index.md).

### B33 — seL4 cutover review findings

**Status:** Resolved 2026-08-09.

**Was:** The post-cutover static review recorded CUT-001 through CUT-077 across
capability isolation, lifecycle cleanup, shared-memory aliases, storage,
userspace services, gate integrity, CI/profile policy, and project records.
Several defects were merge blockers, and several gates could pass without the
current artifact or without the evidence named by their target.

**Fix:** Every finding was re-grounded and repaired. Final integration also
separated the capability-subset proof from the fabric control protocol and
dropped init's retained endpoint copies after spawn so peer-death retirement
can drain the QoS graph.

**Exit condition observed:** focused root and host tests pass; the supervision,
QoS, root-boot, gate-control, and layout-resource checks pass; formatting,
Clippy, Python lint, and dependency policy pass. See
[`devlog/2026-08-09-b33-cutover-review-remediation/`](../devlog/2026-08-09-b33-cutover-review-remediation/index.md).

### B31 — six oracle properties blocked `kernel/` deletion

**Status:** Resolved 2026-08-09.

**Was:** Two deletion audits found six acceptance properties that would have
disappeared with the frozen custom-kernel oracle, plus orchestration coupling in
the workspace, Justfile, check scripts, component transport, CI, and generation
builder.

**Resolution:** P5.4.final records each disposition. Complete component-wrapper
admission moved to `boot-contracts`; the seL4 root boot gate now observes
independent frame accounting, exact task and shared-buffer reclamation, clean
exit beside deliberate fault isolation, and panic/fault failure markers; the
global gate control proves missing, reordered, or contradictory evidence turns
every seL4 plane red. Free-frame reuse, custom EL1 mechanism, and
PMM/VMM/heap/APIC internals were reclassified where seL4 changes the mechanism.
The retired NVMe QEMU path was not promoted into false product evidence:
`storage_nvme_read_check` fails closed and M5.7 remains blocked on a seL4 NVMe
driver plus physical Framework observation.

**Exit condition observed:** `kernel/`, its workspace membership, custom-kernel
build and check orchestration, legacy component syscall transport, and custom
generation-builder path are removed together. The surviving repository gates
exercise the seL4 product or portable host contracts. See
[`devlog/2026-08-09-p5-4-final-kernel-retirement/`](../devlog/2026-08-09-p5-4-final-kernel-retirement/index.md).

### B32 — three scenario receive spins were invisible to the root

**Status:** Resolved 2026-08-09.

**Was:** The call plane's terminal receiver and two operation-plane receive
paths used `yield_now()` on `ERR_WOULDBLOCK`. `seL4_Yield` kept the components
runnable, so the root could neither name their endpoint wait nor distinguish a
real dependency from an iteration-budget spin.

**Fix:** All three now call `wait(&[WaitSource::Endpoint(...)])`. This is valid
for timeout and peer-death terminals: the brokers publish those records on the
same route endpoints. Parking exposed a pre-existing operation teardown race,
so client B now records its terminal before client A closes the backup route and
lets the broker exit. The backup probe likewise waits on the route it receives
from.

**Exit condition observed:** `just sel4_call_check` and
`just sel4_operation_check` both pass with every affected timeout, peer-death,
and unrelated-route marker present. See
[`devlog/2026-08-09-b32-parked-scenario-receivers/`](../devlog/2026-08-09-b32-parked-scenario-receivers/index.md).

### B29 — one block device per granule

**Status:** Resolved 2026-08-08.

**Was:** `slime-root` brought up at most one virtio block device. QEMU packs
eight virtio-mmio transports into one 4 KiB granule, so two attached disks land
at `0xa003e00` and `0xa003c00` — the same page — and `DeviceRegion::remap` maps
the frame to a driver's standing window, leaving nothing for the second.

**Fix:** `device::MappedGranule`, a borrowed view carrying the virtual base and
no capability. One owner maps the page; a second driver reads and writes its
registers at its own offset through the borrow, and can neither remap nor
unmap. `probe_devices` keeps a standing-granule table and `bring_up_shared_block`
brings up a transport in a page another driver already stands in.

**Exit condition observed:** `just sel4_transfer_check` boots with two disks and
records `SLIME_ROOT block ready` for both, with a component holding one
capability over each and the read-only one byte-identical afterwards.

Two further defects surfaced on the way, both now fixed and gated:

* declared placement hardcoded `Block { device: 0 }`, so a component holding two
  devices reached the same one twice — successive block grants now name
  successive devices;
* placement intersected the component's *union* of rights rather than the
  grant's own, so a read-only source came out writable and accepted a write.
  Both paths now use the grant's rights, which is what "this grant declares this
  much" means.

See
[`devlog/2026-08-08-p5-4-3-transfer-plane/`](../devlog/2026-08-08-p5-4-3-transfer-plane/index.md).


### B30 — the dango plane launched no commands

**Status:** Resolved 2026-08-08.

**Was:** Dango booted, read its scripted keystrokes, and resolved commands, but
no launch reached the spawn service.

**Three causes, none of them the hypothesis recorded when this was opened.**

1. `construct_child` never placed a child's declared **executables**. A spawned
   `spawn-service` found slots 1 and 2 empty and refused every request with
   `slot=1 ungranted`. The same defect class as P5.4.2c's missing declared
   authority, in the one resource kind that slice did not cover.
2. Declared authority was placed in a **fixed kind order**, and two components
   disagreed about it: `powerbox-chooser.rs` reads a directory then input,
   `dango.rs` reads input then a cwd root. Both placement paths now walk the
   generation's own grant order, which is what the oracle does.
3. `Resource::is_transferable` refused **endpoints** by kind, so a shell could
   not give a child its stdin. The reasoning was wrong rather than narrow: what
   bounds every move on that path is the sender holding `RIGHT_TRANSFER`, and
   the oracle's `sys_send` gates on exactly that bit with no kind predicate.

**Exit condition observed:** `just sel4_dango_check` — 14 markers, 2 profile
resolutions, 2 accepted spawn requests, `resolve-denied`, `parse-error`, and
`[dango] interactive session closed`. See
[`devlog/2026-08-08-p5-4-3-dango-plane/`](../devlog/2026-08-08-p5-4-3-dango-plane/index.md).

### B25 — a spawn-granted endpoint moves on seL4 and copies on x86, so a parent cannot broker a later introduction

**Resolved 2026-08-08.** Devlog:
[`devlog/2026-08-08-b25-endpoint-copy-call-plane/`](../devlog/2026-08-08-b25-endpoint-copy-call-plane/index.md).
Endpoint authority now carries `Side`, so a spawn grant is the same non-consuming
narrowing copy as every other grant and `ChannelTable` no longer records a
single task holder per end. Capability transit binds to the receiving *side*,
not a task selected at send time; whichever co-holder dequeues the message may
collect it, while task-naming loan creation refuses an ambiguous receiver.
Observed exit condition: `just sel4_call_check` passes 50 markers across ten
causal chains, including three parent-vouched post-spawn supervision transfers,
all C8.6 outcomes, and clean exits for the five spawned tasks plus init.
All twelve seL4 plane gates were re-run, not only the call gate: the change
rewrote marker text four of them read. `sel4_channel_check`, `sel4_loan_check`,
and `sel4_crossing_check` were red and are fixed, and
`sel4_gate_control_check`'s spawn-plane pin correctly caught a deleted
distribution assertion that had not been replaced. The crossing gate also
surfaced a root defect: `ChannelTable::live_queues` counted entries no
capability table names, which the retired per-end task cache had masked.

**Problem:** `slime-root`'s `distribute_channel_ends` (`slime-root/src/main.rs`)
treats an endpoint named by a spawn grant as a **move**: it reassigns the
channel's holder to the child and calls `table.drop_slot` on the parent's slot.
The retired kernel copies: `preflight_spawn_grant`
(`kernel/src/task/mod.rs:286`) performs `cap.derive(grant.rights)` at `:320`
into a fresh vector that `spawn_with_caps_for` (`:402`) installs into the
child, and neither reads nor mutates the parent's table — so the parent keeps
its end.

That difference is invisible to every component that hands an end away and
never touches it again — which is every component in the nine passing planes —
and fatal to any composition where a parent grants one end at spawn and then
*uses* that channel itself. The x86 call plane is exactly such a composition:
`init.rs::launch_fabric_calls` spawns `fabric-service` with all four service
halves, keeps them, and afterwards moves each participant's supervision handle
to the broker with `cap_transfer` over the matching half.

**Not a slot-numbering defect.** Two earlier versions of this entry blamed
`SlotCursors::take`'s `used_slot_zero`, first as a slot *collision* and then as
a slot *gap*. The gap is real, but it was a consequence of declaring the
control channels as **generation grants** — the root then numbers a launched
component's ends from its own cursor, which resumes above the factory grants
staging installed. Having `init` mint the pairs and hand them out at spawn
removes it, because `construct_child` installs a child's grants at `0..count`
in the requested order. Observed with the pairs minted: the fabric's four
controls arrive as `channel handed parent=5 child=6 … slot=2,3,4,5`,
contiguous above the two factory grants at the head of its grant array.

The grants themselves stay in the manifest, which the first attempt at this got
wrong by deleting them. `_control_sources`
(`scripts/build/build-generation.py:833`) derives `FABRIC_CALL_CLIENTS` — the
table the broker maps a control slot to a caller identity with — from exactly
those four grant *names*, and in `FABRIC_CALL_CONTROL_GRANTS` order rather than
the builder's `(name, source, target)` sort. Removing them emptied the table
and tripped `request_response_controls`' four-control assert before the broker
read a slot. They are the naming source; the minted endpoints are the
authority.

**Evidence:** `devlog/2026-08-07-p5-4-6-call-spawn-semantics/`. With the plane
rebuilt to mint its control pairs, the boot reaches
`SLIME_GRAPH channel handed parent=5 child=6 key=4 slot=2` — the fabric's end
arriving *and* init's slot being dropped in one step. Every participant's role
request then reaches the broker (`SLIME_GRAPH received task=4 channel=2`) and
is never answered: `Broker::provision` blocks in `consume_supervision` awaiting
a handle no one on this plane can send, and the graph ends `live=10`,
`parked=8`, `transfers served=0`.

The obvious alternative — each participant sending a handle naming itself — is
not constructible. `serve_spawn` installs a supervision handle only into the
**parent's** table, and only after `construct_child` has built the child's
(`slime-root/src/main.rs:3586-3603`), so no component ever holds a handle
naming itself.

**Narrowed by experiment, 2026-08-07.** Inverting the call plane's spawn order
*does* carry the supervision handoff, so the endpoint-move semantics alone are
not the whole blocker. Spawning the participants first with the *participant*
half of each control pair, keeping the *service* half in init, transferring each
participant's handle over it, and spawning the fabric last with the service
halves reached `[init] call supervision delegated` — the step this entry was
filed for. Both halves of a pair are still granted exactly once, so no
`drop_slot` takes anything init needs later.

What that order cannot then deliver is the **fabric's own** handle, and for a
second, independent reason. Two participants lend to the broker
(`fabric_call_scenario`'s `send_large_request` and `send_large_reply`), so both
need a `RIGHT_SUPERVISE` capability naming the fabric at their
`FABRIC_SUPERVISION_SLOT`. A *spawn grant* copies (`preflight_spawn_grants`
installs `held.resource` and leaves the parent's slot), which is how
`drive_sample_plane` hands one handle to a lender — but it requires the fabric
to exist first, which is the order this experiment inverted. A *transfer* moves
(`serve_cap_transfer` calls `table.drop_slot` on the source), and
`FLAG_RETAIN_TRANSFER` keeps the delegation bit at the destination without
making the move a copy — so one handle reaches one receiver. Init cannot obtain
a second, because `bootstrap_executable_slot` resolves an executable by
component identity to exactly one slot and each spawn returns one handle.

So the two requirements are order-incompatible as the components are written:
the control ends want the fabric spawned last, the fabric handle wants it
spawned first. That is a sharper statement than "the grant moves", and it means
the fix is still a model decision rather than a composition detail. Observed
directly; the experiment was reverted and the tree is back to the committed
plane.

**Severity:** Latent for every current plane, and a hard blocker for any plane
whose parent must broker an introduction after spawning. It is a genuine
*semantic* divergence from the frozen oracle, not a numbering accident, so it
cannot be resolved by re-blessing a fixture.

**Proposed fix:** Decide which semantics the model wants and make both
implementations agree, rather than working around it per plane. A copy matches
the oracle and keeps `init.rs` portable, but it means two tasks name one
channel end and `ChannelTable` resolves queues by holder — so the copy needs a
holder model that admits more than one. A move is the cheaper invariant and is
arguably the more capability-honest one, but then the oracle's own call plane
is not portable as written and `launch_fabric_calls` needs restructuring.

The experiment above adds a third option, cheaper than either and worth
weighing first: let a component obtain a **second** handle naming a task it
already supervises, so a broker's handle can reach both of its lenders without
the fabric having to be spawned before the participants. The narrow form is a
`supervision_derive`-style operation returning a fresh capability naming the
same task, which is a copy of authority the caller already holds and widens
nothing. That would make the inverted order carry the whole plane, leaving the
endpoint move/copy question a real but no longer blocking difference.

**Third option implemented, 2026-08-07.** `supervision_derive` (operation 32)
exists and is gated. A caller holding `RIGHT_SUPERVISE` on a supervision handle
receives a second capability naming the same task at the same rights, in a fresh
slot, keeping the source. Root side is `serve_supervision_derive`
(`slime-root/src/main.rs`); the ABI is mirrored in both component transports.

It widens nothing by construction — same task, same rights, `RIGHT_SUPERVISE`
required to ask — so it cannot mint authority the caller could not already have
transferred. `graph::holds_supervision` already scanned every live table for *any*
holder, because a handle has always been movable, so reclamation needed no change.

**Observed on the supervision plane**, which is the one plane where init holds a
handle it has not yet given away:
`SLIME_GRAPH supervision derived task=0 child=3 slot=5`, then the *derived* handle
answers the child's outcome, then the source is still intact for the existing
transit transfer. Both markers are gated in
`check-sel4-supervision-plane.py`. Two fault injections confirmed: returning the
source slot instead of a new one, and installing the derived handle with no
rights, each trip a distinct component assertion. A third — dropping the
`RIGHT_SUPERVISE` gate — is **not** covered, because every caller on this plane
holds that right; recorded rather than claimed.

**This does not yet close B25**, and investigating the call plane afterwards found
that B25 is no longer what stops it.

**The supervision grant already works.** `launch_fabric_calls` grants
`service.supervision_slot` to *both* `fabric-call-client` and
`fabric-call-server` (`init.rs:841` and `:860` — the same slot, twice), and the
boot shows all five components spawning. So a *supervision* spawn grant copies:
`distribute_channel_ends`' move applies to channel ends only. B25's two blocking
reasons were both about supervision handles, and neither is what the plane hits.

**What the plane actually hits is a missing component, not a missing operation.**
The boot reaches `[init] call participants spawned` and then dies with
`[fabric-call] fail: time phase receive`. `fabric-call-time` waits on
`recv(1, …)` for a phase byte, and *nothing on this plane sends one*: only
`fabric_operation_scenario.rs` has a time-phase publisher (`PHASE_TIME_SLOT = 2`,
`send` at `:648`), while `fabric_call_scenario.rs` has only a **client** phase
channel (`CLIENT_PHASE_SLOT = 1`). There is no time-phase sender in the call
scenario at all.

A contributing defect was found and fixed-then-reverted along the way:
`init.rs` grants `FABRIC_CALL_PHASE_TIME_SLOT` to `fabric-call-time`, and that
constant is `SLOT_ABSENT` (`u32::MAX`) because `sel4-call.zti` declares no phase
grants. So the component was handed a slot naming nothing. Minting the pair in
`init` and granting the service half to the fabric was written, built, and booted
— and the plane *still* fails identically, because plumbing a channel does not
create the publisher that was never written. The change was reverted rather than
committed as a partial fix that changes no observable outcome.

**Fixed, and the failure is gone.** `fabric-call-time`'s own comment already said
"no phase channel in the boot layout" and it already had a `park_only` path for
exactly this — but the guard was `fabric_boot::active()`, which keys on
`SLIME_FABRIC_BOOT_CHECK`. The x86 boot generation sets that; the seL4 call plane
does not, so the component took the phase path on a plane with no phase publisher.

The component now also parks when `FABRIC_CALL_PHASE_TIME_SLOT == SLOT_ABSENT`,
read from the generated boot layout it already includes. Testing the *slot* rather
than adding a second flag is deliberate: the condition that matters is whether a
phase channel exists, the layout already answers that, and a flag would have to be
kept in step with every future generation.

Observed: `[fabric-call] fail: time phase receive` is gone and replaced by
`[fabric-call-time] boot idle without a role`. All eight other plane gates re-run
green.

**The plane still does not complete, and the remaining gap is now located exactly.**
It wedged with no component failure — `graph iterations exhausted live=11 parked=9`.
Tracing it:

* The broker is task 6. It received once on channel 4 and then went silent — it
  never replied and never parked, because
  `call_broker.rs::consume_supervision`'s `ERR_WOULDBLOCK` arm was `yield_now()`,
  which is `seL4_Yield` and invisible to the root. **Fixed:** that arm now parks
  with `wait(&[WaitSource::Endpoint(control)])`, matching `consume_request` in the
  same file and `operation_broker.rs::consume_supervision`, both of which already
  parked — this was the one arm that did not. The plane now reports
  `parked task=6 reason=wait` and reaches a genuine all-parked deadlock instead of
  burning the root's iteration budget, which is a strictly better failure: the
  root's accounting can name the waiter. All eight other plane gates re-run green.
* `consume_supervision` waits for a descriptor carrying a `RIGHT_SUPERVISE`
  handle naming the *participant*, on that participant's control channel.
* **Nothing on this plane sends one.** `drive_call_plane` (the seL4 path;
  `launch_fabric_calls` is the x86 one, keyed on a different flag) never calls
  `transfer_supervision` at all. Its own comment at `init.rs:1901` describes the
  intended cut — "each participant delivers its **own** handle over its own
  control channel, as its first act" — but the grants below it hand each
  participant a handle naming the **fabric** (`init.rs:1917`), never one naming
  itself. So the plan in the comment was never implemented, and it *cannot* be as
  written: `serve_spawn` installs a supervision handle only into the **parent's**
  table, so no component ever holds one naming itself.

**And `supervision_derive` does *not* close it**, which is worth stating plainly
after adding the operation: the derive copies a handle the **caller** holds, and
what is missing here is a handle naming the **participant itself**, held by that
participant. Init holds one naming each participant, but init has no channel left
to the fabric — the endpoint grant moved every service half away at spawn.

Traced to the exact shape the broker expects: `consume_request` then
`consume_supervision` on the **same** control channel
(`call_broker.rs:273-275`). The participant already holds that channel
bidirectionally (`RIGHT_SEND | RIGHT_RECV`, `init.rs:1915`) and sends the request
over it, so the channel is not the obstacle. The obstacle is that no component can
obtain a supervision capability naming *itself*: `serve_spawn` installs one only
into the parent's table.

**So the options narrow to two, and both are real design choices rather than
plumbing:**

1. Let a spawn place a self-naming supervision handle in the *child*, so a
   participant can present its own identity. That is a new authority shape —
   a component holding a handle to itself — and needs its own argument about what
   it permits (`supervision_status` on oneself, notably).
2. Keep an endpoint grant from moving for this one case, so init retains a service
   half and can deliver the derived handles itself. That is the original move/copy
   divergence, and it is where B25 started.

The derive is still the right operation to have — it is what makes option 2 a
two-line change once the endpoint question is settled, because init can then hand
the same participant handle to the fabric *and* keep its own for the termination
wait. But B25's core question is unavoidable, and it is a model decision the way
the entry always said.

**A third route was looked for and does not exist.** The obvious workaround is for
init to mint a *fifth* pair as a private delegation channel — grant one half to the
fabric, keep the other, and deliver the derived handles over it. That fails on a
stated constraint rather than a mechanism: the broker reads each participant's
supervision handle from `client_control[index]`
(`call_broker.rs:273-275`), the participant's *own* control channel, and
`init.rs:1907` records that `consume_supervision` "cannot tell the two paths
apart, which is what keeps the broker unmodified". Routing delegation over a
different channel means changing the broker, and an altered broker is no longer
the same composition the oracle's gate asserts — which is the property P5.4 exists
to preserve.

**Sizing the two real options, so the decision is informed. Re-examined
2026-08-08, and both earlier estimates were wrong in the same direction — they
priced the shallow form of option 1 and the shallow objection to option 2.**

* *Copying endpoint grant.* The earlier sizing — "change `producer`/`consumer`
  and every path that reads it, the widest change of the three" — prices only the
  **shallow** form, where `Entry` grows a holder list. That form is as bad as
  stated and worse: `mark_dead` (`channel.rs:378`) would have to become a
  refcount, matching what the oracle gets for free from
  `Arc::strong_count(&owner_alive) == 2` (`kernel/src/ipc/mod.rs:166`), and
  `peer` (`channel.rs:362-369`) would return a *set*, which `Transit`'s
  send-time receiver binding (`transit.rs:62-65`) cannot consume.

  There is a **deeper** form that is cheaper than either option, and it is the
  one to weigh: put the *side* in the capability —
  `Resource::Endpoint { channel, side }` — and resolve queues by side rather
  than by task. Then `distribute_channel_ends` is deleted outright rather than
  inverted, because a granted endpoint becomes an ordinary copy alongside every
  other kind (`preflight_spawn_grants:3207` already copies; endpoints are the
  one kind singled out for a move), and `Entry::producer`/`consumer` are deleted
  with it. That is not a new representation but the *removal* of one: the field
  doc at `channel.rs:552-556` already states these are "a cache of who holds
  each end, maintained by `ChannelTable::reassign` with no capability check of
  its own", and the only reason the cache exists is that the capability does not
  say which end it is. Holder questions then become table scans of the shape
  `channel::sweep` (`channel.rs:574`) already performs, bounded by
  `MAX_TASKS * MAX_TASK_CAPS` = 32 × 64 on cold paths.

  Two things this form must still answer, neither of them the representation:
  `Transit` binds an in-flight capability to a receiver *task* at send time and
  would bind to a *side* instead; and the declared self-edge
  (`check-sel4-channel-plane.py:113-116` — `queues=1`, "init holds both
  directions at one slot") needs a side that means *both*, because `materialize`
  installs exactly one slot for a loopback (`channel.rs:806`).

* *Self-naming supervision handle at spawn.* Mechanically small, as recorded.
  But the semantic objection recorded here — that it "makes `supervision_status`
  on oneself reachable" — is the weak one, and it is not what should decide.
  Asking one's own status can only ever answer `WouldBlock`, a task can already
  deadlock itself on a loopback channel, and `serve_buffer_loan` already refuses
  a self-loan at `main.rs:4853`.

  **The real objection is that it moves who vouches for an identity, and it
  degrades the oracle.** `consume_supervision` (`call_broker.rs:1146-1157`)
  checks magic, version, `object_kind`, `direction`, `rights_mask`, and
  `route_identity` — it cannot check *which task* the handle names. Today the
  broker is trusting the parent's introduction, because init is the sender. Under
  option 1 that stays true. Under option 2 the participant vouches for itself,
  and it is holding a second `RIGHT_SUPERVISE` handle naming the **fabric**
  (`init.rs:1917`), which satisfies every field the broker checks. A participant
  sending *that* one makes the broker treat the fabric as the loan receiver.
  `slime-root` happens to catch the result at `main.rs:4853` (`peer == id`);
  **the oracle does not** — neither `sys_shared_buffer_loan`
  (`kernel/src/syscall/mod.rs:786-799`) nor `SharedBufferTable::loan`
  (`kernel/src/memory/shared_buffer.rs:296-342`) compares lender against
  receiver. So option 2 opens a hole on the frozen side to unblock a plane on
  this one.

**And there is no "parent keeps a third end" route, for an arithmetic reason
worth stating so it is not re-attempted.** `endpoint_create` installs exactly
two slots (`main.rs:1790-1806`), and the first grant of a minted loopback always
moves the *consumer* side whichever slot named it (`channel.rs:426-435`). Three
holders of a two-slot pair is therefore unconstructible, and the x86 plane's
shape is exactly three: init's layout carries both
`fabric-call-client-control` and `...-control-service`
(`contracts/boot-layout/v1/fixtures/fabric-call.layout:53-56`), init grants the
client half at `init.rs:839`, sends the descriptor over the *same* slot at
`init.rs:883`, and only then drops it at `:884`. Whatever closes this must let
one end have two holders, or let a participant name itself — the two options
above and nothing else.

One further constraint on any re-transfer variant: the participants' executable
grants are declared `transferable = false` (`sel4-call.zti:95,102,109`), so the
supervision handle a participant spawn returns carries no `RIGHT_TRANSFER`
(`main.rs:3762`) and cannot be `cap_transfer`ed at all without a fixture change.

**Option 1 was built as a spike, 2026-08-08, and it gets further than the entry
expected before hitting one wall that is not a plumbing detail.** Reverted; the
tree is back to the committed planes. What it established, all observed:

* *`Side` in the capability works, and the deletion is real.*
  `Resource::Endpoint { channel, side }` with `Side::{Producer,Consumer,Loopback}`
  let `distribute_channel_ends`, `recall_channel_ends`, `ChannelTable::reassign`,
  and `Entry::{producer,consumer}` all be **deleted**, and
  `restore_transferred`'s `reassigned` rollback argument with them. An endpoint
  grant became an ordinary copy beside every other kind. Total footprint was five
  files: `slime-root/src/{channel,graph,main,transit}.rs` plus the two spawn-plane
  assertions below.
* *Holder questions are answerable from the graph.* `mark_dead` became a per-*side*
  abandonment query (`holds_endpoint_side(key, side, except)`), and the
  `peer death channels=N` marker's count became `CapabilityTable::endpoints_held`.
  No refcount was needed, contradicting this entry's earlier sizing.
* *`just sel4_channel_check` and `just sel4_spawn_check` pass*, as do
  `sel4_root_boot_check`, `sel4_component_graph_check`, and `sel4_loan_check`.
  Host tests went 109 → 112.
* *Two spawn-plane assertions had to be inverted, and they are the honest cost.*
  `init.rs:2085` asserted `send` on a granted end answers `ERR_BAD_CAP`, and
  `:2159` asserted it for all six B15 grants; `check-sel4-spawn-plane.py` asserted
  the `channel handed` marker. All three assert the *move*, so option 1 makes them
  false by construction — they encode the divergence rather than a property the
  oracle shares, and `devlog/2026-08-05-p5-3-3-spawn-plane/index.md:283` lists
  "make the endpoint grant a copy" as an intended fault injection.
* *A new test pins B25's actual property* — an end with two holders survives one
  holder dying — and fault-injecting the pre-B25 `mark_dead` fails exactly that
  test and nothing else.

**The wall: `Transit` binds an in-flight capability to a receiver *task*, and with
two holders per end there is no longer a unique one.** `just sel4_sample_check`
wedges: `parked task=0 reason=wait` / `parked task=3 reason=wait`, boot exceeds
180s. `drive_sample_plane` mints a pair, grants the consumer half to
`sample-receiver` and the producer half to `sample-lender`, and — now that a grant
copies — init keeps **both**. `serve_send` resolves the loan's destination through
`channels.peer(channel, id)`, which became "the first live holder of the opposite
side", and init is enumerated before the child. So `transit.depart` binds the loan
to init, `land_caps` calls `transit.arrive(token, receiver)`, that returns `None`,
and the receiver parks forever on a capability delivered to the wrong task.

This is not a bug in the spike; it is the model question the spike surfaces, and it
was written into `channel::peer_of`'s doc before the plane was run. A capability
naming a *queue* cannot name a *recipient*, and message-carried capability transfer
needs a recipient. Two ways out, neither attempted:

1. *Bind transit to a side rather than a task*, and have `arrive` admit any holder
   of that side. Then delivery is first-come between co-holders — fine for the
   x86 call plane, where init sends and only the broker receives, but it makes
   "who gets this capability" depend on scheduling wherever an end really is
   shared.
2. *Make the capability name its recipient*, the way `Resource::Loan`'s
   `LoanHandle` already does (`main.rs:4616` refuses a loan sent to anyone but its
   declared receiver). That is the principled answer and the larger change.

So option 1's cost is now known concretely: the deletions are as clean as hoped,
the passing planes survive, and what remains is *one* genuine design decision about
transit binding — not the wide representation change this entry originally priced.
Neither option is written, because both change what a capability *means* on every
plane and that is a decision to take deliberately rather than as a side effect of
unblocking one gate.

**Exit condition:** A parent grants one end of a minted pair at spawn, uses the
other end afterwards to deliver a capability to that child, and the child
observes it — asserted on a plane that declares such a composition, with a
fault injection showing the parent's end going missing is caught. The call
plane's `[init] call supervision delegated` marker is that composition, already
observed; what remains is for the plane to get past it.

### B28 — a `retained` second route on one publisher stops a *different* publisher's parked role reply from ever being taken

**Resolved 2026-08-07.** Devlog: [`devlog/2026-08-07-b28-iteration-budget/`](../devlog/2026-08-07-b28-iteration-budget/index.md).
The cause was `MAX_GRAPH_ITERATIONS = 512`: the QoS plane needs more than 512 and
fewer than 768 root round-trips, measured by bisection. No wake was lost, no
capability was stale, no scheduler was inconsistent, and every component was
correct. Bound raised to 2048 with the measurement recorded. Observed exit
condition: `just sel4_qos_check` passes with fourteen markers across nine causal
chains, and restoring 512 makes it red on its own `wedged waiter` signature.

**Problem:** On the P5.4.5 QoS plane, `fabric-publisher` parks once in `recv`
waiting for its role reply and never runs again, although the fabric delivers
both role capabilities to it — the transcript carries
`SLIME_GRAPH capability transferred task=9 channel=5 to=10 kind=endpoint` twice,
and `serve_cap_transfer` calls `deliver_wake` for each. It produces *zero*
further log lines and is still live at teardown, so the plane never reaches
`[init] fabric stream complete`.

**Bisected to one fixture field.** The trigger is `fabric-publisher-b`'s
*diagnostics* participant being `durability = retained` with
`retainedDepth = 2`. Flipping that one participant back to `volatile`/`0` and
rebuilding, with nothing else changed, makes `fabric-publisher` wake and print
`publish role received`. Flipping it to `retained` makes it park forever. The
affected task is a different component on a different route, which is what makes
this a defect rather than a scenario limitation.

**Two earlier readings, both ruled out by experiment.**

* *Starvation behind the clock driver.* `fabric-publisher-b` performs seven
  `advance_time`/`await_time_credit` round-trips, each re-waking the fabric, so
  the obvious reading was that task 10 is woken and never selected. Reducing the
  advance to a **single** step changes nothing — both transfers still land, the
  task still parks once, and it still never runs. Clock volume is not the
  variable.
* *Slow progress.* Extending the boot window from 200s to 700s changes nothing.

**Evidence:** `devlog/2026-08-07-p5-4-5-qos-clock/boot.log` for the retained
case. The stream plane, which is the same graph without the clock or the retained
diagnostics route, runs a byte-comparable transfer sequence and wakes the same
task at the same point.

**Not diagnosed to a line**, but the search is narrowed. What a second retained
route changes inside `fabric-service` is the untraced step: it adds a retained
history the broker maintains, and `create_late_subscriber` now finds a satisfying
publisher where it previously failed — so the broker takes a path it did not take
before, between the transfer and the point where the parked task would be served.

Two resource-exhaustion readings are also ruled out, so a bound is not the cause:

* *`retainedSamples` too small.* The graph declares `2` while two publishers now
  retain depth 2 each, which looks like the obvious ceiling. Raising it to `4` and
  rebuilding changes nothing — the task still parks forever.
* *Frame-table exhaustion.* `FABRIC_FRAME_CAPACITY` is 32 against a retained
  demand of 4, and the transcript carries no frame-exhaustion marker.

A fifth reading is ruled out too, and it moves the suspicion off the broker: the
late-subscriber path **works**. With the diagnostics route retained the transcript
carries `retained history offered to late subscriber`,
`retained history replayed to late subscriber`, and
`retained history expired for late subscriber` in order — and that replay is what
produces the `QoS lifespan expired` arm. The fabric's capability slots peak at 23
of 32, so it is not out of slots either. The broker is healthy and simply parks on
its stream sources with `fabric-publisher`'s request never served.

**The reply is not lost.** `ParkedReplies` is now instrumented: the root emits
`SLIME_GRAPH replies owed count=` and one `reply owed task=` per still-parked task
at teardown, and only when the set is non-empty, so every healthy plane gains no
line. On this boot the answer is a single owed reply belonging to task **6**,
which is `init` waiting on its children — expected and correct. `fabric-publisher`
is **not** in the list, although the transcript shows `parked task=10 reason=wait`
and no later activity from it.

So its wake *was* delivered: 33 park events across the boot, one owed at
teardown. The task resumed, consumed its reply, and then blocked inside seL4
without issuing another root call — which is why it emits nothing further and why
no root-side accounting shows it as outstanding. That excludes every lost-wake and
lost-reply reading, including the two this entry previously carried.

**The precise state, from the root's own accounting.** `parked=1` at teardown and
the owed list names task 6 alone, so task 10 left the parked table — it was woken.
It then issued **no further root call at all**: the transcript carries zero
`received task=10` lines after the wake, and `recv` is the only thing
`receive_role` does between waking and returning.

That is the contradiction to resolve. `receive_role` loops `recv` then
`wait(Endpoint(CONTROL_SLOT))`, both root operations, so a woken task must either
call `recv` again or park again. Task 10 does neither, and it prints nothing — not
even `publish role received`, which is the next statement after the two-capability
loop completes. A task that returns from `wait` and then makes no syscall is
either faulting silently or looping in userspace on a path with no root call in
it.

**The fault check is done and the path is found.** No fault marker appears — the
root reports them (`SLIME_GRAPH component fault`) — so task 10 did not fault. And
there *is* a userspace loop with no root-visible call in it, in the runtime rather
than the component:

`sel4_transport::wait` (`components/runtime/src/syscall/sel4_transport.rs:264`)
stages its source set through the transfer window, and on a staging failure it
calls `yield_now()` and **returns silently** — no `SYS_WAIT`, no error to the
caller, because `wait` returns `()`. `yield_now` is `sel4::r#yield()`, a kernel
primitive that never reaches the root. `receive_role` then loops back to `recv`,
and a caller that keeps failing to stage spins between the two forever while the
root sees nothing at all. That is exactly task 10's signature: woken, no further
root call, no fault, no output.

The comment there says "the caller re-polls either way", which is true only if the
next poll can succeed. When it cannot, the silent return converts a bounded error
into an invisible hang — and `wait`'s `()` return type is what makes it
unreportable.

**That arm is not the cause — refuted by instrumentation.** A temporary
`debug_write` on the staging-failure branch, rebuilt and booted, produces **zero**
lines. `wait` stages successfully every time on this plane, so the silent-yield
path is never taken and task 10 is not spinning there.

The park accounting is also self-consistent, which removes the last root-side
suspicion: 33 park events, one owed reply at teardown (task 6), and task 10 never
appears in a reclaim or peer-death line. Its park entry was therefore *consumed by
a wake* rather than abandoned. It resumed, and then made no root call by any path
the root or the runtime can report.

Seven readings are now excluded: lost wake, lost reply, starvation, clock volume,
boot duration, the `retainedSamples` bound, the frame table, a component fault, and
the runtime's silent-yield arm.

**Localized to the first `receive_role` iteration.** A marker compiled into
`fabric-publisher` between `role requested` and its two-capability loop prints
`awaiting role cap` and then nothing: the task blocks inside the *first* iteration,
never reaching `role cap arrived`. It is not stuck on the second capability, and it
is not past the loop.

The wiring is right, which is what makes this narrow. Task 10 holds the control
channel at the slot it reads — `channel handed parent=6 child=10 key=5 slot=0`
against `CONTROL_SLOT = 0` — and the fabric transferred both capabilities to that
exact channel (`capability transferred task=9 channel=5 to=10`, twice, rights
`0x1` then `0x2`). So the transfers targeted the queue the receiver polls, the
receiver polled it, and it saw nothing.

That points at `serve_cap_transfer`'s enqueue-plus-wake against a receiver that is
parked *at that moment*: the fabric's two transfers land back to back while task 10
is parked from its `wait`, and the second finds `deliver_wake` a no-op because the
first already un-parked it — but the first wake races the enqueue of the second
capability. On the stream plane the same pair lands and the receiver drains both,
so the ordering that breaks is graph-dependent, which is consistent with the
retained bisect.

That ordering was then read, and it is **correct**, so this reading is refuted too.
`Channel::commit_send` enqueues and `take()`s `recv_waiter`, so the first transfer
carries the wake and the second correctly returns `None`; the receiver is expected
to drain both messages once awake. The transcript confirms the order is favourable:
`parked task=10 reason=wait` precedes both transfers, so `deliver_wake`'s
`parked.reason(task).is_none()` guard cannot have skipped the first wake.

So: the receiver parks, two messages are enqueued on the queue it polls, the wake
is delivered to a task the root agrees is parked, its park entry is consumed, and
it never runs again. Every step is individually correct and the composition
deadlocks.

Two further readings were tested and refuted, which is worth recording because both
look compelling from the transcript:

* *Both ends of the loopback given away.* Init mints `key=5` as a loopback and the
  log shows it handed to child 9 *and* child 10, so init keeps neither end — which
  would leave the queue's `producer`/`consumer` naming only the two children. That
  is exactly what `reassign`'s loopback split is for, and the **stream plane does
  the identical thing** (`channel handed parent=6 child=9 key=5` then
  `… child=10 key=5`, same line shape, same order) and drains both messages. Not
  the cause.
* *Round-robin starvation.* On the stream plane task 10 runs only after the fabric
  parks and every other task blocks, so it is plainly last in the queue — and on
  the QoS plane the clock keeps the fabric busy. But the QoS fabric still parks
  **eight** times after the transfers, so task 10 has scheduling opportunities and
  does not take them. Not the cause either.

Ten readings excluded from the boot log. Both planes are byte-comparable through the
park and the two transfers, the rights on the control end are `send|recv` so
`WAIT_KIND_ENDPOINT` resolves, and every root-side structure reports consistent
state.

**The debugger settles it.** Booting under `-gdb tcp::1234`, letting the plane reach
its deadlock, and attaching `lldb` shows the CPU parked at `0x8060011190`, inside a
`b .` self-loop. Resolving that against the kernel's symbol table puts it in
`idle_thread` (`0x806001118c`, the symbol immediately below). **seL4 has no runnable
thread at all** — so task 10 is not spinning in userspace and not starving behind a
peer: it is blocked in the kernel on an endpoint nothing will signal.

That inverts the remaining suspicion back onto the root, with one specific
candidate. `parked::send_reply` ends with `slot.cap().send(info)` and **discards the
result**. Every accounting structure the root keeps is updated as though the reply
was delivered — the entry is removed from `ParkedReplies`, `recycled` is bumped, the
`reply owed` list is correspondingly empty for task 10 — while an `seL4_Send` that
failed would leave the child blocked forever with no trace anywhere. That is exactly
the observed combination: consistent root bookkeeping, an idle CPU, and a child that
never resumes.

**The send is not the loss either.** `sel4::cap::Unspecified::send` returns `()` —
seL4's `Send` reports nothing, so there was no discarded error to find. Bracketing
the call with markers shows `SLIME_DBG wake replying task=10` followed by
`wake replied task=10`: the root reaches the send, performs it, and returns. The
same bracket fires and *works* for tasks 4, 5, 7, 8, and 9 in the same boot, so the
save/park/wake/reply path is sound in general.

**So the defect is narrower than any structure the root can inspect.** Task 10's
reply is sent over its saved capability, the send returns, the CPU then goes idle
with no runnable thread, and the child never resumes. Every layer reports success
and the thread stays blocked. Eleven readings are now excluded, including the
debugger-motivated one.

**The reply capability is live — measured, not assumed.** `KernelDebugBuild` is
already `ON` in `sel4/config/qemu-arm-virt.cmake`, so `seL4_DebugCapIdentify` is
available; `Cap::debug_identify` was called on the saved slot immediately before the
send. Task 10's slot reports `kind=8`, which is `cap_reply_cap` in
`build/sel4-qemu/generated/arch/object/structures_gen.h:635` — and it is the *same*
kind reported for tasks 4, 5, 7, 8, 9, 11, and 12, every one of which wakes
correctly in the same boot.

So every root-side link is now measured and sound: the task parks, its reply is
saved as a genuine `cap_reply_cap`, two messages are enqueued on the queue it polls,
`deliver_wake` fires while the root agrees it is parked, the send is performed over a
capability the kernel confirms is a live reply cap, and the send returns. The CPU
then idles with no runnable thread and the child never resumes. **Twelve readings
excluded.**

**The kernel's own scheduler state confirms a true deadlock, not starvation.**
Reading it through the gdbstub with the kernel ELF loaded as a symbol target:

* `ksCurThread = 0x8060030c00`, the idle TCB — matching the `idle_thread` PC.
* `ksSchedulerAction = 0`, i.e. `SchedulerAction_ResumeCurrentThread`: the kernel
  has decided there is nothing to switch to.
* `ksReadyQueues[0]`, `[1]`, `[254]`, and `[255]` all have `head = NULL`. Priority
  254 is `CHILD_PRIORITY` and 255 is the root's, so **no thread at any priority is
  runnable**.

That closes the starvation question for good: every thread in the graph is blocked,
including the root. It also means the missing wake is not a scheduling artifact — a
runnable-but-never-selected thread would sit in `ksReadyQueues[254]`, and it does
not.

So the state is fully characterized and internally contradictory at the seL4
boundary: the root sent a reply over a capability the kernel identifies as a live
`cap_reply_cap` naming a blocked thread, and that thread did not become runnable.
Thirteen readings excluded.

**A TCB state read deepens the contradiction rather than resolving it.**
`ksDebugTCBs` (the kernel's debug thread list, available because
`KernelDebugBuild` is on) heads at `0x80604f6c00`, and `tcbState` is the first field
of `tcb_t`. Reading it there gives word 0 = `0x1`, which is
`ThreadState_Running` in `deps/sel4/include/object/structures.h:160`.

So at the deadlock there is a thread the kernel considers **Running** while
`ksReadyQueues` is empty at every priority and `ksCurThread` is the idle TCB. Those
three facts cannot all be consistent with a healthy scheduler: a Running thread
belongs in a ready queue or is current, and this one is neither.

That is the sharpest statement available and it is worth stopping on rather than
guessing past. Fourteen readings excluded. Two candidates remain, and they are
different bugs:

* the thread was made Running and then never enqueued — a missing
  `SCHED_ENQUEUE`, which on this path would be inside seL4's own reply handling;
* or the TCB at the head of `ksDebugTCBs` is not the thread this concerns, and the
  Running state belongs to something else entirely — in which case walking
  `tcbDebugNext` to identify each thread is the remaining read.

**The second candidate is now settled: it is a real child thread.** This build has
no `tcbDebugNext` field, so `ksDebugTCBs` is not a walkable list here — but the TCB it
points at can be identified directly. At `0x80604f6c00`, `tcbPriority` (offset 920)
reads **254**, which is `task::CHILD_PRIORITY`. The idle thread is a different object
at `0x8060270000`-ish with `tcbPriority = 0`. So the Running TCB is one of the
graph's own components, not the idle thread and not the root.

**The inconsistency is therefore confirmed at the kernel level:** a child thread at
priority 254 in `ThreadState_Running`, absent from `ksReadyQueues[254]` (and from
every other priority's queue), while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. A Running thread that is neither
current nor enqueued cannot be scheduled again, which is exactly the observed hang.

Fifteen readings excluded, and the defect is now located to one transition rather
than a subsystem: something set a child's state to Running without enqueuing it, on
the path a reply to a parked task takes. That is either seL4's own
`setThreadState`/`possibleSwitchTo` sequence for a reply-send to a thread blocked in
`Recv`, or a root-side invocation that leaves the thread in Running without the
kernel completing the switch.

**Thread naming was tried and does not help on this build.** `seL4_DebugNameThread`
is exposed by `rust-sel4` as `cap::Tcb::debug_name`, and calling it at spawn compiles
and boots cleanly — but this kernel has no `tcbName` field at all, so nothing stores
the label and no dump can report it. `KernelDebugBuild ON` gives `DebugCapIdentify`
and the `ksDebugTCBs` pointer without the naming storage that
`CONFIG_DEBUG_BUILD`'s thread-name support would add. The change was reverted rather
than left as a call whose effect is unobservable.

**The thread is identified: it is task 10, `fabric-publisher` itself.** Matching was
done through the IPC buffer rather than the VSpace, because the root already prints
the derived address. The Running TCB reports
`tcbIPCBuffer = 0x237000`; `child_vspace.rs` sets `ipc_buffer_addr = footprint.end`
and places the transfer window one page above it, so that TCB's window is
`0x238000` — and the transcript's `window bound task=10 base=0x238000` names exactly
one spawned task with that address. Task 10 is the `fabric-publisher` instance init
spawned, which is the thread that never wakes.

Its saved context is consistent with a live component rather than a fresh one:
`registers[31]` (PC) is `0x2366f0`, far above the `entry=0x211e78`
`fabric-publisher` was started at.

**So the defect is now stated exactly.** `fabric-publisher`'s thread is in
`ThreadState_Running` with a plausible mid-execution PC, absent from
`ksReadyQueues` at every priority, while `ksCurThread` is the idle TCB and
`ksSchedulerAction` is `ResumeCurrentThread`. The root has sent it a reply over a
capability the kernel identifies as a live `cap_reply_cap`. Sixteen readings
excluded; every layer above the scheduler checks out.

**The kernel's reply path was read and it explains the state without being wrong.**
Non-MCS `doReplyTransfer` (`deps/sel4/src/kernel/thread.c:133`) opens with
`assert(thread_state_get_tsType(receiver->tcbState) == ThreadState_BlockedOnReply)`.
On success it does `cteDeleteOne(slot)`, `setThreadState(receiver, Running)`, then
`possibleSwitchTo(receiver)` — so the enqueue is not missing from the kernel.

`possibleSwitchTo` is where a Running thread can legitimately end up in no queue:
when the target shares the current domain and `ksSchedulerAction` is
`ResumeCurrentThread`, it takes neither `SCHED_ENQUEUE` branch and instead sets
`ksSchedulerAction = target` — a *pending switch* held outside the ready queues.
`schedule()` consumes that correctly, so the design is sound; but the measured state
at the deadlock is `ksSchedulerAction = 0` (`ResumeCurrentThread`) with the target
Running and unqueued, which is that pending switch having been **cleared without
being honoured**.

**So the shape of the bug is now pinned even though the culprit is not.** Something
between `possibleSwitchTo` recording the switch and `schedule()` acting on it reset
`ksSchedulerAction` to `ResumeCurrentThread` — plausibly a second
`possibleSwitchTo`/`rescheduleRequired` interleaving from another root operation in
the same kernel entry, which the root's single-threaded dispatch makes possible when
one syscall replies to two different tasks. That is consistent with B28 appearing
only when the retained diagnostics route adds a second reply-bearing path.

**The multi-reply interleaving exists and is observable.** `reclaim_dead_task`
(`slime-root/src/main.rs:4281`) loops over `DeathWakes` and calls `deliver_wake` — so
`send_reply` — once per wake, all inside one kernel entry. The QoS transcript records
`peer death task=3 channels=5 woken=2`: two tasks replied to in a single root
operation. Each `seL4_Send` on a reply cap runs `possibleSwitchTo` for its receiver,
and the second one's call sees `ksSchedulerAction` already holding the first target
rather than `ResumeCurrentThread` — the branch that then fires is
`rescheduleRequired()` plus `SCHED_ENQUEUE`, which enqueues the *first* target and
requests a reschedule.

**But the timing refutes it as task 10's cause.** The only `woken=2` line in the
transcript is at boot-log line 184, and task 10 does not park until line 283. A
pending switch that never existed cannot have been cleared, so this interleaving —
real as it is — is not what strands `fabric-publisher`. Every later wake in that boot
is `woken=0` or `woken=1`, i.e. one reply per kernel entry.

Seventeen readings excluded. That leaves the contradiction fully measured and
unexplained by any mechanism inspected so far: a `Running` child at priority 254,
absent from every ready queue, `ksSchedulerAction = ResumeCurrentThread`,
`ksCurThread` idle, reached after a single reply send over a live `cap_reply_cap`.

**A kernel breakpoint identifies the branch and the caller.** Booting with `-S`,
setting `breakpoint set -n possibleSwitchTo -c "(unsigned long)target ==
0x80604f6c00"`, and continuing stops with:

```
frame #0: possibleSwitchTo(target=0x80604f6c00) at thread.c:562
frame #1: restart(target=…) at thread.h:99 [inlined]
frame #2: invokeTCB_Resume(thread=…) at tcb.c:1698 [inlined]
```

and, at that moment, `ksSchedulerAction == 0`, `ksCurDomain == 0`,
`target->tcbDomain == 0`.

So the call reaching task 10 is its **activation** — `TCB_Resume`, which is
`tasks.activate(id)` from the root's launch loop — not a reply at all. And with the
domains equal and `ksSchedulerAction` at `ResumeCurrentThread`,
`possibleSwitchTo` takes its third branch: `NODE_STATE(ksSchedulerAction) = target`.
The thread is left `Running`, **deliberately unqueued**, with the switch pending in
`ksSchedulerAction` — exactly the state observed at the deadlock, and the reason
`ksReadyQueues[254]` is empty while a child is Running.

**Two corrections, both refuting the paragraphs above.** They are recorded rather
than deleted because each looked conclusive and each is a trap the next reader would
fall into.

*The TCB was not identified.* The IPC-buffer match is ambiguous: **three** tasks in
this boot bind window `0x238000` — task 6 (`init`), task 1 (the root-launched
`fabric-publisher`), and task 10 (the one init spawned). `child_vspace` lays every
component out identically, so `tcbIPCBuffer` cannot distinguish them, and the claim
that the Running TCB is task 10 does not follow. The `tcbPriority = 254` reading
still holds, so it is *a* child rather than the root or the idle thread — nothing
finer.

*No activation switch is dropped.* `rescheduleRequired`
(`deps/sel4/src/kernel/thread.c`) **enqueues** the pending target before overwriting
the action:
`if (action != ResumeCurrentThread && action != ChooseNewThread) SCHED_ENQUEUE(action)`.
So the second `possibleSwitchTo` in the root's activation loop takes exactly that
branch and the first target is enqueued, not lost. The transcript agrees:
`activated components=7`, and six of the seven root-launched instances go on to print
their own failure line, so they demonstrably ran. The breakpoint that fired was one
of those early activations, before task 10 existed at all.

**So the state is measured and the mechanism is still unknown.** Nineteen readings
excluded. What remains true: some child thread is `Running` at priority 254, absent
from every ready queue, with `ksCurThread` idle and `ksSchedulerAction` at
`ResumeCurrentThread`; and `fabric-publisher`'s spawned instance never resumes after
its role reply.

**The VSpace route was tried and does not close the gap either.** The stranded TCB's
`tcbVTable` entry reads a VSpace root of `0x80604b8000` — a kernel object address.
Logging `task.vspace.vspace.bits()` per task gives `0x2c7`, `0x305`, `0x345`, `0x386`,
`0x3cb`, `0x40c`, `0x44d`: distinct per task, and therefore a genuine discriminator,
but they are *root CSpace slot numbers*, not the kernel addresses the TCB stores. The
two namespaces cannot be compared without resolving each cap to its object, which is
what a kernel with thread-name support would have made unnecessary. The
instrumentation was reverted.

**So B28 stops here, root-caused only as far as the evidence allows.** Nineteen
readings excluded. Established beyond doubt:

* `fabric-publisher`'s spawned instance parks once for its role reply and never
  resumes; the plane cannot reach `[init] fabric stream complete`.
* Every root-side layer is correct and measured: park entry, a live `cap_reply_cap`
  (`debug_identify` = 8), `deliver_wake` firing against a task the root agrees is
  parked, the send performed and returning.
* At the deadlock the kernel holds *a* child thread (priority 254) in
  `ThreadState_Running`, absent from `ksReadyQueues` at every priority, with
  `ksCurThread` idle and `ksSchedulerAction` at `ResumeCurrentThread`.
* It is triggered by one fixture field — `retained` on the diagnostics participant —
  and that same field buys two of the five observed C8.5 arms.

**The kernel state was misread, and reading it correctly changes the diagnosis.**
`CONFIG_DEBUG_BUILD` *is* set in `build/sel4-qemu/gen_config/kernel/gen_config.h`, so
`tcbDebugNext`/`tcbName` do exist; lldb could not see them because they live in a
separate `debug_tcb` struct placed inside the TCB's CTE array
(`TCB_PTR_DEBUG_PTR(p) = TCB_PTR_CTE_PTR(p, tcbArchCNodeEntries)`), not in `tcb_t`.
The list is walkable at `(tcb & ~0x7ff) + 0xa0`, and walking it finds seven threads.

An earlier hand-rolled `state` read reported all of them `Running`, which is
impossible on one core and was the tell that the offset arithmetic was wrong. Reading
`tcbState.words[0] & 0xf` through the debug info instead — the same expression
`thread_state_get_tsType` uses — gives:

|TCB|`tsType`|Meaning|
|---|---|---|
|`0x80604f6c00`|4|`BlockedOnSend`|
|`0x80604b7c00`|4|`BlockedOnSend`|
|`0x8060473c00`|4|`BlockedOnSend`|
|`0x8060433c00`|4|`BlockedOnSend`|
|`0x80603f2c00`|5|`BlockedOnReply`|
|`0x8060030c00`|7|`IdleThreadState` (prio 0)|
|`0x807fd8a400`|0|**`Inactive`** — the root task|

The idle thread reading `IdleThreadState` rather than `Inactive` is the control that
confirms the typed read is right and the raw one was not. **So there is no Running
unqueued thread and no scheduler inconsistency.** Every prior paragraph resting on
that — the "kernel-level inconsistency", the dropped pending switch, the
`possibleSwitchTo` third-branch theory — is void. The ready queues are empty because
every thread is legitimately blocked.

**The root task is `Inactive`: it returned.** And the transcript shows why that is
fatal — the last lines are the root's own accounting, printed *after* the serve loop
fell out, including `replies owed count=1` / `reply owed task=6`. The serve loop
(`slime-root/src/main.rs:1522`) is `for _ in 0..MAX_GRAPH_ITERATIONS { if live == 0
{ break } … }`. With `sends=41 receives=37 parks=33`, roughly 111 operations ran
against a bound of 512, so the loop did **not** exhaust its iteration budget — it left
by `live == 0`.

**The `live == 0` reading was wrong too, and the loop's own marker says so.** A guard
was added making an owed reply at `live == 0` fatal; it did **not** fire. The
post-loop line then gave the answer directly: `served live=5`. The loop leaves with
**five tasks still live** and one reply owed, so it exits by exhausting
`MAX_GRAPH_ITERATIONS` — the root spins 512 times without any arrival advancing the
graph, then returns, which is what marks it `Inactive`.

**So the defect is a genuine wedge, and it was silent.** Falling out of the bound was
indistinguishable from settling: the root printed its ordinary accounting summary and
the boot looked healthy apart from a missing final marker. That is exactly how B28
stayed invisible through nineteen readings aimed at the reply path — `fabric-publisher`
never resuming is a *symptom* of the root going away, not an independent fault, and the
four `BlockedOnSend` children are blocked sending to a root that is gone.

**Fixed to that extent: the wedge is now reported.** `serve_component_graph` counts its
iterations and, on reaching the bound with tasks still live, fails with
`SLIME_GRAPH FAIL graph iterations exhausted live=5 parked=1` — observed on the QoS
plane. All nine passing planes were re-run and stay green, so the detector
distinguishes a wedged graph from a settled one rather than tripping on both. The
`live == 0` guard is kept beside it: unreachable today, but it is the other way a
graph can end owing a reply.

**It is a livelock in `fabric-service`, not a deadlock anywhere.** Logging the
operation label on the loop's final iterations shows task 9 — `fabric-service` — in a
fixed cycle: five `Recv` then one `Wait`, repeating to the bound. A `wait` that
returns immediately is a park on a source that is permanently ready, so the broker
burns the root's iteration budget instead of blocking.

**Two always-ready sources found and fixed** (`d69cd8e`). `park_on_streams` already
skips finished publishers and its own comment states the rule — a dead source is
always ready, so leaving one in the set turns the park into a spin — but never applied
it to:

* a subscriber whose peer is gone (`ended` is set both on a clean end event and on
  `ERR_PEER_DEAD`);
* the QoS clock (`TIME_SLOT` was pushed on the flag alone, though the worker already
  probes `time_peer_dead()` before asking it to advance).

With both exclusions the cycle widens from five `Recv` per park to about eleven, so
the always-ready wake is gone. All nine passing planes re-run green, so neither
exclusion changes a settled graph.

**The plane still wedges, and the remaining cause is now located to one condition.**
The stream worker returns only when *every* subscriber has `ended` **and** the clock
peer is dead (`components/bins/src/bin/fabric-service.rs`, the
`all(|subscriber| subscriber.ended)` block). The clock's client half is granted to
`fabric-publisher-b` (`init.rs:1748`), and that component reaches
`[fabric-publisher-b] done`; the root then reports
`peer death task=11 channels=6 woken=1` at transcript line 394, *before* the spin
window opens at 396. So the clock peer does die, the fabric is woken for it, and the
broker still does not take its exit — which places the defect in the broker's handling
of that wake rather than in the wake's delivery.

**Instrumented, and the causal chain is now complete.** Printing the exit block's
conjuncts on every pass gives `subs ended=3/0` then `3/1`, held forever: the broker
carries **three** subscribers and only **one** ever reaches `ended`. The clock probe is
never even attempted, because the first conjunct never becomes true — so the earlier
suspicion about `time_peer_dead()` racing an advance is void.

Instrumenting `announce_end`'s guard shows why. It refuses to end a subscriber that
still holds history or unacknowledged samples
(`if !subscriber.terminal && (!subscriber.history.is_empty() || subscriber.in_flight
!= 0)`), and the two stuck subscribers sit at `hist/inflight=1/1` permanently: one
sample delivered, never acknowledged.

**And the reason they never acknowledge is already on the transcript, upstream of
everything QoS.** `[fabric-subscriber] fail: role reply` and
`[fabric-subscriber-b] fail: role reply` — both subscribers die at role
provisioning, before they can ack anything, and the root duly reports
`peer death task=4` / `peer death task=5`. Their samples stay in flight forever, so
`announce_end` never fires for them, so the broker's exit condition is unreachable, so
it spins to the iteration bound.

**Attributing this to B25 was wrong, and the passing plane disproves it.** Booting the
**stream** plane — which reaches `[init] fabric stream complete` — shows the *same two*
`fail: role reply` lines from `fabric-subscriber` and `fabric-subscriber-b`. They are
expected negative-control assertions on both planes, not the defect, and B28 is not a
B25 symptom.

**The real differentiator is the retire path.** The stream plane logs
`[fabric] QoS peer dead` **twice** — the broker observes both dead subscribers on their
ack channels and calls `retire_subscriber`, which is what lets its exit condition
become true. The QoS plane logs it **zero** times, both before and after the park-set
fixes, so the exclusion in `d69cd8e` is not the cause. Counts: `QoS matched` 7 on the
stream plane vs 6 on QoS.

So the two subscribers stuck at `hist/inflight=1/1` are stuck because the broker never
takes `drain_acks`' `ERR_PEER_DEAD` arm for them, not because they died at role
provisioning — which they do on both planes.

**Traced further, and the ack channels are a red herring too.** `drain_acks` is called
unconditionally for every present subscriber — no flag gates it — so the flag-split
suspicion is void. Instrumenting the publisher sweep shows the broker pumps exactly one
publisher, index 2 / slot 20, which is `fabric-publisher`'s own route: publishers 0 and
1 are already `finished` when `broker` starts. Its `recv` returns `WOULDBLOCK` on every
pass because **`fabric-publisher` (task 10) never resumes to publish anything**. The
subscribers are stuck at `hist/inflight=1/1` waiting on samples that task 10 would have
sent. So the whole QoS-side chain reduces back to the original symptom.

**The root's side of that wake is now fully instrumented and is correct.** Four
measurements, each reverted after being taken:

* `deliver_wake`'s silent `parked.reason(task).is_none()` early return — added a marker;
  **zero** lines. No wake is ever dropped for being unparked.
* `send_atomic`'s wake for the transfer that carries the role — `xfer wake
  present=true target=10`. The wake *is* generated.
* `deliver_wake` reaching its answer — `wake answering task=10` fires. The task is
  answered.
* `ParkedReplies::wake` ordering — `send_reply(held.slot, …)` completes before
  `release_slot` deletes the slot, so the B29 fix does not invalidate the reply it just
  sent.

**And the plane comparison rules out composition.** Byte-for-byte against the *passing*
stream plane, task 10 parks at the same point, receives exactly the same two
`capability transferred … to=10` records on the same channel (key 5 = its
`CONTROL_SLOT = 0`), and the same `sent … queued=1` / `QoS matched` pairs follow. The
two planes are indistinguishable through the entire role handoff; the stream plane then
shows `received task=10` twice and QoS shows it **zero** times.

**So the defect is isolated to one transition with everything around it verified:** the
root sends a reply over a live `cap_reply_cap` to a task the kernel has in
`BlockedOnReply`, on a plane whose every preceding step matches a plane where the same
send works, and the task does not run. Twenty-one readings excluded.

**The baseline was taken, and it retires the kernel-state line of inquiry.** Walking
`ksDebugTCBs` on the *passing* stream plane at `[init] fabric stream complete` gives
`idle=0x8060030c00 cur=0x8060030c00 action=0x0` and only **two** threads on the list:
the idle thread and the root. Every one of the six children is gone, properly reclaimed.

So the healthy terminal state has *no* children, and the QoS plane's five survivors are
the anomaly — which is exactly what a livelocked broker produces and needs no kernel
explanation at all. `ksCurThread == ksIdleThread` with `ksSchedulerAction ==
ResumeCurrentThread` and empty ready queues is *also* the healthy end state, so none of
those three readings ever indicated a fault. This is the control that should have been
taken before any of the seL4 work.

**The component-side marker was taken and it settles what task 10 is doing.**
Bracketing `receive_role`'s wait arm in `fabric-publisher` prints
`[dbg] role: parking` and **never** `[dbg] role: wait returned`. So task 10 is not
looping between `recv` and `wait`, and it is not mis-decoding an answer: it is blocked
inside one `slime_rt::wait` that never returns. The staging-failure arm above it
(`[rt] wait source set could not be staged`) does not fire either, so the wait set did
cross intact.

**And the root's reply is provably aimed at that exact wait.** Instrumenting
`ParkedReplies::commit`/`wake` to print the CSlot index gives, in order:

```
[dbg] role: parking                        <- component enters wait
SLIME_DBG park task=10 slot=1667           <- root saves the reply cap
SLIME_GRAPH parked task=10 reason=wait     <- committed as a Wait, not a Recv
SLIME_DBG wake task=10 slot=1667           <- answered on the same slot
```

`slot=1667` appears exactly twice in the whole boot, so the index is neither reused nor
recycled between the park and the send, and the park is committed with
`ParkReason::Wait` — the operation the component is actually blocked in. Combined with
the earlier readings, every link is now individually verified: the wake is generated
(`xfer wake present=true target=10`), it is not dropped as unparked, `deliver_wake`
reaches `wake answering task=10`, the slot identity matches, `send_reply` runs before
`release_slot`, and the kernel calls the cap a live `cap_reply_cap`.

**So B28 is isolated to a single unexplained transition:** the root invokes
`slot.cap().send(info)` on a live reply capability naming a task the kernel has parked
in `SYS_WAIT`, on the correct CSlot, and that task never resumes — while the *same*
code path answers tasks 4, 5, 7, 8, 9, 11 and 12 correctly in the same boot, and
answers task 10 itself correctly on the stream plane. Twenty-two readings excluded.

**The wait registration was checked too, and it is correct.** Instrumenting
`serve_wait`'s registration loop for task 10 prints `w10 registering
target=Receive(5)` — the same channel key the fabric's two
`capability transferred task=9 channel=5 to=10` records land on, and the same key the
root hands the component at `channel handed parent=6 child=10 key=5 slot=0`. The
registration also passes through `ChannelTable::recv_queue_mut(key, task)`, which
resolves `forward` for the consumer and `reverse` for the producer, so a
holder/direction mismatch would have registered on the wrong queue — it does not.

`serve_wait` additionally re-probes readiness *after* registering, specifically to close
the lost-wakeup window between the first probe and the registration, so a send landing
in that gap cannot be missed.

**Every layer of the root is now individually verified against this one hang**, and the
list is worth keeping because it is what makes the residue small: wait set stages, wait
target resolves to the right key, registration lands on the right queue, readiness is
re-probed after registering, the wake is generated by `send_atomic`, it is not dropped
as unparked, `deliver_wake` reaches its answer, the park was committed as
`ParkReason::Wait`, the reply CSlot index matches the park exactly and is used twice in
the whole boot, `send_reply` runs before `release_slot`, and the kernel identifies the
capability as a live `cap_reply_cap`.

**A note on one earlier kernel reading, so it is not trusted later.** Re-walking
`ksDebugTCBs` on the wedged QoS plane with *typed* reads now gives two children
`BlockedOnSend`, three `BlockedOnReply`, idle at `IdleThreadState`, and the root
`Inactive`. The root being `Inactive` is an artifact of this entry's own wedge
detector — `fatal!` fires and the root exits — so that snapshot describes the
post-mortem, not the hang. Any future kernel reading of this defect must be taken with
the detector disabled, or it measures the detector.

**Two more candidates checked and both refuted, by reading rather than by running.**

`root_service()` cannot differ per task: it is
`cap::Endpoint::from_bits(ROOT_SERVICE_SLOT)` with `ROOT_SERVICE_SLOT = 1`, a
compile-time constant every component shares. Task 10 calls the same endpoint as the
tasks that are answered correctly in the same boot.

The endpoint's *rights* looked more promising, because
`task.rs:348` gates the `grant` right on the generation declaring the grant
transferable — and `init-fabric-publisher` in `sel4-qos.zti` is
`rights = ["exec"; "spawn"]` with `transferable = false`, so task 10's service endpoint
carries `grant_reply` but not `grant`. That would plausibly stop a reply that conveys a
capability. But the *stream* fixture declares that grant identically —
same rights, same `transferable = false` — and delivers the same two transfers to the
same task successfully. So it is not the difference either.

**The fixture diff was done and it is remarkably small.** `sel4-stream.zti` and
`sel4-qos.zti` differ in exactly **three** fields: `generation` (1 vs 19), and on
`fabric-publisher-b`'s *diagnostics* participant `durability`
(`volatile` → `retained`) and `retainedDepth` (`0` → `2`). Nothing else — same
components, same grants, same telemetry route, same capacities
(`FABRIC_FRAME_CAPACITY = 32` on both). The wedged task is `fabric-publisher` on the
*telemetry* route, a different component and a different route from the one field that
changed.

**And the observable consequence is in the loan table, not the frame table.** Both
planes create five loans. The stream plane maps and returns **three**; the QoS plane maps
and returns **one**. Per-loan:

|Plane|`id=1`|`id=2`|`id=3`|
|---|---|---|---|
|stream|mapped by task 9|mapped by task 7|mapped by task 8|
|QoS|mapped by task 9|**never mapped**|**never mapped**|

Loans 2 and 3 are created by the broker and never taken by the subscribers — which is
exactly the `hist/inflight=1/1` state the subscribers are stuck in, seen from the other
side.

**Two tempting explanations for that are already excluded.** It is not frame
exhaustion: instrumenting `pump_publisher`'s `!frames.iter().any(|f| f.refs == 0)` guard
produced **zero** lines. And it is not queue backpressure: the *stream* plane runs
deeper queues (`queued=` up to 11) than QoS (up to 6), so a full channel cannot be what
stops the QoS delivery.

What the transcripts do show at the divergence is a scheduling difference in the broker
itself. After the same `capability transfer task=9 … to=8` and `[fabric] downstream loan
created`, the stream plane keeps serving (`received task=9 channel=21`) while the QoS
plane immediately emits `[fabric] idle: parked on stream sources` and
`parked task=9 reason=wait`. The broker parks with two loans outstanding and undelivered.

**`deliver`'s decline was instrumented, and it is correct behaviour, not the bug.**
Every refusal comes from one arm: `history.entry_at(subscriber.in_flight)` returning
`None`. That is by design — `entry_at` documents `offset >= len => None`, and the
stuck subscribers sit at `in_flight = 1` with `len = 1`, meaning everything the ring
holds has already been sent and is awaiting an ack. `deliver` is right to stop, and the
`in_flight >= history.depth()` gate above it is not involved either (the subscribers
declare `historyDepth = 8`).

**The eviction bookkeeping is also correct**: `history.push` returning an evicted entry
decrements `in_flight` and releases the frame, so a stalled subscriber cannot ratchet
`in_flight` past its depth.

**The subscribers are alive, not dead.** This corrects an assumption three earlier
paragraphs shared. `peer death task=4` / `task=5` name the *root-launched* subscribers,
which hold one channel each and are never provisioned into the graph. The broker's real
subscribers are tasks **7 and 8**, and on the QoS plane they never die at all — they are
`parked`, waiting for samples. On the stream plane they do eventually die
(`channels=3`, `channels=5`). So no `ERR_PEER_DEAD` is owed on their ack channels, and
`drain_acks`' peer-death arm is right not to fire.

**Which puts the whole chain back on one fact:** `fabric-publisher` (task 10) sends
**2** messages on the QoS plane and then blocks in the `SYS_WAIT` that never returns.
Task 11 sends 10, so the broker keeps working; the subscribers ack twice
(against 18 on the stream plane) and then park because no further sample arrives. Every
downstream symptom — the two unmapped loans, `hist/inflight=1/1`, `announce_end`
refusing, the broker spinning — follows from that single hang, and none of them is an
independent defect.

**One candidate fix was written, verified not to fire, and reverted.** `deliver`
collapsed `ERR_WOULDBLOCK | ERR_PEER_DEAD => false` on both send paths, so a dead peer
was retried like a busy one; splitting the arms to retire the subscriber built clean and
kept all nine planes green, but the new arm was never reached on *any* plane — the QoS
plane returns earlier at `entry_at`, and the stream plane's two `[fabric] QoS peer dead`
lines both come from the pre-existing `drain_acks` path. Unobserved code is not a fix,
so it was reverted rather than committed.

**The root now names its wedged waiters, and the answer is not task 10.** The wedge
`fatal!` fired *before* the owed-reply accounting further down the function — and
`fatal!` does not return — so the one path that most needed the diagnosis printed
only counts. Fixed: the exhaustion arm iterates `parked.tasks()` first and emits
`SLIME_GRAPH wedged waiter task=N` per entry.

On the QoS plane that gives, in order: **7, 8, 9, 6** — `fabric-subscriber`,
`fabric-subscriber-b`, `fabric-service`, and `init`. **Task 10 is absent.**

That overturns the reading this entry was built on. `fabric-publisher` (task 10)
sends twice, parks once, is never reclaimed, and is *not* among the tasks the root
is holding a reply for. So its `SYS_WAIT` was **answered** — the root is not owing
it anything — and it still did not resume. The four tasks actually stuck are the
broker and its two subscribers, which is a different shape entirely: the broker is
waiting on sources the subscribers would make ready, and the subscribers are waiting
on samples the broker would deliver.

**The root now prints the whole deadlock.** `ChannelTable::registered_waits` and
`Channel::waits_for` were added — diagnostic-only scans — so the exhaustion arm
emits each waiter's park reason and every channel it is registered on:

```
wedged waiter task=7 reason=Some(Wait)   channel=16 receive=true
wedged waiter task=8 reason=Some(Wait)   channel=12 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=13 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=17 receive=true
wedged waiter task=9 reason=Some(Wait)   channel=22 receive=true
wedged waiter task=6 reason=Some(Wait)   (no channel — a supervision wait)
```

All five channels are broker-minted role endpoints. Channel 22 is the interesting
one: it is minted at transcript line 285 and *is* transferred to task 10 at line
286 (`capability transferred task=9 channel=5 to=10`), so the broker holds one half
and `fabric-publisher` the other. The broker is waiting to receive a sample on it;
task 10 parked without ever sending one.

So the cycle is: the broker waits on the publisher's route (22) and both
subscribers' acks (13, 17); the subscribers wait on their data channels (16, 12)
which only the broker fills; `init` waits on a supervision handle. Nothing can
move because the one task that would break the cycle — task 10 — is parked and
**is not owed a reply by the root**, having been answered already.

**Narrowed once more, to a two-line difference between the two transcripts.**
`fabric-publisher`'s `receive_role` loop runs **twice** — a publisher role is two
capabilities, a send-only data endpoint and a receive-only credit endpoint
(`fabric-publisher.rs:126-143`). Both planes deliver exactly two transfers to task
10. The difference is what happens next:

| | stream (passes) | QoS (wedges) |
|---|---|---|
| `parked task=10 reason=wait` | line 280 | line 283 |
| `capability transferred … to=10` ×2 | 283, 285 | 286, 288 |
| **`received task=10 channel=5`** | **299, 300** | **never** |
| `[fabric-publisher] publish role received` | printed | never |

So the transfers land identically and the *receives* do not happen on the QoS
plane. Task 10 is parked, is owed nothing by the root (it is absent from the
`wedged waiter` list), and never performs the `recv` that would collect the
capabilities already delivered to it.

**The mechanism is now confirmed, and it is starvation rather than a lost wake.**
Three measurements settle it:

1. **Task 10 is not parked.** `live=5 parked=4`, and task 10 is the one live,
   unparked, unreclaimed task. Its `SYS_WAIT` *was* answered.
2. **The kernel says it is blocked sending.** Walking `ksDebugTCBs` with typed reads
   gives one child in `BlockedOnSend` and four in `BlockedOnReply`. The four are
   normal parked-awaiting-root; the one is a task whose call into the root never
   got received.
3. **The root never reaches it.** Logging the operation on the loop's final twelve
   passes gives `task=9 op=Recv` for eleven of them. The broker consumes the entire
   512-iteration budget, so task 10's send is still queued when the budget ends.

The broker's own loop is *not* spinning — instrumenting its `progressed` branch past
400 passes produced **zero** lines, and it emits
`[fabric] idle: parked on stream sources` **ten** times. So it parks, is woken,
serves a handful of `Recv` calls, and parks again — repeatedly. Each cycle costs
root iterations, and ten cycles at ~50 operations each is the budget.

So B28 is: **an always-ready wake source makes the broker cycle park→wake→park, and
those cycles starve the root's iteration budget before `fabric-publisher`'s queued
send is served.** That is the same class as the two park-set spins fixed in
`d69cd8e` — a third source is still permanently ready — and it explains why the
plane depends on one fixture field: `retained` diagnostics add the route whose
source never quiesces.

**The park set was printed, and it narrows the candidate to one slot without yet
convicting it.** Instrumenting `park_on_streams` to dump its contents gives, across
the ten cycles:

```
20 11 13 15 09  <- park set   (×5)
20 11 15 09     <- park set   (×3)
20 11 15        <- park set   (×1)
```

Slots 09 and 13 correctly drop out as their peers retire. **Slot 20 is present in
every set including the last.** It is the broker's half of channel key 22
(`endpoint minted task=9 key=22 slots=20,21`) — `fabric-publisher`'s route, the same
channel the wedge diagnostic reports the broker waiting to receive on.

**But that does not by itself explain the wake**, and the distinction matters:
`ChannelTable::is_ready` for a `Receive` target is `len != 0 || !peer_alive`, and
task 10 is alive and has sent nothing, so key 22 should be *not* ready. Being
present in the park set is not the same as being ready.

So the remaining question is one predicate on one channel: what makes the root
answer `wait` immediately when key 22 is in the set. Either `receive_ready` sees a
queued message that the broker's `recv` then fails to take, or the set contains a
second target on key 22 whose readiness differs — the broker pushes a publisher's
data slot and a subscriber's *ack* slot, and slot 21 is key 22's other half.

Thirty-one readings excluded. Every layer above this predicate is now measured:
task 10 answered and unparked, the kernel's per-thread states, the root's iteration
accounting, the broker's own loop not spinning, and the park set's exact contents. B28 stays open; the two park-set spins fixed under it
(`d69cd8e`) were real and are kept. B28 stays open; the two park-set spins fixed under it (`d69cd8e`) were real
and are kept.
Re-check this entry after B25 lands rather than investigating it further on its own.

**One wider finding stands regardless of B28**, and it is worth its own slice:
`sel4_transport::wait` returns `()`, so its staging-failure branch can only
`yield_now()` and return silently. It is unreachable on every current plane — hence
the zero lines above — but if it were ever reached it would convert a bounded error
into an invisible hang, exactly the signature that made this defect take seven
attempts to characterize. It should either report or be made impossible by
construction.

**Severity:** Blocks P5.4.5's exit condition and nothing else. Latent for every
other plane: no other seL4 graph declares two retained routes on one publisher.
The tradeoff is quantified — `retained` yields five observed C8.5 arms with
`fabric-publisher` parked, `volatile` yields three with it running, and neither
reaches the final marker — so the committed fixture keeps `retained` as strictly
more coverage.

**Exit condition:** With the diagnostics route `retained`, `fabric-publisher`
takes its role reply and the plane reaches `[init] fabric stream complete`,
asserted by a gate, with a fault injection showing the parked case caught.


### B12 — the component build's `--remap-path-prefix` names a path that does not exist

**Resolved 2026-08-07.** Devlog:
[`devlog/2026-08-07-b12-component-remap/`](../devlog/2026-08-07-b12-component-remap/index.md).
The hardcoded literal is gone from `components/.cargo/config.toml`;
`build-rust-components` now appends `--remap-path-prefix={ROOT}=.` for triple
targets through `--config`, mirroring what the JSON-target branch already did
through `RUSTFLAGS`.

**`--config` and not `RUSTFLAGS`, which is the whole difficulty.** Setting
`RUSTFLAGS` *replaces* the config's rustflags rather than adding to them, so it
would have silently dropped `relocation-model`, `code-model`, and three link args
the x86 link depends on. The JSON branch can set `RUSTFLAGS` freely only because a
JSON target inherits none of those to begin with.

**Two corrections to this entry, both material.** First, the checkout is now
`/Users/iceice666/code/slime_os`, so the stale literal is not even a *prefix* of
the real path — the mangling this entry describes stopped happening at some point
and the flag became an outright no-op. Second, and more importantly, **the
severity was overstated**: these are release builds, and the x86 component ELFs
embed *zero* absolute source paths (`strings … | grep -c '/Users/iceice666'` is 0
for every component). So the flag had nothing to remap either way.

That is why the deferral's central fear — that fixing this would alter every
component ELF and therefore every generation identity the oracle's gates assert
against — turned out to be empty. Measured directly: the generation identities
before and after the fix are **byte-identical**
(`df40ce7a…13e5`, `ebdf06d0…b092`), `just generation_check` passes, and the seL4
channel, stream, and component-graph plane gates are unaffected.

**Exit condition partially met, and the remainder is now argued rather than
observed.** Two builds from two different checkout directories were *not* run —
that needs a second clone, which this environment cannot usefully provide. What
was established instead is stronger than the original worry and weaker than the
original exit condition: the flag is no longer wrong, it is computed from the
actual root, and the artifacts it guards contain no paths for it to affect. If a
future build turns on debug info for components, the flag becomes load-bearing and
the two-checkout comparison becomes worth running for real.

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

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate**, on the same
reasoning: that slice replaces the seventh seL4 generation through the same
build path, whose rustflags are keyed by triple and match none of the stale
literal's. See `devlog/2026-08-05-p5-5-2-stream-plane/`.

**Deferral re-reviewed 2026-08-05, before opening P5.5.1's gate**, on the same
reasoning: that slice adds a seventh seL4 generation through the same build
path. See `devlog/2026-08-05-p5-5-1-typed-fabric/`.

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

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's gate.** Still
deferred, on the same reasoning. B16's fix adds an eighth seL4 generation and a
new component binary built through the same JSON-target path, which the
rustflags this defect concerns do not match, so the reach is unchanged once
again. `just generation_check` and `just contracts_check` were run to confirm
the new binary perturbed neither contract validation nor generation identity.
See `devlog/2026-08-07-b16-supervision-records/`.

**Deferral re-reviewed 2026-08-07, before opening P5.4.1's own gate.** Still
deferred, on the same reasoning once more. B22's fix adds a ninth seL4
generation and a new component binary through the same JSON-target path, whose
rustflags this defect does not match, so the reach is unchanged.
`just generation_check` and `just contracts_check` were run to confirm the new
binary perturbed neither contract validation nor generation identity. See
`devlog/2026-08-07-p5-4-1-oracle-inventory/`.

### B30 — `release_trust_check` was red, unregistered, and its rotation refusals never reached Rust

**Resolved 2026-08-07.** Devlog:
[`devlog/2026-08-07-b30-release-trust-gate/`](../devlog/2026-08-07-b30-release-trust-gate/index.md).
Observed exit condition: `just release_trust_check` passes, is listed in
`AGENTS.md`'s gate index, and each rotation continuity branch is guarded by its
own fixture — removing the replacement check fails with
`apply_rotation accepted version-skip`, removing the previous check fails with
`apply_rotation accepted stale-previous`.

**Problem:** three separate defects in one gate, found by running it.

1. **It could not run at all.** `scripts/lib/release_trust.py` re-exports generated
   constants from `boot_contracts`, but never imported `ROTATION_BYTES`,
   `ROTATION_HEADER_BYTES`, `ROTATION_MAGIC`, `ROTATION_VERSION`, or
   `MAX_TRUST_KEYS`. `just release_trust_check` died with
   `AttributeError: module 'release_trust' has no attribute 'ROTATION_BYTES'`
   before asserting anything, so all thirteen of its `expect_error` cases were
   dead code.
2. **It was not in the gate index.** `AGENTS.md:61-77` is canonical, and this
   target was absent — which is why a red gate went unnoticed.
3. **Its rotation refusals tested Python, not the kernel's decoder.**
   `verify_rotation` (`check-release-trust.py:181`) is a pure-Python
   reimplementation of the same rules. Only the *valid* rotation was ever handed
   to `apply_rotation` through the `verify_release` example, so all three
   continuity assertions proved the fixture was malformed, never that
   `release.rs` refuses it.

**Evidence:** with the import fixed, deleting the
`replacement_version != current.version + 1` branch from `apply_rotation` left
the entire gate **green**. So did deleting `previous_version != current.version`.

**Fixed.** The four rotation constants and `MAX_TRUST_KEYS` are now loaded from
`boot_contracts` directly in the check (`CONTRACTS`), rather than widening
`release_trust`'s imports to names its own body does not use — which ruff
correctly flags as F401. Every rotation refusal now goes through
`apply_rotation` as well as the Python mirror, via
`expect_rust_rotation_refused`. A fixture for stale `previous_version` was added,
because the two existing continuity cases vary the *signature counts* and never
reach that branch.

The new fixture is `(previous=2, replacement=2)` and not `(2, 3)`: the
replacement version must stay at `current.version + 1` or the replacement branch
fires first and masks the branch under test. Getting that wrong is why the first
attempt at this fixture still passed under injection.

**Exit condition met.** `just release_trust_check` passes, is registered in
`AGENTS.md`, and each continuity branch is now guarded by its own fixture:
removing the replacement check fails with `apply_rotation accepted version-skip`,
removing the previous check fails with `apply_rotation accepted stale-previous`.
Both observed, then reverted.

**One guard attempted and deliberately not shipped.** `apply_rotation`'s
`replacement.validate()?` still has no fixture that isolates it: deleting the call
leaves the gate green. Two candidate fixtures were built and both failed to
discriminate, because the signature loop rejects them first —
`verify_signature_entries` resolves each key-id by `sha256(key)` against
`root.keys[..key_count]`, so any replacement root malformed enough to fail
`validate` also fails to match a signature. Shipping a fixture that passes with
and without the call would have looked like coverage while proving nothing, so it
was reverted rather than committed.

**A third attempt established *why*, and the answer is that the call is
redundant on this path rather than untested.** `build_rotation` was
parameterised to take the replacement threshold, and a fixture built with a
correct two-key set and `threshold = 3` — signature-valid, `validate`-invalid.
It is still refused with the call deleted, because
`verify_signature_entries` independently returns `MissingSignatures` when
`count < root.threshold`, and the replacement root is passed to it immediately
after. Every malformation `TrustRoot::validate` catches is therefore also caught
downstream on this path:

* threshold above key count → `count < threshold` in `verify_signature_entries`
* zero, duplicate, or trailing keys → no `sha256(key)` matches the entry's key-id

So `replacement.validate()?` is defence in depth, not a live guard, and no
black-box fixture can distinguish its presence. Three candidate fixtures were
built and all three were reverted rather than committed, because a test that
passes with and without the code it names looks like coverage while proving
nothing.

**Recorded as accepted, not open.** The honest statement is that `validate` is
directly covered by the fifteen `TrustRoot::validate` unit tests in
`boot-contracts/src/release.rs`, and its use inside `apply_rotation` is
unreachable-by-construction given the checks that follow it. If that ordering ever
changes — if a future `apply_rotation` uses the replacement root before signing —
the call becomes load-bearing and will need the fixture this note describes.

### B29 — `ParkedReplies::wake` never deleted the reply CSlot it counted as recycled — **resolved 2026-08-07**

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

**Problem:** `build_sel4_generation`'s manifest→flag loop
(`scripts/build/build-generation.py`) set the selected manifest's flag and
popped every other manifest's in the same iteration. With one flag per manifest
that is correct. The moment two manifests declare the same flag it is not: a row
later in the table pops what an earlier row set, and which one wins depends on
table order rather than on the selection.

**Found by** P5.4.5's QoS plane, which is the stream driver plus a clock and so
declares `SLIME_SEL4_STREAM_CHECK` alongside the oracle's
`SLIME_FABRIC_QOS_CHECK`. Adding the `sel4-qos` row *after* `sel4-stream`
cleared the stream plane's own flag, and `just sel4_stream_check` failed with
`boot exceeded 180s without reaching the final marker` — init fell through to
`[init] launching component graph` and spawned nothing. Observed directly, and
worth recording because the failure is a timeout rather than an error: nothing
said "flag missing", and the plane simply ran a different composition.

**Resolved by** collecting the selected manifest's flags into one set and every
flag the table declares into another, then setting the first and removing the
rest. A flag two manifests share now survives for whichever asked for it,
independent of row order.

**Exit condition observed.** `just sel4_stream_check` passes with the
`sel4-qos` row present, and the QoS plane's own boot shows both flags in effect
— it runs `drive_stream_plane` and its components take the QoS path. All nine
seL4 plane gates pass with every image rebuilt. See
`devlog/2026-08-07-p5-4-5-qos-clock/`.

### B26 — the `[layout]` dump reported the grant's rights, so a too-permissive layout row was unobservable — **resolved 2026-08-07**

**Problem:** `slime-root/src/main.rs` printed each layout row's rights from the
*installed capability*, which `launch_component_graph` fills from the
**generation grant**, rather than from the boot-layout entry the row exists to
freeze. `bootstrap_executable_slot` and `bootstrap_slot` test *containment*
(`rights & !entry.rights != 0`) rather than equality, deliberately and
correctly — a layout marks a channel half `RIGHT_TRANSFER` because init hands
it on, while the grant is not about delegation at all, and requiring equality
rejected a well-formed graph once already. So the two legitimately differ, and
a dump carrying only one of them could not show a layout declaring strictly
more authority than anything uses. B10 exists to keep the table that declares a
slot and the table that fills it in agreement; this was the one direction of
disagreement the gate was blind to.

**Found by** fault-injecting P5.4.6's call plane: changing
`SEL4_CALL_LAYOUT`'s `fabric-call-server` row from `0x10008` to `0x1000c`
rebuilt the generation to different bytes (verified by md5) and the gate still
passed, while swapping two slot *numbers* in the same table was caught
immediately. That contrast is what localized the gap to rights.

**Resolved by** `declared_layout_rights`, which resolves the layout entry
behind a bootstrap row — by identity for an executable, by role for the two
singular factories — and appends `declared=0x…` when it differs from the
installed value. Appended and only on disagreement, so every row that agrees
keeps the retired kernel's exact four fields and stays comparable to
`dump_boot_layout`'s output slot for slot. `check-sel4-boot-layout.py`'s
`ENTRY` pattern admits the optional tail.

A channel end is deliberately not covered: it is named by its *grant*, and one
capability can be reached by more than one grant name, so reporting a declared
value would mean picking one. Executables and the two factories are where a
layout row's rights are unambiguous, and they are the rows a layout edit
touches.

**Exit condition observed.** The previously-invisible `0x10008`→`0x1000c`
injection now fails the gate, reporting
`now: [layout] 5 executable fabric-call-server 0x10008 declared=0x1000c`
against the frozen row. Restored and re-verified green.

The fix immediately earned itself: re-blessing surfaced three *pre-existing*
disagreements nothing had ever reported — `sel4-loan`, `sel4-sample`, and
`sel4-stream` each declare `0x1000004` on their shared-buffer-factory row while
the root installs `0x1000000`. Those are legitimate containment differences,
now recorded rather than invisible. See
`devlog/2026-08-07-b26-layout-declared-rights/`.

### B24 — `SharedBufferTable::quotas` never reclaimed, so `MAX_CHARGE_HOLDERS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's and B22's defect shape in a third table, and the one B16's
sweep implicitly cleared. `slime-root/src/shared_buffer.rs:502` declares
`quotas` one line below `charges`, which B16 named among the correct tables.
`charges` **is** correct — `uncharge` frees it at `:1782-1784`. `quotas` had no
free path anywhere: `declare_quota` reuses a slot only for the same `HolderId`
and otherwise takes a fresh one, while `commit_teardown`, `reclaim_holder`, and
`advance_epoch` never mentioned it. Because `construct_child` keys it by task id
and `TaskTable::next_id` never rewinds, a spawn/reap graph presented a fresh
holder every time and the 96 slots bounded the holders a boot could **ever**
construct.

Found by P5.4.1's lifetime-vs-live class audit rather than one at a time, which
is the reason that audit was scoped as a class: `quotas` is *keyed* per-task but
*declared* per-component at boot, so it does not read as a per-task table at a
glance and B16's per-task sweep passed over it.

**Resolved by** `release_quota`, called from `reclaim_dead_task` after charge
settlement — the ceiling outlives every charge made against it and is dropped
only once nothing can be charged again. A **direct release rather than a derived
sweep**, unlike B16 and B22: a quota has exactly one holder and that holder is a
task, so "the task is gone" is complete information. Those two needed predicates
because a supervision handle or a channel end can be named by a capability that
outlives the task; a quota cannot.

**Exit condition amended, and why.** The condition recorded when this item was
opened asked for a graph constructing more than `MAX_CHARGE_HOLDERS` holders.
That is unreachable: root CSlots are deliberately never returned
(`task.rs:165-167`), and the supervision plane's 35 spawns consume 2321 of 3457,
so a boot exhausts CSlots near 52 tasks and cannot reach 97. Stretching the
evidence to fit the original wording would have been the wrong move; the
condition is restated to what the platform can carry.

**Exit condition (observed 2026-08-07):** every constructed holder releases its
declared ceiling when its task dies, observed under `just sel4_supervision_check`
— 38 holders constructed over one boot, 38 `SLIME_GRAPH quota released` lines,
and `quotas=0` on the terminal accounting — and fault-injected to show that
disabling the release leaves `quotas=38`. Asserted on that existing plane rather
than a tenth image, since it is already the deepest spawn/reap loop in the
corpus. See
[`devlog/2026-08-07-b24-shared-buffer-quotas/`](../devlog/2026-08-07-b24-shared-buffer-quotas/index.md).

**Follow-up recorded, not opened:** root CSlot non-reuse is now the binding
lifetime constraint on graph longevity, ahead of every table this class audit
examined. Deliberate and documented rather than a defect, but P5.4.1 classified
it as acceptable-monotonic without quantifying it.

### B23 — `slime-root`'s unit tests were run by no gate — **resolved 2026-08-07**

**Problem:** 102 `#[test]` functions across 13 modules were compiled by nothing
and run by nothing, while `slime-root/src/main.rs` described those modules as
"bounded, pure, and unit-tested in place". Two independent blockers: no Justfile
target named the crate, and it could not have run anyway — `main.rs` is
unconditionally `#![no_std]`/`#![no_main]`, the package declared no lib target,
and the crate built only for a seL4 JSON target with no `libtest`.

**Resolved by** splitting the mechanism modules into a `slime_root` library the
binary links, rather than a `cfg(test)` escape (which neither blocker admits) or
a separate test crate (whose passing tests would be evidence about a copy). The
`sel4` crate builds for a host target given `SEL4_PREFIX`, so nothing had to be
excluded: all 13 covered modules run, including the seL4-touching ones.
`sel4-root-task` is scoped to `cfg(target_os = "none")` because it pulls
`sel4-alloca`, whose inline ELF section directive will not assemble on Mach-O;
only the binary needs it and the seL4 build is unchanged.

**What the first run found, which is the point:** three latent defects, every
one a test silently wrong since something changed under it. Nine `push` call
sites had been stale since P5.3.2 added a `transferable` parameter. An
`elf_header` fixture was 20 bytes against `LEGACY_HEADER_LEN`'s 32, so it had
been asserting `Unrecognized` rather than the bare-ELF arm ever since
`component_image::target` gained its length guard. A `qualified` fixture sized
its tail with a literal that no longer matched. All three are test bugs rather
than production bugs — the good case, but not evidence that nothing was hiding.

**Exit condition (observed 2026-08-07):** `just test_sel4_root` runs 102 tests
across 13 modules and asserts the count, so a module that stops being covered is
visible. It is a gate of its own rather than a `test_host` arm, because it needs
the installed seL4 prefix that `test_host`'s CI runner does not build — the same
reason `lint_sel4_root` stands apart. Fault-injected by removing one `transit`
test: the gate fails with `ran 101 tests, expected 102`. The nine seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged, so the lib
split did not disturb the image. See
[`devlog/2026-08-07-b23-slime-root-host-tests/`](../devlog/2026-08-07-b23-slime-root-host-tests/index.md).

**Noted, not fixed:** `just test_host`'s `slime-proto` arm pins
`x86_64-unknown-linux-gnu` and therefore fails on an `aarch64-apple-darwin`
host, which was true before this change and is confirmed by stashing it.
`test_host` is left untouched — this fix adds no arm to it, and
`test_sel4_root` uses the host triple.

### B22 — `ChannelTable` never reclaimed, so `MAX_CHANNELS` was a lifetime bound — **resolved 2026-08-07**

**Problem:** B16's exact defect shape in a second table.
`slime-root/src/channel.rs` never freed an entry: `push` derived its key as
`self.len` (`:446`), `mark_dead` (`:339-354`) marked both queues of a dying
task's channels dead but freed nothing, and `reassign` only rewrote the holder
fields. So `MAX_CHANNELS` (32) bounded the channels a boot could **ever** mint,
not those live at once, and every channel a long-running graph minted was spent
permanently.

**How it differed from B16, and why that changed the fix's evidence:** B16
dropped a record *silently* and hung the parent, so converting the failure into
a reported one was part of its fix. B22's was already a bounded refusal —
`ChannelError::TableFull` becomes `IpcError::DestinationSlotsExhausted` — so
"the failure became reportable" proves nothing here. The gate could only be
satisfied by the graph *succeeding* past 32. The downstream symptom was the real
cost: a refused `mint` surfaces in the component, and at `MAX_CHANNELS = 16` the
stream plane's exhaustion "read as four broken components rather than one
exhausted table" (`channel.rs:107-111`). The bound had already been crossed once
and raised rather than fixed.

**Resolved by** `channel::sweep(&mut ChannelTable, &GraphTables, &Transit)`,
which frees every entry no live holder can name — derived from state that
already exists, exactly as `supervision::sweep` is. Two predicates, not one:
`GraphTables::holds_endpoint` for the live half and `Transit::holds_endpoint`
for the in-flight half, because `serve_cap_transfer` drops the capability from
the sender's table *before* parking it, so a sweep reading only the graph would
free the channel a transfer is mid-way through moving.

A precondition came with it: `key = self.len` had to become a monotonic
`next_key`. That derivation is unique only while `len` never decreases — once
the sweep frees an entry, the next `push` would reissue a key some live
capability already names, and `Resource::Endpoint { channel }` is the only
handle a component holds. That would have converted an exhaustion bug into
confused-deputy redirection, which is strictly worse.

The sweep is lazy, firing on `TableFull` and retrying, for B16's reason: one
trigger condition is one thing to keep correct, and a channel that stays is a
channel that still works.

**Exit condition (observed 2026-08-07):** `just sel4_crossing_check` boots a
graph that mints 33 pairs against a 32-entry table and still sends and receives
on every live channel, including a pair held across the crossing and an end
parked in `Transit` across it. The transcript records the first sweep as
`freed=28 live=4 minted=32` and the terminal line as `minted=37`; what the gate
*asserts* is looser and deliberately so — a nonzero `freed` on the sweep line
and a terminal `minted` in 33..=99, since pinning exact counts would break on
unrelated allocator changes while the loop-vs-bound arithmetic is enforced
separately from source. Three fault injections confirmed failing:
removing the sweep dies at the 33rd mint, removing the `Transit` half of the
predicate loses the in-flight end, and restoring `key = self.len` trips the
gate's key-derivation source check. The other eight seL4 gates,
`just generation_check`, and `just contracts_check` are unchanged. See
[`devlog/2026-08-07-p5-4-1-oracle-inventory/`](../devlog/2026-08-07-p5-4-1-oracle-inventory/index.md).

**Follow-up opened:** [B24](#b24--sharedbuffertablequotas-never-reclaims-so-max_charge_holders-is-a-lifetime-bound),
a third table of the same shape found by the same class audit.

### B21 — the toolchain was pinned by name, so each host resolved a different binary — **resolved 2026-08-06**

**Problem:** `flake.nix` pinned the seL4 cross toolchain by *name*
(`CROSS_COMPILER_PREFIX = crossCC.targetPrefix`), and `build-sel4.py` passed
that bare prefix to CMake, which resolves `${prefix}gcc` through `PATH`. A name
is not an identity. `pkgsCross.aarch64-multiplatform.stdenv.cc` is a *cross*
wrapper on `aarch64-darwin` and `x86_64-linux` but a *native* wrapper on
`aarch64-linux`, where `targetPrefix` is empty and `bin/` contains no
`aarch64-unknown-linux-gnu-`-prefixed entry. The prefixed lookup therefore
skipped that wrapper and found the **unwrapped** GCC its own `setup-hook` had
put on `PATH` — a different compiler driver *and* a different assembler,
selected by `PATH` order rather than by anything pinned.

**This corrects B20's recorded root cause.** B20 attributed the divergence to
Darwin's wrapper injecting `-fno-omit-frame-pointer` where `aarch64-linux`
"forces neither". Both wrappers ship a byte-identical
`nix-support/cc-cflags-before`; nixpkgs emits it for every non-x86-32,
non-s390 target. B20's two pre-fix hashes, `e8cbab4f…` and `f2d316e1…`, differ
by *driver*, not by host: both are reproducible on one machine by choosing the
wrapped or unwrapped compiler.

**Resolved by** exporting `CROSS_COMPILER_PREFIX` as an absolute
`"${crossCC}/bin/${crossCC.targetPrefix}"` store path, so every host runs the
same driver and assembler. This is the fix B20 proposed and rejected as
"larger, with a worse failure mode"; that rejection rested on a false premise.
`crossCC` is the same derivation each platform already evaluates and installs,
so nothing new is fetched and no pinned hash moves. `just sel4_pin_check` now
fails if the bare form returns — the prefix pin cannot catch this itself, since
it reports "toolchain drift" without naming which host is odd.

B20's `-fomit-frame-pointer -momit-leaf-frame-pointer` are **kept**. Fault
injection shows they close a *different* leak than the one B20 recorded: with
the toolchain pinned but the flags removed, the hosts still diverge in
`.debug_line` alone (`e8cbab4f…` vs `4c694979…`, both 982208 bytes, every ALLOC
section equal), because GAS's DWARF-5 view numbering for the extra prologue row
is not host-independent. That binutils behavior is masked, not fixed.

**Exit condition (observed 2026-08-06):** `kernel.elf` rebuilt from scratch on
`aarch64-darwin` and `aarch64-linux` is `97dcb029…`, 973184 bytes on both —
**unchanged** from the recorded pin, now depending on the toolchain rather than
on `PATH`. `CROSS_COMPILER_PREFIX` resolves to the wrapper on `aarch64-linux`
instead of being empty. `just sel4_qemu_image_check` passes on `aarch64-darwin`,
and the new guard is fault-injected: reverting to `crossCC.targetPrefix` fails
`just sel4_pin_check`. `x86_64-linux` was not re-observed; its prefix was
already the cross form, so the change is expected to be a no-op there
(**[INFERENCE]**). Both hosts are on one machine, one virtualized — the right
test for toolchain and `PATH` independence and no evidence about physical
boards. See `devlog/2026-08-06-b21-cross-toolchain-binary-selection/`.

### B16 — a supervision termination record was never reclaimed, so a long-lived graph exhausted the table — **resolved 2026-08-07**

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

**Deferral re-reviewed 2026-08-05, before opening P5.5.2's gate.** Still
deferred, on the same observation, and this is the largest graph the cutover
declares: P5.5.2's stream plane creates thirteen tasks — seven launched, six
spawned — against `MAX_RECORDS = 32`. The bound is approached more closely than
by any earlier slice and still not reached. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

Worth stating plainly, since the margin is now under 3×: this stays a latent
bound rather than a defect only because every declared generation runs to
completion and exits. A long-lived graph that spawns and reaps repeatedly is
what makes it bite, and P5.4 — which retires the oracle — is the point at which
"every declared generation" stops being a safe quantifier.

**Why deferred rather than fixed in P5.3.3:** the counting version touches every
path that installs or releases a capability, and the refusal version needs a
gate whose graph spawns past the record table to prove it. Neither is a line;
both want the multi-child graph P5.3.4 composes.

**Exit condition (observed 2026-08-07):** a graph that creates more than
`MAX_RECORDS` tasks over its lifetime still answers `supervision_status`
correctly for every live handle, observed under `just sel4_supervision_check`,
with the nine existing seL4 gates passing. (The entry said *five*; there were
nine by the time it was closed.)

**Fix: a derived sweep, which is neither option this entry proposed.** The
refusal was rejected on the entry's own terms — refusing the spawn makes the
graph the exit condition requires impossible to observe, so choosing it would
mean amending the condition in the same change that claimed to meet it. The
reference count was unnecessary: the live-holder set is already represented, so
`supervision::sweep` derives it, reclaiming every record no live holder can
name. Same choice, same reason, as `TaskTable::live_children`, and it fails
safe — a sweep that does not run leaves a record that still answers correctly,
whereas a missed decrement loses one forever.

The predicate reads **two** holders. A supervision handle in flight is held by
no capability table at all, so a sweep consulting only `GraphTables` would free
a record mid-transfer and leave the receiver waiting forever: this defect,
reintroduced by its own fix. `Transit::holds_supervision` is the second half,
and fault injection #2 below is what proves it is load-bearing.

The residual case is now reported rather than silent: if every record has a live
holder, `record_termination` emits
`SLIME_GRAPH FAIL termination lost task={} reason=records-full`, matching
`unland_caps`'s convention. That is what closes the *silent*-loss defect rather
than merely raising the bound.

**Observed:** 35 tasks created over one boot, `terminated=38` against
`MAX_RECORDS = 32`, with `freed=30 live=3` at the sweep — the retained handle,
the in-flight handle, and the current record all preserved. Two fault
injections, both confirmed failing: removing the sweep fails at
`termination lost task=33 reason=records-full`; removing only the `Transit` half
of the predicate fails at `a handle parked across the crossing lost its
outcome`, with every earlier marker still passing. See
`devlog/2026-08-07-b16-supervision-records/`.

### B20 — the prefix pin held for one platform at a time — **resolved 2026-08-06**

**Problem:** B19 made `kernel_sha256` independent of the dev *shell*; it was
still per-*platform*. `aarch64-darwin` produced `e8cbab4f…` and `aarch64-linux`
produced `f2d316e1…` from the same checkout, the same `flake.nix`, and the same
pinned seL4 source and config.

The cause was the toolchain, not a leak. `flake.nix` names
`pkgsCross.aarch64-multiplatform.stdenv.cc`, which resolves to a **cross**
`gcc-wrapper` on Darwin and a **native** `gcc` on `aarch64-linux` — the
empty-`targetPrefix` fact B19's analysis recorded, seen from the other side.
Darwin's `nix-support/cc-cflags-before` forces
`-fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`, so every function
prologue differed. Because that file lives inside the wrapper derivation rather
than the environment, B19's scrub could not reach it.

**Resolved by** having the build state its own frame-pointer policy:
`-fomit-frame-pointer -momit-leaf-frame-pointer` joins the prefix maps and the
fixed seed in `CMAKE_C_FLAGS`/`CMAKE_ASM_FLAGS`. This is a policy the build
**chooses**, not a compiler default it restores, and it moves *both* platforms:
GCC's aarch64 backend disables `-fomit-frame-pointer` at every `-O` level, so an
aarch64 kernel keeps its frame pointers at `-O2` unless the flag is explicit.
(`-Q --help=optimizers` claims otherwise at `-O2`; that is a reporting trap, and
it is what an earlier draft of this entry got wrong.) The choice is sound because
seL4 states no frame-pointer preference and nothing walks one — the AArch64 trap
path's `x29` uses are full register-context saves indexed off `sp`, and
`Arch_userStackTrace` scans `SP_EL0` linearly. `-momit-leaf-frame-pointer` is
belt and braces: under `-fomit-frame-pointer` it changes no emitted code, and it
is kept only because it names the second of the wrapper's two injections.

Darwin's two other injections need no counter-flag: `-march=armv8-a` is what seL4
passes itself and what both compilers default to, and the glibc/gcc
`-idirafter`/`-B` paths reach nothing in a `-nostdinc -ffreestanding -nostdlib`
build.

Naming one cross toolchain for every system — B20's own proposed fix — was
rejected as larger, with a worse failure mode, and moving the pin for a reason
unrelated to the defect. It remains the stronger fix and is now optional.

`kernel_sha256` is re-observed as `97dcb029…` on **all three platforms tested**.

**Exit condition (observed 2026-08-06):** `kernel.elf` built on
`aarch64-darwin`, `aarch64-linux`, and `x86_64-linux` are **byte-identical** by
`cmp`, each 973184 bytes at `97dcb029…`, from three different dev-shell seeds
(`r279wlb3cq`, `65gzz0x3v8`, `6ckb6q72lb`), with all nine `sel4_*` Justfile gates
passing. `x86_64-linux` is the case that matters most:
there `pkgsCross.aarch64-multiplatform.stdenv.cc` is a genuine *cross* wrapper as
on Darwin, rather than the native `gcc` `aarch64-linux` resolves, so both wrapper
shapes agree. B19's property still holds on each: a real-shell build and a
hostile-environment build are byte-identical. Fault-injected symmetrically —
replacing the flag string with `""` reverts Darwin to `e8cbab4f…` and
`aarch64-linux` to `f2d316e1…`, the exact pre-B20 divergence. Both Linux hosts
are containers under a macOS hypervisor, one of them emulated, not separate
hardware — the right test for toolchain independence and no evidence about
physical boards. See `devlog/2026-08-06-b20-cross-platform-kernel-identity/`.

**Root cause superseded by B21 (2026-08-06).** The mechanism recorded above is
wrong. Both wrappers ship a byte-identical `cc-cflags-before`; the divergence
was `PATH`-order *binary* selection, not a per-platform wrapper policy, and the
two pre-fix hashes differ by driver rather than by host. The "stronger fix …
now optional" is implemented and moved no hash. The frame-pointer flags are
kept, for a residual `.debug_line` leak this entry did not identify. See the
B21 entry above and
`devlog/2026-08-06-b20-cross-platform-kernel-identity/index.md`'s
`## Corrections`.

### B19 — the seL4 prefix pins bound the dev-shell derivation hash, not the toolchain — **resolved 2026-08-06**

**Problem:** `sel4/pins.toml`'s `[observed_prefix]` is the gate that would
notice a change of seL4 compiler, and it pinned the **dev shell's own derivation
hash** instead. `configure_and_install_sel4` inherited `os.environ`, and nixpkgs
puts `-frandom-seed=<first 10 chars of the devShell derivation hash>` into
`NIX_CFLAGS_COMPILE`; GCC seeds symbol and section naming from it, so adding a
tool to `flake.nix` — or reordering the list — changed `kernel.elf` byte-for-byte
and was reported as toolchain drift. The same variable carried per-package
`-isystem` store paths, and `NIX_HARDENING_ENABLE` imposed
`-fstack-protector-strong`, `-fzero-call-used-regs`, and `_FORTIFY_SOURCE=3` on a
freestanding kernel whose own `CMakeLists.txt` asks for `-fno-stack-protector`.

**Resolved by** making the kernel build independent of the shell rather than by
re-pinning per host. `sel4_build_environment` builds the environment from
`os.environ` minus every flag-carrying `NIX_*` variable, the `CFLAGS`-family
names CMake seeds `CMAKE_<LANG>_FLAGS_INIT` from, the bintools wrapper's
`NIX_SET_BUILD_ID`/`NIX_BUILD_ID_STYLE` switches, and
`CMAKE_INCLUDE_PATH`/`CMAKE_LIBRARY_PATH`/`CMAKE_PREFIX_PATH`; a fixed
`-frandom-seed=slime-sel4-qemu-arm-virt` replaces the shell's seed. The scrub
matches by *prefix* because the cc-wrapper reads target- and role-mangled
spellings (`NIX_CFLAGS_COMPILE_aarch64_unknown_linux_gnu`, `_FOR_BUILD`,
`_FOR_TARGET`) rather than the base names.

Of the exact-name groups, only the search paths were a live route:
`CMAKE_INCLUDE_PATH` is prepended to `find_file` order, which no `-D` protects,
and seL4 resolves `KERNEL_HELPERS_PATH` that way. The rest are defense in depth
and are labelled so in the code rather than described as leaks.

`kernel_sha256` was re-observed as `e8cbab4f…` on `aarch64-darwin` — **since
superseded by B20's `97dcb029…`**, which is the same kernel built with the
frame-pointer policy stated rather than inherited. The other four
pinned artifacts were already reproducible and are unchanged. The hash still
binds `cmake`, `ninja`, and the host Python generators, which this file does not
pin — recorded as a residual in the devlog, not claimed as closed.

**Exit condition (observed 2026-08-06):** `just sel4_qemu_image_check` passes,
and adding `hexdump` to `flake.nix`'s `packages` moves the shell's seed from
`r279wlb3cq` to `rhl1f441df` while leaving `kernel_sha256` byte-identical. A
third build with a fabricated seed, fake `-isystem` store paths, a narrowed
hardening set, and an ambient `CFLAGS` is byte-identical too. Fault-injected:
one nibble changed in `kernel_sha256` makes the gate exit 1.

**A second host was then observed, on `aarch64-linux` under OrbStack** (shell
seed `65gzz0x3v8` against Darwin's `r279wlb3cq`). B19's property holds there —
a real-shell build and a hostile-environment build are byte-identical — but at
`f2d316e1…` rather than `e8cbab4f…`, because Darwin resolves a *cross*
`gcc-wrapper` that forces `-fno-omit-frame-pointer` while `aarch64-linux`
resolves a *native* `gcc` that does not. That is a genuine toolchain difference,
which is what the gate exists to catch, so the pin stands as recorded. It does
mean `[observed_prefix]` is **per-platform**; that was opened as B20 rather than
folded in here, and B20 is now resolved — both platforms produce a
byte-identical `97dcb029…`. See
`devlog/2026-08-06-b19-sel4-prefix-pin-shell-coupling/`.

### B18 — the seL4 stream gate was scheduling-dependent — **resolved 2026-08-06**

**Problem:** `just sel4_stream_check` passed roughly one run in three. Two
independent causes, both invisible on x86 because the retired kernel's
cooperative scheduler orders the events favourably every time.

**Cause 1 — a publisher writing to a route it had already retired.**
`fabric-publisher-b` sent its first `diagnostics` sample with `FLAG_LAST` and
then published on that route again after the large telemetry sample. That second
send was **dead code**: `FLAG_LAST` sets `publisher.finished`, and both the
broker loop and `park_on_streams` skip a finished publisher, so nothing ever
read it. Worse than inert — once `diagnostics` retired, only `telemetry` kept
the fabric alive, so after that drained the send answered `ERR_PEER_DEAD`, which
`publish` treats as fatal. Deleted.

**Cause 2 — `debug_write` was one syscall per byte.** Under `PRINTING` the
component-side implementation called `seL4_DebugPutChar` per character,
bypassing the root entirely. The root's own `debug_println!`, or another
component's line, could land mid-string: the transcript showed ` QoS matched`
where `[fabric] QoS matched` was written, and whichever gate required the
destroyed marker failed on a boot that was otherwise correct.

This was the larger cause, and it masqueraded as several different bugs — a
missing `re-delegation denied`, a missing `large sample published`, and (because
a corrupted `QoS matched` changes what the transcript appears to say about
matching) an apparent provisioning race. Diagnosing it as one defect rather than
three took reading full transcripts rather than the gate's 40-line tail.

`Operation::DebugWrite` is now served by the root's graph loop, which is
single-threaded and answers one request at a time, so a line printed inside that
arm cannot interleave with anything. Atomicity is structural rather than a
matter of timing. The cost is that printing now needs a bound transfer window;
every launched component binds one before it runs.

**Two fixes were tried and reverted**, both recorded because each looked
plausible and each made things worse:

- **Moving `FLAG_LAST` to the second diagnostics sample**, where the route
  genuinely ends. Wedges `just fabric_qos_check`, whose subscriber waits for the
  terminal event the early flag produces.
- **Making the stall stop acking.** `receive_large_sample` acks the inline
  samples it passes over, which does drain the ring the stall is supposed to
  overrun — but removing the ack wedges the fabric outright, because it waits
  for a delivery slot that never frees. The ack is load-bearing.
- Narrowing `fabric-subscriber-b`'s declared `historyDepth` from 4 to 2 also
  failed, and for the same underlying reason as everything else: the failures
  were marker corruption, not ring arithmetic.

**Exit condition (observed):** ten consecutive `just sel4_stream_check` runs
pass, with all six other seL4 gates, `just fabric_stream_check`,
`just fabric_qos_check`, `just fabric_visibility_check`, and
`just data_fabric_boot_check` unchanged. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B17 — the capability transfer's subset test had no coverage — **resolved 2026-08-05**

**Problem:** `slime-root/src/main.rs::serve_cap_transfer` enforces four rules,
and P5.5.1's gate observed three. The fourth — the **subset test**,
`rights & !source.rights != 0`, which is what makes the move narrow-only
against *what the holder actually has* — was not observed: deleting it left
every marker in that gate intact.

**The entry's stated reason was wrong, and that is the interesting part.** It
argued the property was unreachable from any graph this cutover could declare,
because reaching it needs a capability holding transfer authority while being
strictly narrower than its kind admits, and `cap_transfer` with
`FLAG_RETAIN_TRANSFER` was "the only thing that produces one" — which a
component cannot use on itself, since the two ends of a channel it holds alone
are a loopback the root refuses to split.

A plain **spawn grant** produces one. `preflight_spawn_grants` installs the
requested mask verbatim, so `grant(endpoint, RIGHT_SEND | RIGHT_TRANSFER)`
yields exactly send+transfer where `Endpoint` admits send+recv+transfer.
Init already does this on x86 for `DANGO_OUTPUT_SLOT` — the shape existed in the
tree the whole time; nobody had asked to widen one. The gap was a missing arm,
not an unreachable property, and the analysis that said otherwise was checking
`cap_transfer`'s own outputs rather than every path that installs a mask.

**Resolution:** `sel4-stream.zti` grants `fabric-publisher` a second endpoint
end at send+transfer, carrying no traffic and belonging to no route. It goes to
the publisher because that component already carries the other two
transfer-rule denials, so all three sit together and each states which rule it
proves. The component asks to move it with `recv` restored: that passes the transfer-authority rule,
passes the descriptor/kind rule, and computes zero against the per-kind mask, so
only the subset test can refuse it.

The arm is guarded on **holding** the subject rather than on a check flag,
because an empty slot answers the same `ERR_BAD_CAP` the subset test does — a
bare widening arm would pass identically in a graph that never granted the
endpoint, which is the "looks like coverage and is not" failure this item was
opened for. It establishes possession by *using* the granted end first, so a
graph without one skips silently and claims nothing.

**Exit condition (observed):** `just sel4_stream_check` observes the refusal,
and removing `rights & !source.rights` from `serve_cap_transfer` fails that gate
— the fault injection P5.5.1 ran and could not make fail. See
`devlog/2026-08-05-p5-5-2-stream-plane/`.

### B15 — a spawn carries at most four grants on seL4, against the oracle's sixty-four — **resolved 2026-08-05**

**Was:** `slime-root`'s spawn read its grant array through
`transfer_window::read_staged`, whose bound is `ipc::MAX_MESSAGE_BYTES` (64). At
`SPAWN_GRANT_RECORD_BYTES` = 16 that is **four** records, against the retired
kernel's sixty-four. Real x86 callers already exceeded it —
`init.rs::GENERATION_MANAGER_CAPS` and `dango_caps()` are six each, and
`launch_fabric_graph` hands the fabric nine — so a component that runs on the
retired kernel would have failed to launch its children on the cutover, which is
the one property P5.4 must be able to claim.

**Fixed by** a second staged bound rather than a wider message.
`transfer_window::MAX_STAGED_ARRAY_BYTES` (1024) bounds an *array* staged
through a window, where `MAX_STAGED_BYTES` bounds a *message*; the two stay
separate numbers because a `send` payload becomes an `ipc::Message` and is that
wide by construction, while a grant array becomes no message at all. The
component side needed no change: `sel4_transport::spawn` already encoded into a
`MAX_SPAWN_GRANTS * GRANT_RECORD_BYTES` buffer and staged it into a 4096-byte
window, so the refusal was entirely root-side.

**Exit condition observed 2026-08-05** under `just sel4_spawn_check`: `init`
spawns `sysinfo` with **six** grants — B15's own number, and the size of this
repository's largest real grant lists — and all six ends move, each granted slot
leaving init's table while each retained half still sends. Fault-injected: with
the narrow reader restored the spawn is refused outright and the gate fails. See
`devlog/2026-08-05-p5-5-1-typed-fabric/`.

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
