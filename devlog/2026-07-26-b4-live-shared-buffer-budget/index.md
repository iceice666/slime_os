# B4 — wiring the shared-buffer budget and factory into a real generation

| Field | Value |
|---|---|
| Date | 2026-07-26 |
| Kind | Defect |
| Status | Verified |
| Scope | `scripts/build/build-generation.py` budget emitter, generation manifest fixture, `bootstrap` factory mint, `slime_rt` shared-buffer wrappers, dango/spawn-service startup probe |
| Roadmap | B4, C7.2, C7.3, C7.7 |
| Gates | `just shared_buffer_accounting_check`, `just generation_check`, `just spawn_service_check` |
| Trigger | Backlog B4, opened by the 2026-07-26 C7 audit (`devlog/2026-07-26-c7-audit/`) |
| Baseline | Every built generation contained zero `KIND_RESOURCE` objects; no `SharedBufferFactory` was ever minted; every live holder was `HolderQuota::DENY` |

## Summary

The C7 shared-buffer plane was fully implemented but unreachable on a running
system: no generation declared a `shared-buffer-budget/v1` resource, no manifest
granted `bufferCreate`, and `bootstrap` never minted a `SharedBufferFactory`, so
every component booted deny-by-default and C7.3's exit condition ("two holders
receive distinct generation-declared budgets") held only inside the kernel test
harness. This lands the missing wiring end to end: the host builder emits the
budget as a digest-authenticated `KIND_RESOURCE` object, the manifest declares
per-holder quotas plus two factory grants, `bootstrap` mints the factory at a
fixed capability slot and validates both grants, and `slime_rt` gained the five
shared-buffer syscall wrappers that userspace previously had no way to call. A
bounded startup self-check in `dango` and `spawn-service` runs the full
create → map → write → seal → unmap → release lifecycle, so a normal boot now
prints `[dango] shared-buffer quota live` and `[spawn-service] shared-buffer
quota live` before either component enters its main loop. The C7.3 exit
condition is now observable on the live boot path rather than in-harness only.

## Observable symptom

Not a crash — an absence. The mechanism was complete and its unit gates passed,
but nothing in a running system could allocate a shared buffer.

- Command: `just generation_check`, then parse the built artifact.
- Expected (per C7.3): a generation resource object declaring per-holder quotas.
- Observed before this change: `generation-1.bin` held 21 objects, **zero** of
  kind `KIND_RESOURCE` (4). The single `SLIMESB` byte match in the file was at
  offset 248756, inside the kernel object's range (72347..639962) — kernel
  `.rodata`, not an object payload.
- Corroborating: no `bufferCreate` grant in `contracts/generation/v1/fixtures/valid.zti`;
  `bootstrap.rs` minted `EndpointFactory` and `Input` but never
  `SharedBufferFactory`; `slime_rt` exposed no shared-buffer syscall wrapper.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `build-generation.py` already supported `"resource": 4` in its `KIND` map and the recovery manifest used it, but nothing emitted a budget payload | The object kind existed; only the emitter and manifest stanza were missing |
| 2 | Python bindings for the budget wire format already existed (`scripts/lib/boot_contracts.py`, `SHARED_BUFFER_BUDGET_*`), generated from the Zutai contract | No new schema work; the builder just had to pack the existing layout |
| 3 | `SharedBufferBudget::decode` rejects unsorted or duplicated holder tables, and `holder_identity` is a domain-separated SHA-256 over the component name | The emitter must sort by identity, not by name — sorting is part of the format |
| 4 | The `.zti` fixture dialect rejected `--` comments (the original file contains none); my first manifest edit failed to parse | Moved the rationale into the builder and schema, where comments are supported |
| 5 | `echo-agent` is spawned on demand by `spawn-service`, not part of a normal boot | Chose `dango` + `spawn-service` as holders so both quotas are exercised every boot |
| 6 | `init` addresses its capabilities by fixed slot index, and the transfer block conditionally appends at 40/41 | Placed the factory at slot 40 *before* the transfer block so its slot is fixed on every boot, shifting the transfer slots to 41/42 |
| 7 | Spawn grants are non-consuming derive-copies (`task::spawn_from_cap`) | One factory capability held by init can be granted to both holders without duplication |
| 8 | `slime_rt` had no wrapper for any `SYS_SHARED_BUFFER_*` syscall, so no component could exercise the plane even with a grant | Added the five wrappers; this is also the gap behind backlog B5 |
| 9 | Two QEMU gates reported exit 124 while every test printed `[Passed]` | My own `timeout 400` was too short: the same gate takes ~9 min. Confirmed by re-running at clean HEAD (also 124) and then at 1500 s (exit 0). Not a defect |

