# C9.5: recorded means captured, and the one grant that carries the recording

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/{fabric-trace/v1,recording-policy/v1,generation/v1,generation/v5,syscall-abi/v1}`, `boot-contracts/src/{recording_policy,generation}.rs`, `components/proto/src/{recording_stream,lib}.rs`, `components/{runtime,lib,bins/replay-probe,bins/init}`, `slime-root/src/{generation,ipc,main}.rs`, `scripts/{build,check}`, `docs/{syscall-abi,capability-matrix}.md` |
| Roadmap | C9.5, C9, C9.1, C9.2, C9.4, C8.11, C8.15, B23, B57, B70, B71 |
| Gates | `just replay_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check`, `just contracts_check`, `just generation_check`, `just test_sel4_root`, `just test_host` |
| Trigger | C9.5 was the first actionable roadmap milestone: the backlog's Open section is empty, and M5.7/RP3/RP4 are hardware-blocked. |
| Baseline | C9.1–C9.4 complete. No generation could declare a component deterministic, no nondeterminism source was classified, and nothing recorded or replayed a typed trace. |

## Summary

A generation can now declare a component deterministic, and that claim is
constrained rather than asserted. Four kinds added to `fabric-trace/v1` carry
what a deterministic component observes — clock reads, timer expiries, lifecycle
transitions — plus the typed outputs the claim is made about; a bounded machine
in `slime-proto` records them without allocating and replays them only after
validating the whole stream. The half that gives the claim meaning is the
classification: every capability right in `contracts/generation/v5` carries a
required `determinism` field, and a component holding any authority no recorder
captures cannot be declared deterministic. `just replay_check` boots one image
twice and compares the declared traces; the recorded run and its replay agree on
every typed output across both boots.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/fabric-trace/v1` | Four kinds (11 `clockRead`, 12 `timerExpiry`, 13 `lifecycle`, 14 `output`), a clock-source vocabulary, and lifecycle/output ceilings; `maxKind` 10 → 14 | One trace, so records of different families can be ordered against each other |
| `components/proto/src/lib.rs` | Per-kind field rules for the four families: every field a family does not use is fixed, `flags` included | A recording is comparable only if unused bytes cannot vary |
| `contracts/generation/v5` | `determinism` is a required field on every right; the renderer refuses to emit an unclassified one and folds `RIGHT_RECORDED`/`RIGHT_UNRECORDED` | Totality is structural: a right added without a class does not type-check |
| `contracts/recording-policy/v1` (new) | Per-instance stream participation, role, determinism claim, record capacity, and the one declared `streamGrant` | The determinism claim and its single exception are both declared data |
| `components/proto/src/recording_stream.rs` (new) | `Recorder`/`Replay` over fixed arrays; whole-stream validation before any input is exposed; single-use recorder | "Refused rather than partially replayed" is a property of the code, not an intention |
| `slime-root/src/generation.rs` | Locator, admission, and the rights join over the instance's own bindings under `grant_applies_to_instance`, minus the declared stream grant | The root re-derives the refusal from the encoded generation rather than trusting the builder |
| `slime-root/src/main.rs` | Label 57 handler, and `CAPABILITY IMPORT` refuses unrecorded authority for a deterministic receiver | A certified claim stays true after launch, not only at it |
| `contracts/generation/v1/fixtures/sel4-replay.zti` | Generation 45, boot action 35, five probe instances | Every arm of the plane is non-vacuous declared data |
| `scripts/check/check-sel4-replay-plane.py` (new) | Two boots, marker chains, fixture-shape checks, and a declared-trace comparison | The milestone's first required check is observed rather than argued |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A right is added without a determinism class | renderer `allClassified` | `INVALID_GENERATION_SCHEMA` from `just boot_gen` |
| The two masks stop partitioning the vocabulary | `just test_host` | `the_determinism_masks_partition_every_declared_right` |
| A deterministic component is granted an unrecorded source | `just generation_check`, admission | build failure naming the right; `UnsatisfiableRecordingPolicy` |
| The declared exemption names nothing, or a recorder claims one | `just replay_check` fixture shape + builder | build failure naming the grant |
| A truncated, reordered, or over-capacity stream is partially replayed | `just replay_check`, `just test_host` | `[replay] FAIL`; `every_truncation_is_refused_…` |
| A terminal-flagged record of the wrong shape opens a stream | `just test_host` | `a_noncanonical_terminal_is_refused_before_any_input_is_exposed` |
| A refused replay step advances the cursor | `just test_host` | `a_refused_clock_read_does_not_move_the_cursor` |
| Runtime import widens a deterministic instance | `just replay_check` | missing `SLIME_RECORD refused import` |
| The plane's markers stop being load-bearing | `just sel4_gate_control_check` | 26-marker pin, delete/reorder/failure controls |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just replay_check` | Pass: two boots, identical declared traces, `SLIME_GRAPH HEALTHY generation=45 required=6 live=0 completed=6 failed=0` | Direct |
| `just sel4_gate_control_check` | Pass: 40 gates reject 1556 mutations (was 39/1518) | Direct |
| `just sel4_boot_layout_check` | Pass: 31 plane layouts match their fixtures | Direct |
| `just contracts_check` | Pass: 44 declared operations documented | Direct |
| `just generation_check` | Pass: byte-identical isolated builds | Direct |
| `just test_sel4_root` | 183/183 across 19 modules | Direct |
| `just test_host` | Pass, including 21 `recording_stream` and 18 `recording_policy` tests | Direct |
| `just clock_authority_check`, `wait_set_check`, `scheduling_class_check`, `lifecycle_restart_check` | Pass: no C9 sibling regressed | Direct |
| `just lint_all`, `fmt_check_all`, `ruff`, `typos`, `deny`, `machete` | Pass | Direct |
| Fixture mutation: `replay-unrecorded` → `deterministic = true` | Refused, naming `blockRead` | Direct |
| Fixture mutation: drop the replayer's `streamGrant` | Refused, naming `recv, send` | Direct |
| Fixture mutation: `streamGrant` naming an unbound grant | Refused | Direct |
| Fixture mutation: a recorder declaring a `streamGrant` | Refused | Direct |

