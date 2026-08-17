# B59, B66 — one contract for the syscall ABI, and 97 rights declarations becoming one

| Field | Value |
|---|---|
| Date | 2026-08-17 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/syscall-abi/v1/`, `contracts/fabric-graph/v1/schema.zt`, `components/proto/src/syscall_abi.rs`, `components/runtime/src/syscall.rs` + `syscall/sel4_transport.rs`, `slime-root/src/{main,ipc,graph,generation,console,directory,peer_endpoint}.rs`, 14 userspace components, `scripts/build/build-generation.py`, `docs/{syscall-abi,capability-matrix}.md` |
| Roadmap | B59, B66, B57, B46 |
| Gates | `just contracts_check`, `just generation_check`, `just test_host`, `just sel4_boot_check`, `just data_fabric_profile_check` |
| Trigger | B57 fixed the rights *predicate* but left the duplication; the structural audit had counted 97 declaration sites and four hand-synchronized tables |
| Baseline | Four number tables crossing the root/userspace boundary, each hand-authored once and re-typed elsewhere, agreeing by discipline only |

## Summary

Four tables crossed the root/userspace process boundary with no single source:
23 rights names across **97** declaration sites, the 22-entry operation-label
table in two full copies plus prose, the 5 status codes in two copies plus the
same prose, and the 16-byte spawn-grant record as two constants joined by a doc
comment reading "Matches ...". A new `contracts/syscall-abi/v1` now declares the
labels, statuses, message bounds, and record layout and generates one module both
crates consume; rights consolidated onto B57's generated vocabulary. 97 sites
became 1, and that one is a `u64::MAX` sentinel rather than a named right. B66
fell out of the same work: `ipc.rs`'s wait-source ceiling was a third spelling of
a value `contracts/fabric-graph/v1` already declared and the builder already
imported. Both new guards were verified to bite by mutating the contract.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/syscall-abi/v1/{schema,gen_rust}.zt` | New contract: operations with pinned labels, statuses, message bounds, spawn-grant record layout. Validator rejects duplicate labels, duplicate codes, an operation in an undeclared service, and a record whose fields do not exactly fill it | The ABI has one declaration |
| `components/proto/src/syscall_abi.rs` | Generated; registered in `slime-proto` | Both crates read one module |
| `components/runtime/src/syscall.rs` | Label modules and `ERR_*` re-exported from the generated module instead of declared; `SpawnGrant` lost `#[repr(C)]` | The wrapper stops owning the numbering |
| `components/runtime/src/syscall/sel4_transport.rs` | Encodes grants through `GRANT_SLOT_OFFSET`/`GRANT_RIGHTS_OFFSET` | The record layout is declared, not literal |
| `slime-root/src/main.rs` | Imports the same label modules; grant decode uses the same offsets; `SPAWN_GRANT_RECORD_BYTES` is the generated constant | Dispatcher and wrapper cannot disagree |
| `slime-root/src/ipc.rs` | `slime_status` returns generated `ERR_*`; message bounds re-exported from the contract; `CHANNEL_CAPACITY` deleted; `MAX_WAIT_SOURCES` re-exports `fabric_graph::MAX_INGRESS_SOURCES` (B66) | One error table, one ceiling |
| 20 Rust files | 69 `u64`/`Rights` rights declarations became imports from `boot_contracts::generation`; 23 `u32` ones became narrowing aliases over the same constants | One rights vocabulary |
| `components/proto/Cargo.toml`, `components/runtime/Cargo.toml` | `boot-contracts` as a proto dev-dependency; `slime-proto` as a runtime dependency (acyclic) | The generated module is reachable from both sides |
| `scripts/generate/generate-syscall-abi-bindings.py`, `Justfile` | `just syscall_abi_gen`; wired into `contracts_check` | Staleness is a gate failure |
| `docs/capability-matrix.md`, `roadmap/README.md` | Provenance paragraph made true; invariant 4 now says both couplings are gated | The docs describe what is enforced |
| `build-generation.py` + 4 more files | Nine `SYS_WAIT` mentions retired — the syscall no longer exists; the builder's refusal now reports actual numbers | Comments name mechanisms that exist |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| An operation is silently renumbered, invalidating older component images | `just test_host` → `components/proto/tests/syscall_abi.rs::operation_labels_are_frozen` | Pinned label mismatch, naming the operation |
| Two operations share a label | contract validator + `operation_labels_are_distinct` | Generation emits `INVALID_SYSCALL_ABI_SCHEMA`; test fails |
| A status code becomes non-negative and reads as success | `status_codes_are_frozen` | Asserts every error `< 0` |
| The grant record layout drifts between crates | `spawn_grant_record_layout_is_frozen` | Offsets or size mismatch, or fields not filling the record |
| `docs/syscall-abi.md` misses a renumbered operation | `just contracts_check` | `syscall-abi.md does not document declared operations: N (\`OP\`)` |
| Generated bindings drift from the contract | `just contracts_check` | `generated … is stale; run just syscall_abi_gen` |
| A rights bit is re-added by hand somewhere | B57's `right_all_is_a_union_of_named_bits_and_excludes_the_gap_at_17` plus one vocabulary | Union mismatch |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just contracts_check` | pass — includes "docs/syscall-abi.md documents all 23 declared operations" | Direct |
| `just generation_check` | pass — two isolated builds byte-identical | Direct |
| `just test_host` | pass — 12 suites, including the new `syscall_abi` (5 tests) | Direct |
| `just test_sel4_root` | pass — 118/118 | Direct |
| Guard bites: renumber `EXIT` 3→7 in the contract, regenerate | `operation_labels_are_frozen` aborts (`panic = "abort"`); reverted | Direct |
| Guard bites: change doc label 32 row to 99 | `syscall-abi.md does not document declared operations: 32 (\`DERIVE\`)`; reverted | Direct |
| Rights census after migration | 26 generated + 23 narrowing aliases + 1 `u64::MAX` sentinel; was 97 hand-written | Direct |
| Ceiling census after B66 | Exactly one declaration: `maxIngressSources = 9` → one generated constant | Direct |
| `just sel4_root_boot_check`, `just sel4_boot_check` | pass — 30 markers, 5 chains, 21 slots, 19 tasks | Direct |
| `just sel4_dango_check`, `sel4_powerbox_check`, `sel4_filesystem_check`, `sel4_directory_check` | pass — exercise all four `u32` narrowing sites on real boots | Direct |
| `just sel4_visibility_check`, `sel4_matrix_check`, `sel4_stream_check`, `sel4_spawn_check` | pass — the migrated brokers and spawn path | Direct |
| `just sel4_boot_layout_check`, `sel4_gate_control_check`, `sel4_capability_layout_check`, `architecture_contract_check` | pass | Direct |
| `just data_fabric_profile_check` | pass — after regenerating the fallback profile | Direct |
| `just ruff`, `just typos`, `just fmt_check_all`, `just lint_all` | pass | Direct |

