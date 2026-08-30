# B90: the `Block` capability kind is deleted, and two of its premises were wrong

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation/v5/{vocab/rights.zt,gen_rust.zt}`, `boot-contracts/src/{generation.rs,generated/generation.rs}`, `slime-root/src/{graph,ipc,generation,graph_runtime}.rs` and `graph_runtime/{console_runtime.rs,services/spawn.rs}`, `scripts/build/{build-generation.py,boot_layout.py}`, `scripts/check/{check-generation,check-system-spec,check-component-spec}.py`, `scripts/lib/component_spec.py`, `contracts/system-spec/v1/systems/reference.zti`, `contracts/generation-manifest/v1/fixtures/valid.zti`, five `contracts/component-spec/v1/components/*.zti`, `contracts/store/v1/README.md`, `docs/{capability-matrix,syscall-abi}.md`, `README.md`, `AGENTS.md` |
| Roadmap | B90, B83 |
| Gates | `just contracts_check`, `just system_spec_check`, `just component_spec_check`, `just architecture_contract_check`, `just test_sel4_root`, `just test_host`, `just framework_safety_check`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` |
| Trigger | Asked to research B90's two proposed cutovers before choosing one; the research disproved the blocker on the option the entry called blocked |
| Baseline | B83 (resolved 2026-08-29) deleted `BLOCK TRANSACT` and left the `Block` kind, its two rights bits, `SERVICE_BLOCK`, and two launch-order ordinal counters compiled into every product image with no operation able to resolve them |

## Summary

B90 offered two clean cutovers and declared the first one — deleting the kind —
blocked by roadmap invariant 7's bounded rollback window. It is not blocked:
invariant 7 says the opposite of what the entry assumed, and `Generation::decode`
implements it by refusing every superseded format at the header before reading a
single capability record. A second premise was also wrong: the entry claimed
`capability_rights_valid` *requires* both block rights, which would let the
vestige refuse a generation; the predicate is `rights & required != 0`, so either
bit alone passes and always did. With both premises corrected, Option 1 was the
cheaper cutover as well as the honest one, and Option 2 turned out to require
inventing a contract concept — a decode-only marker on a kind or right, for which
no precedent exists — in exchange for zero observable change on any built image.

The kind, both rights bits, `SERVICE_BLOCK`, `BlockRights`, `BlockCapability`,
`CapabilityEntry::Block`, `resolve_block`, and both duplicated per-device ordinal
counters are gone. `RIGHT_ALL` fell by exactly 3072. The real coupling was never
the rollback window: it was the frozen CP1 baseline chain, which B83's own
deferred follow-ups had already identified as belonging to "the CP1 fixture's own
migration". That migration is what this entry performs.

## Observable symptom

- Command: `grep -rn "resolve_block" slime-root/src`
- Expected: at least one caller on a dispatch path, since the kind is admitted,
  materialized, and rights-checked at boot.
- Observed: three references, all assertions inside `graph.rs`'s own test module.
  `service_for_root_label` maps labels to ten service ids and none to
  `SERVICE_BLOCK`, so no request could reach a `Block` capability even in
  principle.
- Exit/fault/serial evidence: none — this is dead authority, not a fault. The
  admission half was live: `service_for_capability` derived `SERVICE_BLOCK`, so a
  generation granting a `Block` capability to an instance that did not declare
  service `8` failed decode with `BadBinding`. A vestige that can refuse a
  generation while granting nothing is the shape Grammar rule 2 forbids.

## Investigation log

| Step | Observation | Consequence |
|---|---|---|
| 1 | `roadmap/README.md:154` reads "A generation in a superseded wire format counts as failed: the root refuses it (`UnsupportedVersion`, distinct from `BadMagic`) rather than migrating it… Format bumps are therefore *not* rollback-compatible by migration — they are rollback-*safe* by refusal" | Invariant 7 forbids the migration B90 believed it required. The blocker is a misreading |
| 2 | `boot-contracts/src/generation.rs:752-763` compares magic and version before any table offset is read; v2/v3/v4 return `UnsupportedVersion` immediately | An old generation's capability-kind table is never decoded, so retiring a kind from v5 cannot affect it |
| 3 | `roadmap/07-architecture-portability.md:44-48` says "bounded decoding of existing x86 artifacts preserved for the rollback window"; its evidence devlog scopes that to kernel/component *image* revisions | The terminology collision on "rollback artifacts" is the origin of the misreading. Executable-format compatibility is a separate, still-live surface |
| 4 | `slime-root/fixtures/generation.bin` is `SLIMEG3`/version 3 | The one tracked x86 generation blob is already refused by the current decoder; it survives only as an inert `sel4_root_boot_check` classification fixture |
| 5 | `build-generation.py:2816` fails unless the target profile is `aarch64-sel4-qemu-virt` or `aarch64-rpi5` | No build path can emit a generation carrying a `Block` grant, so the kind is unreachable from every image, not merely unresolved |
| 6 | `capability_rights_valid`'s final predicate is `rights != 0 && rights & !allowed == 0 && rights & required != 0` | B90's "requires `blockWrite` on every `Block` grant" is wrong. `valid.zti`'s `blockRead`-only `block-read` grant passed then and passes any equivalent test now |
| 7 | `contracts/block-authority/v1/schema.zt:32-33` declares `rightRead = 1`, `rightWrite = 2` in a `u16` field; the builder maps the reused manifest spellings to those constants at `generation_resources.py:353` | The two systems share only human-readable names. Deleting generation bits 10/11 cannot affect per-ring storage authority |
| 8 | The determinism join reads capability `grants` and `mintedBindings` only (`generation_resources.py:1324-1351`), never `blockRingAuthority`; `check-sel4-replay-plane.py:286-308` asserts the per-ring form explicitly and says checking the old grant list "would make the assertion vacuous after the cutover" | C9.5's unrecorded-source classification does not depend on bits 10/11. Retiring them cannot weaken the replay refusal |
| 9 | `RightBit` has exactly `{ name, bit, manifest, determinism }`; searches for `frozen`/`legacy`/`deprecated`/`decodeOnly` across `contracts/` find only prose and format-specific constants | Option 2's decode-only marker would be a new contract concept. A nonempty `manifest` spelling simultaneously drives Python admission, `right_named`, `RIGHT_ALL` membership, and determinism classification, so retaining today's rows cannot express "decodable but not grantable" |
| 10 | `valid.zti` is generated from `reference.zti` and compared against a never-regenerated baseline whose own rule is "never regenerated and never edited" (`check-system-spec.py:46-50`) | The actual cost of Option 1. Re-blessing the baseline would destroy what makes it evidence, so the removal needs a named exemption alongside `KNOWN_DEAD_BINDINGS` |
| 11 | `just/quality.just:491` asserts `expected=211` across 19 modules | `AGENTS.md:83`'s "118 host unit tests across 14 modules" is stale documentation. Tests were rewritten rather than deleted, so the assertion holds unchanged |

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/generation/v5/vocab/rights.zt` | `BLOCK_READ`/`BLOCK_WRITE` deleted; bits 10 and 11 documented as reserved beside the existing gap at 17 | Every declared right names a gated operation (Grammar rule 2) |
| `contracts/generation/v5/gen_rust.zt` | `CAPABILITY_BLOCK` and `SERVICE_BLOCK` removed from both the Python and Rust emitters, each with a reserved-number note | One vocabulary, no kind admitted by a mask no operation reads |
| `boot-contracts/src/generated/generation.rs`, `scripts/lib/boot_contracts.py` | Regenerated; `RIGHT_ALL` `17179738111` → `17179735039`, a fall of exactly 3072 | Generated files are outputs, changed only by regenerating their contract |
| `boot-contracts/src/generation.rs` | `CapabilityKind::Block`, its decode arm, both rights-mask arms, the `service_for_capability` arm, and `SERVICE_BLOCK` in the known-service set deleted | Admission cannot require a service no label selects |
| `boot-contracts/src/generation.rs` tests | The gap test now loops bits 10, 11, 17 over all twelve kinds; the partition test's `BASELINES` completed to twelve kinds and IO1's four bits moved to `MANIFEST_DECLARABLE` | The two rights classes partition the vocabulary, not a nine-kind subset |
| `slime-root/src/graph.rs` | `BlockRights`, `BlockCapability`, `CapabilityEntry::Block`, its constructor, its four generic arms, and `resolve_block` deleted | No typed root entry exists that no operation can resolve |
| `slime-root/src/graph_runtime.rs`, `graph_runtime/services/spawn.rs` | Both bespoke `block_index` counters deleted; every remaining non-executable kind is constructed with resource `0` | The one launch-order ordinal whose value nothing read is gone; IO device identity stays in the IO-resource table |
| `slime-root/src/graph_runtime/console_runtime.rs`, `ipc.rs` | The `Block` construction arm and the `"block"` manifest spelling deleted | A kind the manifest cannot declare is not askable |
| `scripts/build/build-generation.py`, `scripts/check/check-generation.py`, `scripts/lib/component_spec.py` | `block` removed from the kind table, both rights tables, and the service map in all three independent copies; `_DEVICE_KINDS` now derives from `SERVICE_INPUT`/`SERVICE_IO_RESOURCE` | The three Python restatements of the kind vocabulary agree with the contract |
| `contracts/system-spec/v1/systems/reference.zti` | Three `block` grants and their three slot pins deleted | The CP1 source system declares no grant in a retired kind |
| `contracts/generation-manifest/v1/fixtures/valid.zti` | Regenerated: 48 grants → 45 | The generated fixture is reproducible from its source spec |
| Five `contracts/component-spec/v1/components/*.zti` | `block` removed from the three frozen storage probes' `requires`/`devices`; `init` no longer provides it; `virtio-blk-driver` now declares the `device` authority it actually holds rather than `provides = ["block"]` | Each record states authority the generation can express |
| `scripts/check/check-system-spec.py` | `RETIRED_CAPABILITY_KINDS` plus `strip_retired_kinds`/`check_retired_kinds`, asserting corpus-level non-vacuity and that the kind is genuinely unspellable | The frozen baseline stays frozen; its divergence is explained rather than blessed away |
| `scripts/check/check-component-spec.py` | The corpus-coverage arm moved from `storage-probe` to `directory-probe`, with a guard that the chosen spec requires something | A rule about unmet requirements is exercised by a spec that has requirements |
| `scripts/build/boot_layout.py` | Docstring records why retained x86 `storage-capability` rows outlive the kind | A role is a CSpace position, not a capability kind |
| `docs/capability-matrix.md`, `docs/syscall-abi.md`, `README.md`, `AGENTS.md` | Two "ungated in the root" rows deleted; kinds table now twelve; residue and partition paragraphs rewritten; README's rollback wording corrected to executables; test count corrected 118/14 → 211/19 | Documentation moves in the same change as the surface it describes |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The kind becomes spellable again while the baseline comparison still skips its grants | `just system_spec_check` | `check_retired_kinds` fails naming the kind still in `BUILDER.CAPABILITY_KIND` |
| Any listed retired kind outlives its subject and becomes dead text | `just system_spec_check` | The hoisted per-kind guard fails: "no frozen baseline declares a grant in [...] so those RETIRED_CAPABILITY_KINDS entries cover nothing and must be removed" |
| The exemption widens to cover a live-kind grant | `just system_spec_check` | The liveness guard names the offending kind, and the per-system `unexplained`/`live` sets refuse a stripped binding that names a live grant |
| A negative-control mutation degrades into a membership error, silently un-testing the rule it names | `just component_spec_check`, `just system_spec_check` | Each arm names live vocabulary, so a retirement that removes it fails as an unknown kind/right in the mutation rather than passing quietly |
| A reserved bit or discriminant is silently reassigned | `just test_sel4_root`, `just test_host` | `right_all_is_a_union_of_named_bits_and_excludes_the_reserved_gaps` fails on the offending bit; `right_named("blockRead")` stops being `None` |
| The partition test narrows again, hiding a right declarable on an unenumerated kind | `just test_host` | `declared_rights_partition_into_manifest_declarable_and_root_only` fails its length or union assertion |
| Storage-write authority returns to a capability grant | `just framework_safety_check` | "grants blockWrite as a capability" — now doubly enforced, since the kind cannot be spelled |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just test_sel4_root` | Pass — 211/211 across 19 modules, count assertion unchanged because tests were rewritten rather than deleted | Direct |
| `just test_host` | Pass — 310 `boot-contracts` tests | Direct |
| `just contracts_check` | Pass — all 30 contract families | Direct |
| `just system_spec_check` | Pass — 2 systems derived semantically identical to their baselines, 20 named mutations refused | Direct |
| `just component_spec_check` | Pass — 57 records, 43 named mutations refused, identities stable | Direct |
| `just architecture_contract_check` | Pass — target and executable-artifact contracts | Direct |
| `check-generation-v5.py` | Pass — all 40 seL4 manifests encode `SLIMEG5` version 5 | Direct |
| `check-boot-layout-resource.py` | Pass unchanged — 19 fixtures, 16 seL4 fixtures, 31 planes, 129 rows | Direct |
| `check-framework-authority.py` | Pass — 8 fixtures grant `blockWrite` only to approved ring owners | Direct |
| `check-fabric-manifest.py`, `check-data-fabric-profile.py` | Pass | Direct |
| `RIGHT_ALL` arithmetic | `17179738111 - 17179735039 = 3072 = (1<<10) + (1<<11)` | Direct |

Two verification failures were observed and fixed rather than worked around, and
both are recorded because each was a real defect in this change:

1. A mis-scoped `CUT` deleted `"sharedBufferFactory": RIGHT["bufferCreate"]` from
   `validate_capability_rights`'s `required` table. It surfaced as a `KeyError`
   on the first regeneration. Confirmed repaired by auditing the file's whole
   deletion set — exactly five lines, all naming `block`.
2. The first `boot-contracts/src/generation.rs` edit used replace where deletion
   was needed and duplicated six lines. The file was reverted and redone with
   `CUT`; the parse warning that caught it is why nothing downstream saw it.

Four fault injections, each reverted after being observed, because a guard that
has never failed is a guard nobody has tested:

| Injection | Observed | Guard proved |
|---|---|---|
| `"block": 4` restored to the builder's `CAPABILITY_KIND` | `system spec check: 'block' is listed as a retired capability kind but the builder still admits it` | The kind-resurrection guard fires with its own message, and the tree returns to green on revert |
| A new right `SCRATCH` assigned to reserved bit 10, bindings regenerated | `cargo test -p boot-contracts` exits 101 in `right_all_is_a_union_of_named_bits_and_excludes_the_reserved_gaps` | The reserved positions are pinned, not merely documented. `RIGHT_ALL` returned to `17179735039` and 310/310 passed on revert |
| The corpus-coverage arm pointed back at `storage-probe` | `component spec check: the corpus-coverage arm needs a spec that requires something` | The new guard catches exactly the vacuity this change would otherwise have introduced silently |
| `RETIRED_CAPABILITY_KINDS = {"block", "neverExisted"}` | `no frozen baseline declares a grant in ['neverExisted']` | The non-vacuity guard is per-kind, so a second entry cannot ride along on the first entry's coverage |
| `RETIRED_CAPABILITY_KINDS = {"neverExisted"}` (set mistyped) | Same intended message, *not* the baseline-divergence one | The guard is hoisted above every baseline comparison, so an engineer who mistypes the set is not pointed at the frozen fixture and tempted to re-bless it |
| `RETIRED_CAPABILITY_KINDS = {"block", "input"}` (a live kind listed) | `'input' is listed as a retired capability kind but the builder still admits it` | The liveness guard covers every listed entry, not just the first |

### Reviewer panel

A five-lens read-only panel reviewed the diff: a canonical pass, plus
correctness, security, convention, and gate-integrity lenses. Verdicts were three
*correct* and two *incorrect*, with **no P0 or P1 and no security defect**. Two
reviewers independently reproduced the two load-bearing arguments in this entry —
that `declared_capability(kind, 0, rights)` is behavior-preserving because the IO
resource fields have no readers, and that `RIGHT_ALL`/`RIGHT_UNRECORDED` each fall
by exactly 3072 and remain unions of the surviving named bits.

Eleven findings were applied, all P2/P3. Every one of them was verified by
mutation or direct probe before being acted on; none were taken on assertion.
They fall into three groups:

1. **Three more vacuous negative controls**, the same class this change had
   already fixed once for the corpus-coverage arm and then failed to look for
   elsewhere. `unsorted_capability_kinds` and `undeclared_device` in
   `check-component-spec.py`, and `rights_outside_kind` in
   `check-system-spec.py`, each mutated a spec using the now-unspellable `block`
   or `blockWrite`, so each was refused by a *membership* check before reaching
   the rule it named. All three now name live vocabulary, and each was confirmed
   by instrumenting the gate to print its refusal reason: `provides: must be
   sorted`, `runtime.devices: 'input' appears in neither provides nor requires`,
   and `rights do not match capability kind endpoint`.
2. **Four false statements this change introduced.** `docs/syscall-abi.md` kept
   `8` block in a service-id list it says is generated, three lines above the new
   paragraph saying the id was deleted — the same intra-file contradiction the
   predecessor audit entry was opened to fix. `contracts/store/v1/README.md`
   still described storage authority as a `Block` capability with bits 10/11.
   `component_spec.py` justified its `sel4-storage-probe` exemption, in two
   places, by a `requires = ["block"]` that no longer exists. And this entry's
   own Regression guards table named `just framework_authority_check`, which is
   not a Justfile target (`framework_safety_check` is).
3. **Two defects in the exemption this change added**, both in code written for
   B90 and both found only because the gate-integrity lens went looking. The
   non-vacuity guard was per-*set*, so a second bogus entry rode along on
   `block`'s coverage unnoticed (proved: `{"block","neverExisted"}` exited 0).
   And `check_retired_kinds`'s second assertion was a tautology — it re-tested
   the predicate `strip_retired_kinds` had already filtered on, so its `fail`
   branch was unreachable by construction while its docstring advertised it as
   load-bearing. A first attempt to fix it by reading the untouched fixture was
   *also* unreachable, as round 2 caught: `declared` is built from the same grant
   list the stripped entries came out of, so it fires only on a duplicated grant
   name, which `system_spec.py` refuses independently. Exhaustive enumeration
   over all kind assignments of a small fixture confirmed 0 firings with unique
   names. The assertion is now deleted rather than rewritten a third time, and
   the docstring cites the two checks that *can* fail — the caller's
   `unexplained`/`live` comparison against the unmodified fixture, and the
   hoisted per-kind coverage guard.

## Decisions

- **Decision:** delete the wire discriminant and both rights bits from v5 rather
  than keep a decode-only half.
  **Rationale:** the rollback window does not read them. Invariant 7 refuses
  superseded formats at the header, and the only tracked x86 generation blob is
  `SLIMEG3`, already refused. Keeping a decode half would preserve machinery for
  a decoder path that cannot execute.
  **Rejected alternative:** B90's Option 2. It needs a new contract concept for
  which no precedent exists — `RightBit` has no status field, and capability
  kinds are renderer literals duplicated in three Python tables — and buys zero
  observable difference, because the builder already refuses every target profile
  whose fixtures could declare the kind.

- **Decision:** reserve bits 10/11, kind 4, and service 8 rather than reassign
  them.
  **Rationale:** a bit position and a discriminant are ABI. `rights.zt` already
  says so for bit 17 (B57), and the same reasoning applies to a retired right
  with more force: a component compiled before the retirement names the same bit.
  **Rejected alternative:** compacting the numbering. It would make one
  generation's `blockRead` another's `storeRead`.

- **Decision:** excuse the frozen baseline's three grants with a named
  `RETIRED_CAPABILITY_KINDS` exemption that asserts its own non-vacuity.
  **Rationale:** the baseline is evidence precisely because it is never
  regenerated (`check-system-spec.py:46-50`), so re-blessing it would destroy the
  property the gate exists to hold. This repository already has the shape for
  this: `KNOWN_DEAD_BINDINGS` and `POST_BASELINE_SECTIONS` are two named,
  separately-asserted exemptions rather than one blanket ignore. The two
  assertions matter independently — one keeps the exemption from covering a grant
  it was not opened for, the other keeps a retired kind from quietly becoming
  spellable while the comparison still skips it.
  **Rejected alternative:** re-blessing `baselines/valid.zti`, or a blanket
  "ignore grants the derivation omits", which would hide any future divergence.

- **Decision:** keep the three frozen storage component records, with empty
  `requires`/`devices`, rather than deleting the components.
  **Rationale:** deleting them would churn four frozen `.layout` fixtures and the
  `BASE_LAYOUT`/`OVERRIDE_2`/`OVERRIDE_3` tables for a retired custom kernel,
  which is a separate concern from a capability kind: boot-layout rows name
  *roles*, and `storage-capability` is a role the retired kernel resolved. The
  component-spec gate already reports all three as
  declared-without-implementation.
  **Rejected alternative:** removing the components in this change. It widens the
  diff into B10 layout evidence without closing anything B90 names.

- **Decision:** move the corpus-coverage negative control to `directory-probe`
  instead of relaxing it.
  **Rationale:** the arm proves that a corpus requiring a kind nothing provides
  is refused. With `block` retired, `storage-probe` requires nothing, so the arm
  became vacuous — it passed only because the mutation could no longer express
  the condition. `directory-probe` requires `directory`, which `init` alone
  provides, which also keeps the paired converse arm meaningful. A guard now
  fails the gate if the chosen spec ever stops requiring something.
  **Rejected alternative:** deleting the arm, which would drop coverage of a real
  corpus rule.

- **Decision:** correct `virtio-blk-driver`'s record to `provides = []` and
  `devices = ["device"]`.
  **Rationale:** it declared `provides = ["block"]` in a kind it never held as a
  capability. Its real authority is the four IO kinds it already requires and the
  per-ring rights the generation declares; the block service it offers peers is
  an IO0 ring, not a capability kind.
  **Rejected alternative:** leaving `devices` empty, which would understate that
  it drives hardware.

- **Decision:** delete `check_retired_kinds`'s second assertion rather than
  rewrite it a third time.
  **Rationale:** it was written to prove "the exemption covers only grants it was
  opened for," but every formulation available to it reads a field the filter
  itself selected on, so it cannot fail on any input the call site can produce.
  Two attempts confirmed that: the original re-tested the filter's own predicate,
  and reading the untouched fixture instead fires only on a duplicated grant
  name, which `system_spec.py` already refuses. The property is real and is
  genuinely checked by the caller's `unexplained`/`live` sets, which compare
  against the unmodified fixture. Keeping a dead assertion that documents itself
  as load-bearing is worse than not having one, because the next reader trusts
  it.
  **Rejected alternative:** a third rewrite against a synthesized independent
  copy of the grant table, which would test the synthesizer rather than the gate.

## Open risks and follow-ups

- [ ] Bits 12–15 (`storeRead`, `storeWrite`, `healthConfirm`, `bootUpdate`)
      remain named-but-ungated: inside `RIGHT_ALL`, admitted by no kind mask,
      checked by no operation. They are residue of the retired kernel's
      `ObjectStore`/`GenerationControl` kinds. This change deliberately did not
      touch them — they are a different retirement with different fixtures — but
      they are now the only four left, and `docs/capability-matrix.md` records
      them as such.
- [ ] `scripts/check/check-generation.py`'s kind and service tables are a third
      independent hand-written copy of a vocabulary the contract owns, and it
      still knows nothing of IO1's four kinds or `SERVICE_CLOCK`/
      `SERVICE_IO_RESOURCE`. CP0's devlog already flagged the builder's copy as
      B59-class follow-on work; this entry adds that the oracle has the same
      problem and a wider gap.
- [ ] The three frozen storage components and their four `.layout` fixtures
      remain, per the decision above. B83's deferred item for them is now
      narrower — the grants are gone, the identities are not — but not closed.
- [ ] No seL4 QEMU plane was run for this change. Every deleted symbol was
      unreachable from every composition (step 5), and the host, contract, and
      spec gates cover the admission and vocabulary surfaces that changed; but
      "no plane regressed" is inherited from that reachability argument rather
      than observed.
- [ ] Reserved positions are documented and pinned *positively* — the gap test
      asserts bits 10/11/17 are outside `RIGHT_ALL` and rejected by every kind —
      but nothing pins them *negatively* at the wire layer: no test feeds a
      generation carrying capability discriminant 4 and asserts the exact refusal
      (`DecodeError::BadBounds` from `CapabilityKind::decode`). The security lens
      raised this as informational; it is a gap in evidence, not in behavior.
- [ ] `contracts/store/v1/README.md` and `docs/capability-matrix.md` are prose
      checked only by review. This change falsified an authority claim in the
      first and was caught by a reviewer, not a gate. The predecessor entry
      already carries the open item for a check that compares documented gate
      status against the rights some operation actually checks; this change is a
      second instance of the same class and strengthens the case for it.
- [ ] `contracts/store/v1/README.md` still cites `components/bins/...` paths that
      CP3 moved. Pre-existing and out of scope here, but noted while correcting
      the authority paragraph above it.

## Artifacts and provenance

- Focused report: none; the investigation log above is the record, and every
  claim in it names a file and line the reader can re-check.
- Raw transcript: none frozen. Every result under Verification is a `just` target
  or a `scripts/check/` script the reader can re-run.
- Serial/debugger/model output: none — no plane was booted, as recorded above.
- Related roadmap item: [`roadmap/00-backlog.md`](../../roadmap/00-backlog.md)
  (B90 resolved; B83's fixture follow-up narrowed).
  Predecessors: [`devlog/2026-08-29-b83-root-block-path-deleted/`](../2026-08-29-b83-root-block-path-deleted/index.md),
  [`devlog/2026-08-30-io-reference-doc-drift/`](../2026-08-30-io-reference-doc-drift/index.md)
  (the audit that opened B90),
  [`devlog/2026-08-17-b64-format-coexistence/`](../2026-08-17-b64-format-coexistence/index.md)
  (invariant 7's refusal-not-migration rule).
