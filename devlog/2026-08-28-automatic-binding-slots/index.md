# First automatic instance-binding slots

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/generation-manifest/v1/compositions/{sel4-demo,sel4,sel4-loan,sel4-io-driver-authority}.zti`, matching plane gates, generation slot allocation and QEMU product verification |
| Roadmap | none |
| Gates | `just sel4_demo_check`, `just sel4_component_graph_check`, `just sel4_loan_check`, `just fmt_check_all`, `just lint_all`, `just ruff` |
| Trigger | Inventory found 582 of 600 composition instance bindings still carried explicit slots although the builder already assigns omitted slots deterministically |
| Baseline | Six `spawn-service` declarations, four loan bindings, and three I/O supervisor bindings pinned slots that their holders already resolve by name |

## Summary

The first post-inventory cuts remove thirteen redundant ordinary binding slot declarations from four compositions. `spawn-service` resolves its RPC, `sysinfo`, and context bindings by name; loan init and console resolve four bindings through runtime queries; the I/O driver supervisor resolves the worker executable, device, and MMIO authority by grant name before passing them positionally to its child. The builder restores every existing slot, all encoded generations remain byte-identical, the boot-layout resource remains unchanged, and rebuilt QEMU gates complete with the migrated bindings in service.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `sel4-demo.zti` | Omitted `slot` from `spawn-service-rpc`, `spawn-service-sysinfo`, and `spawn-service-sysinfo-context` | Ordinary named bindings do not restate a layout number unless an external positional contract requires it |
| `check-sel4-demo-plane.py` | Decode the source fixture before boot, require those three slots to be absent, run the production allocator, and require the resolved values to remain 0, 1, and 3 | A later source edit cannot silently re-pin an automatic batch or renumber its frozen resolved layout |
| Demo admission marker | Updated the stale grant count from 26 to the manifest's observed 28 | The behavioral gate matches the generation it actually builds rather than an older composition count |
| `sel4.zti` | Omitted `slot` from the product graph's `spawn-service-rpc`, `spawn-service-sysinfo`, and `spawn-service-sysinfo-context` bindings | The same component implementation uses the same name-resolved authority across product compositions rather than preserving unnecessary per-fixture numbers |
| `check-sel4-component-graph.py` | Require all three product source slots to remain omitted and resolve to 0, 1, and 3 before rebuilding or booting | The interactive product gate guards the source representation while exercising RPC reception, executable spawn, and context delivery |
| `sel4-loan.zti` | Omitted init's `dango-output`, `init-shared-buffer-factory`, and `sample-receiver-side` slots plus console's `console-shared-buffer-factory`; positional peer bindings remain pinned | Slot pinning is per holder: init resolves its three bindings by name and console resolves its factory by name, while console's channel and sample-receiver still consume endpoint halves positionally |
| `check-sel4-loan-plane.py` | Require all four named source slots across init and console to remain omitted and resolve to 0, 2, 4, and 1 before rebuild or boot | The loan gate protects the asymmetric cut and exercises console traffic, console's independent factory quota probe, init's factory allocation, receiver naming, and the full loan scenario |
| `sel4-io-driver-authority.zti` | Omitted the supervisor's `io-driver-worker-executable`, `probe-device`, and `probe-mmio` slots; retained its IRQ/DMA pins and every worker binding | The supervisor resolves the three migrated authorities by name, while the spawned worker still receives and consumes its four grants positionally |
| `check-sel4-io-driver-authority-plane.py` | Require the three supervisor source slots to remain absent and resolve to 0, 1, and 2 before build or boot | The authority gate exercises executable lookup, named device/MMIO delegation, worker restart, mediated MMIO, IRQ/DMA cleanup, and denial of an ungranted component |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A removed source slot is reintroduced | Matching plane gate | `<holder>/<grant> redundantly pins slot …` before build or boot |
| Automatic allocation moves a migrated binding | Matching plane gate | Resolved slot differs from the frozen value before boot |
| The encoded capability layout changes | Matching plane gate and the boot-layout resource check | Behavioral boot failure, byte comparison failure, or resolved layout fixture mismatch |
| The Python gate becomes invalid | `just ruff` | Ruff formatting or lint failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pinned Zutai `check` on `sel4-demo.zti` | Passed | Direct |
| Builder before/after comparison | Both `generation.bin` files had SHA-256 `38ec0e6cef6f38c605778602ee77090c2ffa94385e79dc52d899a604afab5727` | Direct |
| `python3 scripts/check/check-boot-layout-resource.py` | 31 seL4 planes agreed between the derived resource and root-resolved layout; check passed | Direct |
| Rebuilt `python3 scripts/check/check-sel4-demo-plane.py --arm slice` after each cut | Automatic-slot precheck passed; the image rebuilt and QEMU completed the C7 data path, C8 route graph, and product component graph in one boot | Direct |
| Product builder comparison | Pinned and automatic `sel4.zti` generations both had SHA-256 `9b77a503e0b064fe8f77569fbb27095f05688fb4473c936aa5e79cbaf9a7a8cd` | Direct |
| Rebuilt `python3 scripts/check/check-sel4-component-graph.py` after each product cut | Three-slot source omission guard passed; QEMU serial input traversed the resolved RPC endpoint, launched `sysinfo` through the resolved executable, and delivered its context while all four required resident instances remained live | Direct |
| Loan builder comparison | Pinned and automatic `sel4-loan.zti` generations both had SHA-256 `e3b5b2587933c31c727e043f22e1e46599ac5212f28593ffd3dcbdd1fa0b7d64` | Direct |
| Rebuilt `python3 scripts/check/check-sel4-loan-plane.py` after each loan cut | Four-slot source omission guard passed; init reached console, console resolved and exercised its independent factory quota, init allocated through its resolved factory, named the receiver through the resolved endpoint, and completed the 8192-byte sealed-loan and quota scenarios | Direct |
| I/O authority builder comparison | Pinned and automatic `sel4-io-driver-authority.zti` generations both had SHA-256 `054b659c5e24088b184eadf1df5436fda2421f266babca7758cd2bf452ad3992` | Direct |
| Rebuilt `python3 scripts/check/check-sel4-io-driver-authority-plane.py` | Three-slot source omission guard passed; the supervisor resolved and delegated executable/device/MMIO authority, the worker faulted with live authority, cleanup advanced the epoch, and the replacement completed | Direct |
| `just fmt_check_all` | Passed | Direct |
| `just lint_all` | Passed | Direct |
| `just ruff` plus Ruff format check on the changed gate | Passed | Direct |

## Decisions

- Decision: proceed by holder and binding, not by grant. One endpoint may be automatic for a name-resolving holder while its peer's half remains pinned for a positional receiver.
- Rationale: the six `spawn-service` declarations, four loan bindings, and three I/O supervisor bindings are queried from the authenticated generation by stable name; each migration reproduces its existing slot around explicit neighbours without changing positional consumers.
- Rejected alternative: remove every individually redundant slot found by the inventory. Equality with today's allocator is necessary but does not prove absence of a frozen artifact, positional protocol, or numeric test dependency.

## Open risks and follow-ups

- [ ] Remaining explicit instance slots need the same per-holder classification; this entry proves thirteen declarations across four compositions.
- [ ] `init-sample-receiver` stays pinned at 1 because init queries `executable:sample-receiver` from the boot layout, whose executable slot must remain aligned with the grant installation position; `powerbox-client` has no loan-path runtime consumer.
- [ ] Minted and notification bindings remain fully explicit and retain positional consumers; they are outside these batches.
- [ ] QoS participant controls stay pinned at 0, and `fabric-publisher-b-clock` stays pinned at 11 because omitting it alone resolves to 0; allocator equality must be measured, never inferred from displayed order.
- [ ] The I/O supervisor's `probe-irq` and `probe-dma` remain pinned: although either can be omitted in some isolated trials, omitting all name-resolved supervisor bindings together does not preserve the holder layout.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none; command results were observed directly in the implementation session.
- Serial/debugger/model output: rebuilt demo slice reached `SLIME_GRAPH HEALTHY generation=1 required=4 live=0 completed=4 failed=0`.
- Related roadmap item: none.
