# Repository hygiene ownership and contract-check consolidation

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Change |
| Status | Verified |
| Scope | PR templates, documentation ownership, verification checker ownership, generation-manifest slot-pin contract checks |
| Roadmap | none |
| Gates | `just contracts_check`, `just devlog_check` |
| Trigger | Repository hygiene plan for PR descriptions, implementation comments, and top-level verification scripts |
| Baseline | PRs and comments had no concise ownership rule, and the B91 slot-pin invariant remained a backlog-shaped top-level checker |

## Summary

Repository guidance now assigns local invariants to code comments, review surface to PR descriptions, and investigation and evidence to devlogs. Verification guidance requires new invariants to extend their owning mechanism. The slot-pin reason checks now live as a generation-manifest contract case while `just contracts_check` remains the public interface.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `.github/PULL_REQUEST_TEMPLATE/` | Added short ordinary-change and system-change templates | PR descriptions describe claims, changes, risk, review surface, and verification without becoming audit reports |
| `AGENTS.md` | Added documentation ownership, checker ownership, and the intended shared seL4-plane direction | Repository artifacts reflect durable concepts rather than development chronology |
| Component SDK audit | Compared six checkers totaling 3,692 lines and their top-level functions | The SDK family remains separate because only small subprocess, clone, and publication helpers repeat; the case implementations and evidence semantics are distinct |
| Small policy checker audit | Reviewed architecture portability, lifecycle identity, and Framework storage authority | No merge: each has a distinct source corpus and stable policy boundary; lifecycle identity remains a contract aggregate case, while portability and Framework authority retain independently reusable quality gates |
| Slot-pin checks | Moved the implementation under `scripts/check/contracts/slot_pin_reasons.py` and invoked it from `contracts_check` | Generation-manifest slot reasons are owned by contract verification, not by the backlog item that introduced them |
| Checker comments | Replaced B91 and migration-history framing with current invariants and automatic-slot expectations | Checker documentation states what must remain true; investigation history remains in the B91 devlogs |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Slot reasons stop matching manifest and boot-profile semantics | `just contracts_check` | The moved case rejects the manifest with the holder and grant named |
| A name-resolved binding is unnecessarily re-pinned or its automatic slot drifts | `just contracts_check` | The automatic-slot expectation names the changed binding and expected slot |
| Devlog structure or links drift | `just devlog_check` | The checker reports the malformed entry or missing index row |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre-refactor `just contracts_check` | Passed, including slot-pin reasons and 310 `boot-contracts` tests | Direct |
| Pre-refactor `just x86_portability_check` | Passed over 223 neutral Rust files | Direct |
| Pre-refactor `just architecture_contract_check` | Passed, including the architecture contract and 310 `boot-contracts` tests | Direct |
| `python3 scripts/check/contracts/slot_pin_reasons.py` | Passed: 611 pinned bindings, 68 automatic bindings, and five automatic-slot expectations | Direct |
| Embedded-case control: re-pin `network-service/network-intruder-service` in `sel4-io-network.zti` | `python3 scripts/check/check-contracts.py` exited 1 during slot-reason validation; the fixture was restored with no diff | Direct |
| Post-refactor `just contracts_check` | Passed | Direct |
| `just devlog_check` | Passed | Direct |
| `just ruff` | Passed | Direct |
| `just typos` | Passed | Direct |
| Post-refactor `python3 scripts/check/check-contracts.py` | Passed; the slot-pin case ran inside the owning contract checker before 310 `boot-contracts` tests passed | Direct |

## Decisions

- Decision: do not consolidate the component SDK scripts in this change.
- Rationale: measurement found 3,692 lines with only seven structurally duplicated top-level function shapes; shared code is limited to small process and repository helpers, while each checker owns a different release-lifecycle claim and evidence path. A dispatcher would combine filenames without extracting a common verification mechanism.
- Rejected alternative: a single `check-component-sdk.py` containing all six implementations. It would be a larger file with the same case boundaries and no meaningful reduction in mechanism duplication.
- Decision: retain the three small policy checkers in their current ownership boundaries.
- Rationale: architecture portability scans neutral Rust mechanism, lifecycle identity scans public boundary declarations, and Framework safety freezes a product storage-authority allowlist. Their inputs, failure semantics, and reuse differ.
- Decision: move the slot-pin implementation into the contract checker rather than retaining a separately invoked executable.
- Rationale: slot reasons describe generation-manifest semantics and already run only through `just contracts_check`; making the owning checker invoke the case removes the backlog-shaped executable boundary without changing the public gate.

## Open risks and follow-ups

- [ ] Shared QEMU invocation and transcript machinery remains distributed across plane scripts. Future changes should extract only repeated concrete mechanisms into the documented `scripts/check/sel4/` shape while retaining public `just` targets.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: none; this change moved a host-side contract case and did not alter QEMU behavior.
- Related roadmap item: none.