## Root cause

Not a defect in implemented code — unfinished wiring. C7.2, C7.3, and C7.4 each
recorded the manifest/factory integration as deferred to C7.7, and C7.7 closed
with "Open risks: None" without doing it. The mechanism layer was correct
throughout; what was missing was the three-part path that makes it reachable:
a budget object in the generation, a factory grant naming its holders, and a
kernel mint that turns that grant into a capability. Each half is inert without
the others, which is why every unit gate passed while the live path stayed dark.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/build/build-generation.py` | `holder_identity()` mirroring the Rust derivation, and `build_shared_buffer_budget()` packing a sorted, duplicate-checked, bounds-validated budget; wired into the payload map for the `shared-buffer-budget` object | The generation carries a real budget resource, authenticated by the existing per-object digest table |
| `contracts/generation/v1/fixtures/valid.zti` | `shared-buffer-budget` resource object; `sharedBufferBudget` holder table (dango 8/2/4/2, spawn-service 4/1/2/1); two `bufferCreate` grants | Quotas and authority are manifest-declared, not kernel-hardcoded |
| `kernel/src/runtime/bootstrap.rs` | Mints one transferable `SharedBufferFactory` at slot 40, ahead of the optional transfer block; `require_grant` validates both factory grants at boot | A component receives creation authority only where the generation grants it |
| `components/runtime/src/syscall.rs`, `lib.rs` | `shared_buffer_create/map/unmap/seal/release` wrappers plus a `SharedBuffer` handle type | Userspace can reach the plane at all |
| `components/bins/src/shared_buffer_probe.rs` (new) | Bounded startup self-check running the full one-page lifecycle, reporting `Ok`/`Denied`/`QuotaExceeded`/`Failed` | A live boot proves the quota, rather than asserting it |
| `components/bins/src/bin/{dango,spawn-service}.rs` | Run the probe at startup; fatal on failure | Both declared holders exercise their budget every boot |
| `components/bins/src/bin/init.rs` | Grants the factory to both holders; transfer slots renumbered 41/42 | Grants flow from the manifest through init to the holders |
| `kernel/tests/shared_buffer_accounting.rs` | `booted_generation_declares_distinct_holder_budgets` — decodes the *booted* generation, asserts two distinct non-deny quotas, an absent component denied, and a real charge to the declared ceiling | The live-path arm of the C7.3 exit condition is now guarded |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A generation stops declaring a budget, or holders lose their distinct quotas | `just shared_buffer_accounting_check` → `booted_generation_declares_distinct_holder_budgets` | `DENY` for a named holder, or identical quotas |
| A factory grant disappears from the manifest | `require_grant` at boot | Kernel panics with `required grant missing or changed` before init spawns |
| The plane regresses to unreachable-from-userspace | Startup probe in both holders; fatal on failure | `[dango] shared-buffer denied` / `quota exhausted` / `lifecycle failed`, component exits 1, boot unhealthy |
| The budget emitter produces a non-deterministic or malformed table | `just generation_check` (two byte-identical builds) and generation decode validation | `cmp` mismatch, or `DecodeError::BadOrder`/`Impossible` failing the boot closed |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just spawn_service_check` | pass — `[generation] shared-buffer factory grants valid`, `[dango] shared-buffer quota live`, `[spawn-service] shared-buffer quota live`, `vertical slice healthy` | Direct |
| `just shared_buffer_accounting_check` | pass, 8/8 incl. the new live-generation case | Direct |
| `just shared_buffer_factory_check` | pass, 8/8 | Direct |
| `just shared_buffer_mapping_check` | pass, 8/8 | Direct |
| `just shared_buffer_loan_check` | pass, 7/7 | Direct |
| `just sample_descriptor_check` | pass, 4/4 | Direct |
| `just sample_plane_check` | pass, 5/5 | Direct |
| `just test` | pass, full kernel suite | Direct |
| `just dango_check` | pass — `dango native runtime check: ok` | Direct |
| `just transfer_check` | pass — install, pending boot, promotion, rollback retention (validates the renumbered transfer slots 41/42) | Direct |
| `just generation_cmd_check` | pass — `vertical slice healthy` | Direct |
| `just generation_check` | pass, two byte-identical builds with the budget object present | Direct |
| `just contracts_check`, `just framework_safety_check` | pass | Direct |
| `just fmt_check`, `just lint`, `_components` | clean | Direct |
| Artifact parse: exactly one `KIND_RESOURCE` object, 128 bytes, digest matches, magic `SLIMESB\0`, 2 holders sorted by identity | budget present and authenticated | Direct |