## Decisions

- **Decision:** The nondeterminism classification is a required field on each
  right in `contracts/generation/v5`, not a list in `recording-policy/v1`.
  **Rationale:** C9.5's refusal is sound only if *every* right is classified. A
  list can be partial and the gap is silent — an unclassified authority reads as
  harmless, and the first component granted it is certified deterministic against
  a table that never considered it. A required field cannot be partial.
  **Rejected alternative:** three name-set lists in the recording contract with a
  builder-side completeness check; that check existed and worked, but it was a
  guard where a type would do.

- **Decision:** `recorded` means *this recorder captures it*, which is only the
  three clock reads. **Rationale:** the first revision classified `recv`,
  `bufferLoan`, `supervise`, `parameterRead`, and `lifecycleRestart` as recorded
  on the strength of what a future recorder *could* capture. A reviewer showed
  each admits live state no record carries, so a deterministic component holding
  one would replay against inputs it never had. A classification that describes
  intent rather than mechanism is worse than none: it certifies.
  **Rejected alternative:** adding record families for all five. That is real
  work — a supervision record must carry the whole `supervision_status` answer,
  a loan record the mapped bytes — and claiming it in this milestone would have
  been the same overclaim in a different place. Each becomes `recorded` in the
  change that makes it true.

- **Decision:** `send` is an unrecorded source. **Rationale:** it gates
  `seL4_Call` (`docs/capability-matrix.md`), and `peer_endpoint.rs` mints
  `grant_reply(can_send)`, so a send-only capability really can call and consume
  the reply. The name says output; the authority admits input.

- **Decision:** one declared `streamGrant` is exempt from the determinism join.
  **Rationale:** with every byte-carrying right unrecorded, a replayer cannot
  receive the recording it replays without holding an unrecorded authority — the
  claim becomes unexpressible. The exception is therefore declared, singular, and
  validated three ways: only a replayer may name one, it must be a grant the
  instance is bound, and every other authority is checked unchanged.
  **Rejected alternative:** inferring which endpoint carries the recording. A
  wrong guess exempts a live input.

- **Decision:** the plane's typed outputs derive from the *simulated* clock, and
  the recorded hardware instant is an input rather than an output.
  **Rationale:** C9.5 requires byte-identical typed outputs across two boots. The
  first revision derived an output from two hardware reads and then excluded it
  from the cross-boot comparison — answering a weaker question than the milestone
  asks. The simulated clock moves only when its declared advancer moves it, so
  byte identity is a property of the composition. The hardware read is still
  recorded, to prove the recording carries an input the replay cannot obtain.

- **Decision:** the runtime import gate lives on `CAPABILITY IMPORT`, not on
  export. **Rationale:** the export names a receiver but installs nothing; the
  installation is what would widen the claim.

## Open risks and follow-ups

- [ ] `recv`, `bufferLoan`, `supervise`, `parameterRead`, and `lifecycleRestart`
  are classified `unrecorded`, so no deterministic component may hold them. Each
  becomes `recorded` when a record family carries its answer: received message
  bytes and any capability with them, loan identity/range/contents, the whole
  `supervision_status` answer, parameter key/value observations, and
  `RESTART_ADMIT`'s budget and backoff instant. Until then C9.5's deterministic
  claim covers clock-driven components only, which is what C9.6's sensor →
  controller → actuator graph will need widened.
- [ ] `CLOCK_SIMULATED_ADVANCE` and `SCHEDULING_PROMOTE` are `unrecorded` because
  both answer live state (the pre-advance value; the resulting class and
  priority). A component that only ever *issues* them holds an authority it does
  not read, but the classification is per right rather than per use.
- [ ] The recording travels as endpoint messages rather than a shared buffer.
  One record is exactly `MAX_MSG`, so no framing is needed, but a recording
  larger than a page would want C7's loan path.
- [ ] `just test` still aggregates three gates and names no C9 plane. Adding
  `replay_check` alone would create a second convention; whether the aggregate
  should cover the C9 planes is a question for the track's close.

## Artifacts and provenance

- Focused report: none; the decisions above are the analysis.
- Raw transcript: none retained. The gate reproduces the whole run
  (`just replay_check`), and its two-boot comparison is the artifact.
- Serial/debugger/model output: the gate's own transcripts, reproduced on demand.
- Related roadmap item:
  [C9.5](../../roadmap/02-core-runtime.md#c95--typed-recording-and-deterministic-replay)
