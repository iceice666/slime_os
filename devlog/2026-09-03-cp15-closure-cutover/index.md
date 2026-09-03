# CP15 — plane gates build by closure identity, and three reproducibility defects it exposed

| Field | Value |
|---|---|
| Date | 2026-09-03 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/lib/closure_image.py`, `scripts/check/check-system-image-aggregate.py`, `scripts/check/check-system-test-run.py`, `scripts/generate/generate-system-test-runs.py`, 36 `scripts/check/check-sel4-*.py` gates, `scripts/build/build-generation.py`, `scripts/build/build-system-image.py`, `scripts/build/build-sel4.py`, `contracts/system-test-run/v1/runs/`, `contracts/system-image-closure/v1/closures/`, `just/contracts.just` |
| Roadmap | CP14, CP15 |
| Gates | `just system_image_closure_aggregate_check`, `just system_test_run_check`, `just sel4_capability_layout_check`, `just sel4_qos_check`, `just sel4_call_check`, `just sel4_stream_check`, `just sel4_gate_control_check`, `just sel4_boot_layout_check` |
| Trigger | CP14 closed with four of five deliverables; CP15's cutover began from `0f71321d` |
| Baseline | 49 seL4 plane gates each built their image with `build-sel4.py --<name>-plane`; one test-run record existed of 45 planes; no gate proved the closure corpus and the booted-image corpus were the same set |
| Correction | 2026-09-03: extended from 36 to 41 migrated gates; the third defect and the reverted per-profile keying are recorded below |

## Summary

CP14's fifth deliverable and the tractable core of CP15. All 45 plane gates that boot a seL4 QEMU image now have a frozen test-run record declaring their execution-only inputs; 36 of 49 gates build their image by closure identity rather than by a plane flag; and one aggregate gate proves the closure corpus and the booted-image corpus are the same set in both directions. The cutover exposed three genuine reproducibility defects in the CP13/CP14 closure builder — all invisible while only the closure gates, which compare a closure against itself, exercised it — and one gap in CP14's own scenario modelling. All are fixed and verified by byte comparison against the legacy path.

## Changes

| Change | Why |
|---|---|
| `scripts/lib/closure_image.py` — a gate names a closure and receives a built image plus its build-result record; the identity is independently re-resolved from repository state before every build | A plane flag names behavior the caller chose and nothing in the image records which flag produced it; a closure identity names the inputs and is recorded beside the image. One shared seam rather than 49 near-identical subprocess calls |
| 36 plane gates migrated to `build_closure_image(CLOSURE)`, dropping `IMAGE_VARIANT`, `MANIFEST`, and `check_manifest` | The identity-manifest `variant` field is caller-chosen metadata; a closure identity is a claim about inputs. Verifying the recorded build-result digest against the bytes on disk replaces it |
| `just system_image_closure_aggregate_check` | Closes both drift directions: a closure nobody exercises is an untested build key, an image no closure describes is a build nobody can reproduce |
| Same gate: every surviving plane flag must be reachable from a `just` target, and no gate may build through `closure_image` *and* the legacy builder | The first made the eight orphaned flags a mandatory deletion rather than an optional cleanup. The second catches a half-migrated gate, which would silently keep booting the legacy artifact while appearing migrated |
| Same gate: `SLIME_*` knobs classified three ways — closure-declared, non-keying, legacy-pending-deletion — with all 18 legacy ones named | Two-way classification fails: calling them a failure makes the gate red for the whole migration and gets it disabled; calling them fine lets a *new* ambient knob hide among them. The named set can only shrink |
| `contracts/system-test-run/v1/runs/` — 45 frozen records, `--check` refuses drift, `--bless` re-freezes | CP11 put disks, devices, faults, timeouts, and marker contracts in a test-run contract so a marker oracle cannot alter the image it exercises. 44 planes still carried those as checker constants, where a doubled timeout was an unreviewed edit |
| `build_rust_components` takes `closure_target_name` | Under the `closure` profile it forced `sel4_manifest = None`, naming the component target directory `generation-<n>` where the legacy path named it `sel4-call-18`. CP3 established that this name reaches the shipped ELF's symbols |
| Closure component builds land in the canonical component target directory, not under the caller's output | Components built under `output` — which sits inside `ROOT` — had `--remap-path-prefix=ROOT=.` rewrite the leading portion, leaving the caller's path in `core::panic::Location` strings. **The same closure produced different bytes for different output directories** |
| `sel4-qos-death` scenario closure | `build-sel4.py` listed the qos variant in `STREAM_DEATH_VARIANTS`, so its publisher was compiled to exit mid-stream. CP14 converted that for the stream and fault planes but missed qos, whose closure declared every implementation `default` |
| `sel4_capability_layout_check` routed through CP14's `NegativeBuildCase` records | It selected each mutated root with an ambient `SLIME_B40_MUTATION` and rebuilt into `build/slime-sel4-boot.elf`, the path every other seL4 gate boots — requiring a rebuild-to-clean whose interruption would leave a mutated root where unrelated gates boot it |
| Eight plane flags deleted: io-block, io-driver-authority, io-link, io-network, io-queue, private-memory, reclamation, saturation | Reachable from no `just` target and no checker after migration. A build flag nobody invokes is a second way to produce an image no gate verifies |
| Three further flags deleted: fault-plane, traffic-plane, stress-plane | Orphaned by the second migration batch, by the same rule |
| `closure_image.build` removes a superseded build directory instead of building over it | The builder refuses a non-empty output, which is what stops a stale artifact being reported as this build's result — so reaching the rebuild path means nothing there may survive. Six gates failed on exactly this after a Justfile edit moved every closure identity |
| `sel4-boot-selector` closure over `sel4-channel` | CP14 declared the role but no closure carried it, because the legacy variant named the undrivable `sel4` graph. A selector root embeds no generation, so its base composition supplies only build inputs and can be any derived composition |
| The scenario byte arm digests each build's components immediately after that build | It globbed under the build output, which the reproducibility fix emptied of components; pointed at the canonical directory it then read one build's ELF as another's, because the three builds share that directory |
| An attempt to key the component target directory per build profile, reverted | CP3 established the directory name reaches the shipped ELF's symbols, so per-profile directories move *every* component's bytes and make a scenario appear to change components no profile names. Observed directly: the arm failed with "fabric-intruder changed although no profile names it" |

## Regression guards

- `just system_image_closure_aggregate_check` — 6 named drift controls, each perturbing one input in a scratch copy and requiring the specific refusal: an image with neither closure nor reason, a misnamed image, an exemption naming an unbooted image, an exemption whose reason is not a reason, a name that is both a scenario and a root role, and a newly introduced ambient build knob.
- `just system_test_run_check` — 5 controls: an unadmitted execution kind, a zero timeout, a timeout beyond the contract ceiling, an unresolvable closure identity, and an unadmitted fault kind.
- The flag-reachability and both-build-paths rules are themselves the guard against the legacy surface returning piecemeal.

## Verification

| Claim | Evidence | Kind |
|---|---|---|
| A closure build equals the legacy build of the same composition | `sel4-call` generation `8d1efed96a441fd1` from both paths after the two fixes, having differed as `8d1efed96a441fd1` vs `0c2d7be211f482a3` before | Direct |
| The output-path defect was real | `./build/closure/sel4-call/cargo/components/aarch...` found at six offsets inside `generation.bin`, in `core::panic::Location` strings | Direct |
| 41 gates pass from closure-built images | Booted and observed: call (47 markers, 10 chains), stream (57 frozen markers), qos (14 markers, 9 chains, including peer-dead retirement), loan, spawn, crossing, directory, operation, sample, supervision, visibility, traffic, boot, input, fault, clock-authority, wait-set, private-memory, lifecycle-restart, powerbox, saturation, scheduling-class, all five IO planes, storage, store, filesystem, replay, robot-runtime, transfer, reclamation, demo, stress, the boot-layout composer over all 31 plane layouts, the fabric aggregate's four boots (139 + 140 semantically identical records), and the aarch64 arms of generation and rollback | Direct |
| The B40 audit still refuses every mutation | `just sel4_capability_layout_check`: all 6 refused through typed negative cases; unmutated graph reached its supervisor terminal | Direct |
| Migration broke no unmigrated gate | `just sel4_gate_control_check` (45 gates, 1748 mutations), `just sel4_boot_layout_check` (31 layouts), `sel4_fabric_aggregate_check`, `sel4_component_graph_check`, `sel4_device_check` | Direct |
| Contracts and hosts still hold | `contracts_check`, `system_spec_check`, `system_composition_closure_check`, `system_image_builder_check`, `system_image_closure_check`, `system_test_run_check`, `system_image_closure_aggregate_check`, `test_host`, `ruff`, `devlog_check` | Direct |
| Flag deletion is safe | io_block, private_memory, sel4_reclamation, sel4_saturation, and all 31 boot layouts pass after deletion | Direct |

## Decisions

- **`closure_image` excludes QEMU invocation, markers, disks, and faults.** Those belong to the owning gate and its test-run record. Folding them into the build seam would rebuild the closure/test-run separation CP11 drew, in the one place best positioned to erase it.
- **The `SLIME_*` classification is three-way.** See the Changes table: a two-way split is either red for the whole migration or blind to a new knob.
- **Test-run records are a freeze, not the execution path.** The gates still run from their own constants. Claiming otherwise would be false; what the freeze buys today is that an execution input cannot change without a reviewed record change, and it is what the remaining migration gets verified against.
- **The mutation-to-record map is derived from the records.** Writing it by hand first got two of six names wrong (`badge`/`slot` for `aliased`/`wrong_slot`). The vocabulary is the contract's, and a second copy in a checker is one more place to disagree with what the build resolves.
- **`sel4-qos-death` is a separate closure, not a profile on `sel4-qos`.** The base must stay the plane whose publisher does not depart — the same reason `sel4-stream-death` is separate from `sel4-stream`.
- **Exemptions are enumerated by hand, not pattern-matched.** An image that stops being closure-reachable has to be added by a reviewed edit rather than quietly matching a wildcard.

## Open risks and follow-ups

- [ ] 6 gates remain on plane flags, each blocked on an input CP15 cannot supply: the component-graph and device gates boot the `sel4` composition's image, which admits an external product Slisp with no committed identity; matrix and c-runtime are CP12's hand-authored residuals; boot-selection reads a per-arm `boot_bundle_identity` that is test-run data rather than a build key; and the root-boot aggregate has no plane of its own. `VARIANT_MANIFESTS`, `VARIANT_TARGET_DIRS`, `VARIANT_IMAGES`, `VARIANT_GENERATION_DELTAS`, and the identity manifest's `variant` authority survive because those gates call them.
- [ ] Four gates are declared dual-path with named reasons: the boot-layout composer (29 of 31 planes have closures), the demo gate (its boot-selection and wrong-target arms), and the generation and rollback gates (their riscv64 arms; the closures name platform `qemu-arm-virt`).
- [ ] The 18 legacy-only `SLIME_*` knobs are enumerated and may only shrink; each should disappear with its last caller.
- [ ] CP15's two-clean-corpus-build check, the source guard against hard-coded checker image paths, and the SDK publication clause are not started.
- [ ] The gates consume their test-run records only as a freeze; making the records the execution input is the remaining half of CP14's fifth deliverable in practice.
- [ ] CP12's `sel4-matrix` and `sel4-c-runtime` remain hand-authored, with blockers recorded in `roadmap/00-backlog.md`.
- [ ] `generationCmd*`, `bootSelectionFail`, and `recoveryImage` profiles and the `boot-selector` root role are declared, resolvable, and gated but carried by no closure.

## Artifacts and provenance

- Commits `75ee6fd2` (reproducibility fixes and 35-gate migration), `38174f87` (qos death scenario), `a95e8738` (eight flag deletions), `672c6a1f` (capability-layout negative cases), `fe2e62d3` (aggregate gate), `79deb8c8` (test-run records).
- Aggregate gate at time of writing: 18 booted plane images, 10 closure-reachable and 8 exempt with declared reasons; 44 closures each exercised; 36 flags each reachable; 36 gates migrated with 13 on the legacy flag and none holding both; 6 controls refused.
- No `.rs` file changed in this entry's work.
