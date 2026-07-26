# B5 — driving the shared-buffer syscalls from real components

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Kind | Defect |
| Status | Verified |
| Scope | `slime_rt` loan wrappers, `sample-lender`/`sample-receiver` components, generation manifest, `bootstrap` wiring, `just sample_plane_live_check` |
| Roadmap | B5, C7.2, C7.4, C7.5, C7.7 |
| Gates | `just sample_plane_live_check` |
| Trigger | Backlog B5, opened by the 2026-07-26 C7 audit (`devlog/2026-07-26-c7-audit/`) |
| Baseline | No test or component reached any `SYS_SHARED_BUFFER_*` syscall; C7.7's "two isolated components" were the `u64` constants `0x71`/`0x72` |

## Summary

C7's gates exercised `SharedBufferTable` methods on locally constructed tables
and never crossed the syscall boundary, so the rights gates, the loan receiver
binding, and reclamation-through-termination were unproven — and a kernel
lifecycle regression (B3) shipped underneath that blind spot. This adds the
live-path counterpart: four missing `slime_rt` loan wrappers, two real
components (`sample-lender`, `sample-receiver`) that the generation grants
capabilities to, and `just sample_plane_live_check`. The pair moves a two-page
payload — larger than the 64-byte message bound — through the actual
`SYS_SHARED_BUFFER_*` syscalls, with only the descriptor crossing the IPC
channel. The transcript is order-sensitive and asserts six denial arms
(factory-as-buffer, unsealed loan, writable map after seal, stale descriptor,
mapping past the loaned region, write access through a loan, double return)
alongside the happy path, so silently permitting a denied operation fails the
gate even when the exchange still completes.

## Observable symptom

Not a defect — a missing evidence path. All nine syscalls were dead code from
the perspective of every test and every component.

- Command: `grep 'dispatch|UserFrame|sys_'` over `kernel/tests/`
- Expected (per C7.2/C7.4/C7.5/C7.7): some caller of the syscall surface those
  slices claim to gate.
- Observed: no matches. `grep SHARED_BUFFER_TABLE` over `kernel/tests/` also
  returned nothing, while `SharedBufferTable::new()` appeared 33 times.
  `kernel/tests/sample_plane.rs:57-58` defined its "components" as
  `LENDER = 0x71` / `RECEIVER = 0x72`, and `:462` stood in for peer death with a
  direct `reclaim_owner` call.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | B4 had already added `create`/`map`/`unmap`/`seal`/`release` wrappers; the four loan syscalls still had none | Added `shared_buffer_loan`/`_loan_map`/`_return`/`_revoke` plus a `BufferLoan` handle type |
| 2 | `task::spawn_from_cap` returns a supervision handle to the spawner, and spawn grants are non-consuming derive-copies | Init can spawn the receiver, then hand its supervision handle to the lender — so the loan names its receiver by capability, never by ambient task id |
| 3 | `check-powerbox.py` establishes the convention: paired components, ordered markers, a Python harness asserting the transcript | Followed it rather than inventing a second shape |
| 4 | Init addresses capabilities by fixed slot index; the sample plane needs four (two executables, two endpoints) | Placed them at 41–44, after the B4 factory at 40, shifting the optional transfer block to 45/46 |
| 5 | First run: `[sample-receiver] fail: loan map`, after `[sample-lender] done` had already printed | Real ordering defect **in my component design**, not the kernel: the lender released and exited before the receiver mapped, and lender termination correctly settles every loan it owns — `reclaim_owner` reclaimed the region out from under the receiver |
| 6 | That failure is exactly the C7.5 retention property stated from the other side | Added a settle handshake: the receiver signals after returning, the lender waits before releasing. The wait is load-bearing, not politeness, and the marker `[sample-lender] receiver settled` now pins the ordering |

## Root cause

Not a code defect — an evidence gap left by the C7 slices. Each sub-slice gate
proved its mechanism against a synthetic table, which is a legitimate unit-level
proof, but no gate ever crossed the syscall boundary or used a real task. The
consequence was concrete rather than theoretical: B3's boot wedge lived in
`task::terminate`'s interaction with the shared-buffer table, a path no C7 gate
touched, so eight passing gates said nothing about it.