## Decisions

- Decision: hold one factory capability in init and derive-copy it to both
  holders, rather than minting one per component.
- Rationale: spawn grants are already non-consuming derived copies that can only
  narrow rights, so a single mint expresses "init distributes creation
  authority" without duplicating kernel objects.
- Rejected alternative: mint a factory per holder in `bootstrap` — more kernel
  objects, more fixed slots, and it would put distribution policy in the kernel
  instead of the generation.

- Decision: place the factory at slot 40, before the optional transfer block,
  and renumber the transfer slots to 41/42.
- Rationale: `init` addresses capabilities by fixed index, and the transfer
  block only appends on transfer boots. Appending the factory after it would
  give it a boot-dependent slot.
- Rejected alternative: append last and compute the slot at runtime — the whole
  slot map is a compile-time constant today; introducing one dynamic slot would
  break that pattern for no gain.

- Decision: prove the quota with a startup self-check in each holder, not only
  with a kernel test.
- Rationale: B4 is precisely the finding that in-harness evidence was mistaken
  for live-path evidence. A kernel test asserting the decode would repeat that
  error; running the real syscalls on a real boot does not.
- Rejected alternative: a dedicated `shared-buffer-probe` component — a new
  component and manifest entry to exercise two existing ones, and it would not
  prove that *dango's own* declared quota works.

- Decision: choose `dango` and `spawn-service` as the two holders.
- Rationale: both launch on every normal boot, so both quotas are exercised
  continuously; and they receive different ceilings, which is what makes
  "distinct generation-declared budgets" observable.
- Rejected alternative: `echo-agent` — spawned on demand, so its quota would sit
  unexercised on a normal boot.

## Open risks and follow-ups

- [ ] **B5** remains open and is now partly addressed: `slime_rt` wrappers exist
  and five syscalls (`create`/`map`/`unmap`/`seal`/`release`) are exercised on a
  live boot, but the four loan syscalls still have no wrapper and no test
  reaches them. C7.7's "two isolated components" are still `u64` constants.
- [ ] The startup probe borrows a fixed user address per component
  (`0x4_0000_0000` / `0x5_0000_0000`). Both are free today; a future component
  layout change that maps those addresses would make the probe fail loudly
  rather than silently, but it is an assumption worth knowing about.
- [ ] Quota values (dango 8 pages/2 buffers/4 mappings/2 loans, spawn-service
  4/1/2/1) are deliberately small — enough to prove the mechanism, not sized for
  any real workload. C8 should revisit them when the sample plane carries actual
  traffic.
- [ ] B6, B7, B8 from the audit remain open and untouched by this change.

## Artifacts and provenance

- Focused report: this entry.
- Raw evidence for the original finding: `devlog/2026-07-26-c7-audit/transcript.txt` §5.
- Serial output: `just spawn_service_check` (quota-live markers), `just
  shared_buffer_accounting_check` (8 `[Passed]`).
- Related roadmap items: `roadmap/00-backlog.md` B4 (resolved by this entry);
  `roadmap/02-core-runtime.md` C7.2, C7.3, C7.7.
- Related prior entries: `devlog/2026-07-26-c7-audit/` (opened B4);
  `devlog/2026-07-26-b3-shared-buffer-table-stack-overflow/` (B3, the other
  blocker on the C7 gate).
