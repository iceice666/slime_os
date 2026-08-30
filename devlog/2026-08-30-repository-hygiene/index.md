# Repository hygiene ownership and contract-check consolidation

| Field | Value |
|---|---|
| Date | 2026-08-30 |
| Kind | Change |
| Status | Verified |
| Scope | PR templates, documentation ownership, verification checker ownership, generation-manifest slot-pin contract checks, Just orchestration, seL4 plane execution runtime |
| Roadmap | none |
| Gates | `just contracts_check`, `just sel4_gate_control_check`, `just io_network_check`, `just io_queue_check`, `just io_block_check`, `just io_driver_authority_check`, `just ruff`, `just devlog_check`, `just fmt_check_all` |
| Trigger | Repository hygiene plans for artifact ownership, Just orchestration, and seL4 gate runtime consolidation |
| Baseline | PRs and comments had no concise ownership rule, the B91 slot-pin invariant remained a backlog-shaped top-level checker, Justfiles duplicated roadmap, checker, and investigation prose, and the inspected I/O plane gates each carried their own image identity and QEMU lifecycle implementation |

## Summary

Repository guidance now assigns local invariants to code comments, review surface to PR descriptions, and investigation and evidence to devlogs. Verification guidance requires new invariants to extend their owning mechanism. The slot-pin reason checks now live as a generation-manifest contract case while `just contracts_check` remains the public interface. Justfiles now retain command orchestration and short execution invariants instead of duplicating system semantics or development history. Four materially different I/O planes now share one image-identity and pinned-QEMU runtime while retaining their fixture, marker, device, and post-boot claims in each gate.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `.github/PULL_REQUEST_TEMPLATE/` | Added short ordinary-change and system-change templates | PR descriptions describe claims, changes, risk, review surface, and verification without becoming audit reports |
| `AGENTS.md` | Added documentation ownership, checker ownership, and the intended shared seL4-plane direction | Repository artifacts reflect durable concepts rather than development chronology |
| Component SDK audit | Compared six checkers totaling 3,692 lines and their top-level functions | The SDK family remains separate because only small subprocess, clone, and publication helpers repeat; the case implementations and evidence semantics are distinct |
| Small policy checker audit | Reviewed architecture portability, lifecycle identity, and Framework storage authority | No merge: each has a distinct source corpus and stable policy boundary; lifecycle identity remains a contract aggregate case, while portability and Framework authority retain independently reusable quality gates |
| Slot-pin checks | Moved the implementation under `scripts/check/contracts/slot_pin_reasons.py` and invoked it from `contracts_check` | Generation-manifest slot reasons are owned by contract verification, not by the backlog item that introduced them |
| Checker comments | Replaced B91 and migration-history framing with current invariants and automatic-slot expectations | Checker documentation states what must remain true; investigation history remains in the B91 devlogs |
| Just orchestration | Removed narrative comments across `just/`, converted compatibility recipes into a concise alias registry, and parameterized repeated Zutai model-check commands | Justfiles describe commands and dependency order while contracts, checkers, and devlogs retain semantic ownership |
| seL4 plane runtime | Inspected five I/O gates; four pilot gates now use `scripts/lib/sel4_plane.py` for identity verification, the pinned QEMU command, timeout, transcript capture, terminal stopping, and terminate/wait/kill cleanup | One execution mechanism serves kernel-only, disposable-disk, and fixed-device boots without absorbing plane assertions |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Slot reasons stop matching manifest and boot-profile semantics | `just contracts_check` | The moved case rejects the manifest with the holder and grant named |
| A name-resolved binding is unnecessarily re-pinned or its automatic slot drifts | `just contracts_check` | The automatic-slot expectation names the changed binding and expected slot |
| Devlog structure or links drift | `just devlog_check` | The checker reports the malformed entry or missing index row |
| Public recipe or dependency behavior drifts during comment cleanup | `just --list`, `just --summary`, and Zutai dry runs | Parsing fails, the 187-recipe inventory changes, or a model path/command differs |
| Shared runtime weakens a pilot gate | Public I/O recipes, identity and timeout controls, plus `just sel4_gate_control_check` | Wrong variants, timeouts, failure markers, and missing/out-of-order markers still reject the gate |

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
| `just --list` and `just --fmt --check` | Passed across all imported Justfiles | Direct |
| Public recipe comparison before and after Just cleanup | Identical 187 recipe names in identical order | Direct |
| `just --dry-run bootstate_model_check capability_rights_model_check io_queue_model_check io_resource_model_check` | Emitted the original build command and exact four model paths | Direct |
| `just fmt_check_all` | Passed | Direct |
| Pre-refactor pilot gates | `io_network_check`, `io_queue_check`, and `io_block_check` passed; the concurrently launched `io_driver_authority_check` failed during seL4 installation because another build removed `kernel.dtb`, then passed when rerun serially | Direct |
| Post-refactor no-build pilot gates | Network, queue, and block gates passed against the built images; driver authority passed after a full serial rebuild | Direct |
| Post-refactor public pilot recipes | `just io_network_check`, `just io_queue_check`, `just io_block_check`, and `just io_driver_authority_check` passed serially | Direct |
| Identity negative control | A temporary manifest declaring variant `wrong` was rejected with `wrong image variant 'wrong'` | Direct |
| Runtime negative controls | A missing-QEMU PATH was rejected, and a temporary QEMU stub that ignored progress was terminated and rejected after the one-second bound | Direct |
| `just sel4_gate_control_check` after runtime extraction | Passed all marker mutation controls | Direct |
| Pilot duplication measurement | Five I/O gates inspected; five identity implementations and five QEMU lifecycle implementations existed before. Four gates migrated, leaving zero such implementations in those four entrypoints and one independent implementation in the unselected I/O link gate | Direct |