The one genuine defect this work surfaced was in my own first draft of the
lender (step 5), and it reproduced a real invariant: a creator that exits while
a loan is outstanding has that loan settled by its own termination. The fix is
the handshake, and the ordering is now asserted by the gate.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `components/runtime/src/syscall.rs`, `lib.rs` | `shared_buffer_loan`, `_loan_map`, `_return`, `_revoke`, and a `BufferLoan { slot, id }` handle | All nine shared-buffer syscalls are reachable from userspace |
| `components/bins/src/bin/sample-lender.rs` (new) | Creates a quota-charged buffer, writes a `>MAX_MSG` payload, seals, loans to a capability-named receiver, sends only the descriptor, waits for settlement, releases | The lender half of the C7.7 exit condition, over real syscalls |
| `components/bins/src/bin/sample-receiver.rs` (new) | Receives descriptor plus transferred loan, validates before mapping, maps only loaned bytes read-only, verifies the payload, returns once, signals settlement | The receiver half, including the single-return identity |
| `contracts/generation/v1/fixtures/valid.zti` | Two component objects, two budget entries, a `bufferCreate` grant, a send/recv channel grant, and a `supervise` grant | Both peers hold only what the generation declares |
| `kernel/src/runtime/bootstrap.rs` | Loads both executables, mints the channel, validates all three new grants, tracks both ids, and accepts the scenario's clean exits in `on_idle` | Boot fails closed if a grant is missing or changed |
| `components/bins/src/bin/init.rs` | `launch_sample_plane()` spawns the receiver first, grants its supervision handle to the lender, and waits on both | Receiver binding is capability-derived, not ambient |
| `scripts/check/check-sample-plane.py`, `Justfile` | New `just sample_plane_live_check` asserting an ordered transcript and rejecting any `fail:` line | B5 owns an independently runnable gate |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A syscall rights gate stops denying | `just sample_plane_live_check` — six denial markers must appear *before* the operations they guard | Missing/out-of-order marker, or a `fail:` line |
| A loan escapes its region, gains write access, or returns twice | Same gate (`loan stays read-only`, `malformed descriptor mapped nothing`, `loan returned once`) | Component exits 1 with a `fail:` reason |
| A creator reclaims pages while a loan is outstanding | `[sample-lender] receiver settled` ordered before `[sample-lender] released` | Receiver's map fails, as it did in step 5 |
| The payload leaks through the kernel message queue | Receiver asserts `recv` returned exactly `DESCRIPTOR_LEN == MAX_MSG` | `fail: descriptor is not exactly one message` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sample_plane_live_check` | pass — full ordered transcript, `sample plane check: ok` | Direct |
| `just sample_plane_check` | pass, 5/5 (in-harness composition unaffected) | Direct |
| `just shared_buffer_accounting_check` | pass, 8/8 | Direct |
| `just shared_buffer_factory_check` | pass, 8/8 | Direct |
| `just shared_buffer_mapping_check` | pass, 8/8 | Direct |
| `just shared_buffer_loan_check` | pass, 7/7 | Direct |
| `just sample_descriptor_check` | pass, 4/4 | Direct |
| `just test` | pass, full kernel suite | Direct |
| `just spawn_service_check`, `just dango_check`, `just powerbox_check` | pass | Direct |
| `just transfer_check` | pass — exercises the renumbered transfer slots 45/46 | Direct |
| `just generation_cmd_check`, `just generation_check`, `just framework_safety_check` | pass | Direct |
| `just fmt_check`, `just lint`, `_components` | clean | Direct |

## Decisions

- Decision: add two dedicated components rather than extending `sample_plane.rs`.
- Rationale: B5 is specifically that in-harness composition was mistaken for
  component evidence. Making the in-harness test spawn tasks would still not
  exercise generation-granted capabilities or the syscall entry path.
- Rejected alternative: teach `kernel/tests/sample_plane.rs` to spawn — it would
  need hand-written assembly stubs (as `isolation.rs` does) to issue syscalls,
  proving the ABI but not the capability plumbing.

- Decision: keep the in-harness `sample_plane_check` alongside the new gate.
- Rationale: they prove different things. The in-harness gate can construct
  quota-exhaustion and unrelated-owner scenarios that are awkward to stage with
  real components; the live gate proves the authority path. Deleting either
  loses coverage.
- Rejected alternative: replace the old gate — would trade one blind spot for
  another.

- Decision: have the lender block on a settle message instead of exiting after
  the send.
- Rationale: forced by the kernel's own semantics (step 5). A lender that exits
  early has its loans settled by termination, which is correct behaviour and
  would make the receiver's map fail. The handshake makes the C7.5 retention
  property an asserted ordering rather than a race.
- Rejected alternative: have the receiver map before the lender exits by luck of
  scheduling — non-deterministic, and it would encode a scheduler assumption.

## Open risks and follow-ups

- [ ] `SYS_SHARED_BUFFER_REVOKE` is the one syscall still without a live caller:
  the lender settles by return, not revocation. The wrapper exists and the
  in-harness gate covers the path, but no component exercises it.
- [ ] The create-insert-failure rollback (`syscall/mod.rs:604-611`) and
  loan-insert-failure revoke (`:820-825`) still have no coverage — both need a
  full capability table at the exact moment of insert, which neither gate
  stages.
- [ ] The sample-plane components use fixed probe addresses
  (`0x9_0000_0000` / `0xA_0000_0000`), the same assumption as the B4 startup
  probes.
- [ ] B6, B7, B8 from the audit remain open and untouched.

## Artifacts and provenance

- Focused report: this entry.
- Raw evidence for the original finding: `devlog/2026-07-26-c7-audit/transcript.txt` §6.
- Serial output: `just sample_plane_live_check` (18-marker ordered transcript).
- Related roadmap items: `roadmap/00-backlog.md` B5 (resolved by this entry);
  `roadmap/02-core-runtime.md` C7.2, C7.4, C7.5, C7.7.
- Related prior entries: `devlog/2026-07-26-c7-audit/` (opened B5);
  `devlog/2026-07-26-b4-live-shared-buffer-budget/` (granted the factory this
  gate consumes); `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/`
  (the regression this coverage gap allowed).