## Decisions

- Decision: put the labels/statuses/bounds in a new `contracts/syscall-abi/v1`,
  but leave rights in `contracts/generation/v5`.
  Rationale: rights are a *field of the generation records* (`grantLayout`'s
  `rights 8`) that B57 already declared there, and moving them would churn the
  generation format for no gain. Labels are not generation data at all.
  Rejected alternative: one combined "ABI" contract — it would have coupled the
  generation wire format's version to the syscall table's.

- Decision: declare the Rust module name (`capability_table_labels`) beside the
  service name (`capabilityTable`) rather than deriving it.
  Rationale: the camel→snake transform has no inverse the renderer can check, so a
  renaming bug would be silent. Same reasoning as B57's `manifest` field.

- Decision: **verify** `docs/syscall-abi.md`'s operation table rather than
  generate it. Rationale: its rows carry per-operation operand layouts and result
  conventions the ABI declaration does not model. Generating it would have deleted
  real documentation to satisfy a rule. Checking the label column still removes
  the manual coupling invariant 4 describes.
  Rejected alternative: generating the table and keeping prose elsewhere — the
  operand column is per-row, so it cannot move out without losing its association.

- Decision: give `components/proto` a `boot-contracts` **dev**-dependency for the
  powerbox test. Rationale: the test should assert against the generated rights
  vocabulary, but the library must stay dependency-free so `slime-rt` can depend on
  it. Dev-only keeps both.

- Decision: keep the `MAX_WAIT_SOURCES` alias in `ipc.rs` rather than renaming the
  call site to `MAX_INGRESS_SOURCES`. Rationale: the admission site reads more
  clearly in terms of the ceiling it enforces; the point of B66 was to remove the
  second *declaration*, not the local name.

- Decision: retire the `SYS_WAIT` wording in the same change.
  Rationale: nine sites named a syscall the seL4 cutover deleted, including the
  error message a developer would hit. A comment naming a mechanism the reader
  cannot find is worse than no comment. Bundled because B66 is exactly the "this
  describes a retired mechanism" defect class.

- Decision: `SpawnGrant` loses `#[repr(C)]`.
  Rationale: the structural audit initially flagged it as an unschema'd wire type.
  It is not — the transport encodes field by field — so the attribute was
  *claiming* an ABI role the generated offsets actually hold. Removing it makes
  the type honest rather than adding a schema for a non-wire struct.

## Open risks and follow-ups

- [ ] B60–B65 remain open. B60 (authority policy in the builder) is next.
- [ ] The label freeze test is a hand-maintained list, so adding an operation
  requires editing the contract *and* the test. That is deliberate — the second
  edit is what makes an ABI change reviewable — but it is O(operations) hand
  maintenance of the same shape B63 flags for gate marker counts.
- [ ] **[INFERENCE]** `sel4::Word` is `u64` on every admitted target profile, so
  the generated `u64` labels are the dispatcher's own type. That is read from
  `deps/rust-sel4`'s `pub type Word = sys::seL4_Word` plus the fact that every
  profile in `contracts/target-profile/v1` is AArch64; a 32-bit profile would need
  the labels widened at the call site. Not currently reachable.
- [ ] `docs/syscall-abi.md`'s *console* operation table (labels 0–4) is a separate
  namespace on the console endpoint and is still hand-written on both sides
  (`sel4_transport.rs`'s `CONSOLE_LABEL_*` and `slime-root/src/console.rs`). Same
  defect class, smaller: 5 labels, one sender, one receiver. Not folded in because
  the console endpoint is not the root service endpoint and would want its own
  contract section; worth doing if a third console operation appears.

## Artifacts and provenance

- Focused report: none; the audit that opened B59 and B66 is
  [the structural audit entry](../2026-08-17-structural-audit/index.md), and B57 —
  whose rights vocabulary this builds on — is
  [here](../2026-08-17-b57-b58-rights-vocabulary/index.md).
- Raw transcript: none preserved; every count is reproducible with
  `grep -rn "const RIGHT_"` over `slime-root/src`, `boot-contracts/src`, and
  `components`, and each gate result from its named `just` target.
- Serial/debugger/model output: none quoted; the boot gates' own marker summaries
  are recorded in *Verification*.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md) —
  B59 and B66 in the resolved log; B60–B65 open.
