# Obsolete stage0 and custom-kernel handoff retirement

| Field | Value |
|---|---|
| Date | 2026-08-26 |
| Kind | Change |
| Status | Verified |
| Scope | Workspace membership and lockfile, developer/CI orchestration, Rust target provisioning, boot contracts and generated bindings, selector terminology, normative architecture documentation |
| Roadmap | P5, P5.4.final |
| Gates | `just sel4_boot_selection_check`, `just contracts_check`, `just generation_check` |
| Trigger | The current product boots through the pinned upstream kernel loader, seL4, and the immutable disk-backed selector in `slime-root`, while the retired `stage0/` crate and custom-kernel handoff ABI remained in the live workspace and tooling. |
| Baseline | Generation selection, signed-release and boot-bundle verification, pending-attempt durability, target admission, rollback, promotion, and component loading were already owned by the surviving seL4 path and had to remain behaviorally unchanged. |

## Summary

Removed the unused UEFI `stage0` crate, its workspace and CI/toolchain surface, and the custom-kernel handoff contract it alone consumed. Current source and documentation now describe the actual boot chain: pinned `sel4-kernel-loader` → seL4 → `slime-root`'s immutable disk-backed selector/root admission → userspace generation. The cleanup preserves BootState wire values, signed-release and boot-bundle verification, exact target-profile admission, retained kernel-image compatibility contracts, rollback/promotion semantics, and historical evidence. All regeneration, host contract, deterministic generation, selector QEMU, formatting, lint, dependency, Python, spelling, and devlog gates passed.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Workspace | Deleted `stage0/`, removed the workspace member, and let Cargo remove only the unreachable stage0/UEFI dependency closure from `Cargo.lock`. | Every workspace crate is part of the current product or a live host contract surface. |
| Tooling and CI | Removed stage0 format/lint recipes, UEFI target installation, and stage0 machete scope while retaining the public `fmt_check_all`, `lint_all`, and `machete` aggregates. | Public developer and CI commands name only surviving crates and targets. |
| Toolchains and policy | Removed both UEFI targets and the stage0-only Curve25519 backend override; rewrote the crypto policy rationale around signed releases and the immutable selector. | Provisioned targets and dependency policy match the live trust path. |
| Boot contracts | Deleted `contracts/handoff/v1`, its Rust modules, generator registration, contract check registration, and generated Python fragment; regenerated every remaining boot binding. | No dead custom-kernel ABI is exported or validated as a current boot boundary. |
| Ownership terminology | Updated source, schemas, checkers, and current documentation to name immutable selector/root admission and the seL4 rollback plane. | Durable-attempt, release, target, trace, and activation claims identify their actual owner without changing wire values or behavior. |
| Historical boundary | Preserved devlogs, backlog resolved records, P2/P2.1/P2.2 custom-kernel history, P5 retirement narrative, kernel-image schemas, `KIND_KERNEL`, and retained target profiles. | Historical evidence and compatibility data remain intact while current architecture is unambiguous. |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Generated boot bindings retain a handoff fragment or drift from Zutai sources | `just boot_gen` and `python3 scripts/generate/generate-boot-bindings.py --check` | Regeneration changes tracked output, references a deleted path, or reports stale bindings. |
| Contract/admission behavior changes with the dead ABI removal | `just contracts_check`, `just test_host`, `just generation_check` | Schema/model checks, host BootState/release/component admission tests, deterministic generation construction, or graph admission fails. |
| Selector rollback behavior is weakened or accidentally coupled to stage0 | `just sel4_boot_selection_check` | QEMU fails attempt consumption before candidate read, exhaustion fallback, malformed-pending refusal, health promotion, or sector-scoped mutation checks. |
| Workspace/tooling cleanup leaves stale paths or dependencies | `just fmt_check_all`, `just lint_all`, `just deny`, `just machete`, `just ruff`, `just typos` | A removed recipe/target/path remains, a dependency becomes unreachable or disallowed, or edited source/docs fail repository hygiene. |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just boot_gen` | Passed: remaining boot bindings regenerated and the handoff fragment was removed. | Direct |
| `python3 scripts/generate/generate-boot-bindings.py --check` | Passed: boot bindings are reproducible and current. | Direct |
| Live-root audit for stage0, UEFI-target, and handoff symbols | Passed: survivors are only immutable backlog/P2/P5 historical records; no live build, target, contract, generated binding, source comment, or normative owner references the deleted mechanism. | Direct |
| `just contracts_check` | Passed: all four BootState model scenarios, current generated contracts, and 280 boot-contract checks succeeded. | Direct |
| `just test_host` | Passed: 305 `boot-contracts` tests and all `slime-proto` host tests succeeded. | Direct |
| `just generation_check` | Passed: deterministic generation construction, seL4 pins, generated resources, and the component graph check succeeded. | Direct |
| `just sel4_boot_selection_check` | Passed after correcting a stale structural-check anchor for the current five-argument `package_image` call: attempts persisted across fresh QEMU processes, exhaustion rolled back, superseded pending format was refused safely, health promoted, and only BootState sectors changed. | Direct |
| `just fmt_check_all` | Passed. | Direct |
| `just lint_all` | Passed with warnings denied for boot-contracts, host protocol code, the seL4 root/child, runtime, and all component groups. | Direct |
| `just ruff` | Passed. | Direct |
| `just deny` | Passed: advisories, bans, licenses, and sources were accepted; existing unmatched allowlist warnings remained warnings. | Direct |
| `just machete` | Passed: no unused dependencies in `boot-contracts`, `components`, or `slime-root`. | Direct |
| `just typos` | Passed. | Direct |
| `just devlog_check` | Passed: 227 entries are registered and valid. | Direct |

## Decisions

- Decision: remove the crate and handoff ABI completely, without an alias, empty recipe, compatibility shim, or seL4-shaped replacement.
- Rationale: the upstream kernel-loader/BootInfo boundary and the `slime-root` selector are different mechanisms; preserving a dead custom-kernel ABI would misstate the trusted boot path and keep unreachable dependencies provisioned.
- Rejected alternative: globally replace historical stage0 wording. Historical devlogs, backlog resolutions, custom-kernel milestones, and retained compatibility contracts remain evidence of what existed and are not current architecture instructions.

## Open risks and follow-ups

- [ ] Repository-external branch protection may still name the deleted `lint-stage0` CI job; source correctly removes the lying job and cannot update external settings.

## Artifacts and provenance

- Focused report: this entry.
- Raw transcript: none retained.
- Serial/debugger/model output: observed through the verification commands listed above; no sibling capture retained.
- Related roadmap item: [P5 and P5.4.final](../../roadmap/07-architecture-portability.md#p5-sel4-microkernel-substitution).