## Decisions

- Decision: do not consolidate the component SDK scripts in this change.
- Rationale: measurement found 3,692 lines with only seven structurally duplicated top-level function shapes; shared code is limited to small process and repository helpers, while each checker owns a different release-lifecycle claim and evidence path. A dispatcher would combine filenames without extracting a common verification mechanism.
- Rejected alternative: a single `check-component-sdk.py` containing all six implementations. It would be a larger file with the same case boundaries and no meaningful reduction in mechanism duplication.
- Decision: retain the three small policy checkers in their current ownership boundaries.
- Rationale: architecture portability scans neutral Rust mechanism, lifecycle identity scans public boundary declarations, and Framework safety freezes a product storage-authority allowlist. Their inputs, failure semantics, and reuse differ.
- Decision: move the slot-pin implementation into the contract checker rather than retaining a separately invoked executable.
- Rationale: slot reasons describe generation-manifest semantics and already run only through `just contracts_check`; making the owning checker invoke the case removes the backlog-shaped executable boundary without changing the public gate.
- Decision: keep the `quality.just` cleanup bounded to comments and obvious orchestration.
- Rationale: Kani result handling, host-triple discovery, seL4 prerequisite loading, component allocator grouping, and root-test transcript validation are reusable mechanisms, but moving them safely requires an existing owning checker or library boundary rather than new one-off scripts.
- Rejected alternative: create helper scripts solely to shorten `quality.just`. That would move code without clarifying ownership and would violate the cleanup's behavior-preserving scope.
- Decision: pilot the shared runtime in network, queue, block, and driver-authority gates, leaving the I/O link gate local.
- Rationale: the four migrated gates cover a kernel-only boot, disposable writable and read-only disks with byte-for-byte postconditions, and a fixed virtio device. The link gate also owns a UDP echo thread and socket lifecycle, so retaining it avoids forcing case cleanup hooks into the first mechanism API.
- Decision: keep `CHAINS`, `FAILURE_MARKERS`, fixture checks, additional device arguments, backing-image verification, and success messages in each gate.
- Rationale: `scripts/lib/sel4_plane.py` owns only identity and process mechanics; `sel4_gate_markers.py` remains the sole marker-contract owner.

## Open risks and follow-ups

- [ ] The remaining plane scripts still carry local QEMU lifecycles; migrate the next homogeneous set only after the pilot API proves sufficient, and keep cases with background services or specialized settling behavior local until a concrete shared hook exists.
- [ ] Repeated quality-shell mechanisms should migrate only when an existing `scripts/lib/` or owning checker can provide one clear reusable boundary.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none.
- Serial/debugger/model output: direct public-gate and control-path results are summarized above; no raw transcript artifact was added.
- Related roadmap item: none.
