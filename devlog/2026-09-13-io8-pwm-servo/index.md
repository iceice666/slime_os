# IO8: a declared device, the pwm-servo protocol, and the driver refusing on QEMU

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/device.rs`, `slime-root/src/graph_runtime/platform.rs`, `contracts/pwm-servo/v1`, `components/proto`, `components/lib/src/nvt_pwm.rs`, `components/services/nvt-pwm-driver`, `components/slisp`, `components/system/init`, `contracts/system-spec/v1/systems/sel4-pwm.zti`, `contracts/component-spec/v1/components/{nvt-pwm-driver,slisp}.zti`, `scripts/check/check-sel4-component-graph.py`, `scripts/check/check-slisp-core.py`, `scripts/generate/generate-pwm-servo-bindings.py`, `roadmap/11-io-substrate.md` |
| Work items | 01a07bac-acf5-7574-b0f5-d80c0fde6cd9, 01a07bac-83ad-7cfe-be0e-b589e74af74f, 01a07bac-cbe6-7051-9da4-d2a1d223dfee, 01a09a42-491e-773c-bb1e-dabe78640b61 |
| Gates | `just sel4_pwm_graph_check`, `just sel4_boot_layout_check`, `just slisp_core_check`, `just test_sel4_root`, `just test_host`, `just contracts_check` |
| Trigger | The ESC lane's second session (`devlog/2026-09-07-h1v1-esc-lane/plan.md`, Part C1): the bench facts were pinned and nothing in the tree could grant a fixed-address device to a userspace driver |
| Baseline | [`../2026-09-08-h1v1-pwm-probe/`](../2026-09-08-h1v1-pwm-probe/index.md): the four servo widths observed from U-Boot on the board; IO1's inventory virtio-only |

## Summary

On QEMU, where no PWM block exists, the `sel4-pwm` composition (generation 55, the product graph plus `nvt-pwm-driver`) admitted seven executables, init launched the driver at slot 10 as task 3 before Slisp, the root installed its declared quota (`SLIME_IO quota task=3 instance=nvt-pwm-driver devices=0`: the inventory holds no device on this platform), the driver bound nothing and printed `device absent, refusing requests`, Slisp came up with its three endpoints, the supervisor certified `required=5 live=5 idle=5 failed=0`, and the gate typed `(+ 1 1)`, `(pwm 0 1600)`, and `sysinfo` at the prompt: the shell answered `=> 2`, the driver logged `request ch=0 period_us=20000 pulse_us=1600` and the shell reported `! pwm no-device`, and `sysinfo` still spawned and exited cleanly. Behind that plane: the inventory the platform declares is a host-tested type with a `declared` constructor, the pwm-servo protocol is a Zutai contract rendered to Rust and C, the register arithmetic reproduces the four widths the bench observed, and the two encoders agree on one pinned request. IO8 closes on that observation; the register write to silicon is P6.D's.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/device.rs`, `graph_runtime/platform.rs` | `AuthorityInventory` and `AuthorityDevice` move from the binary's platform module into the device module, gain `declared(region)` (one pre-carved region is device 0 at offset 0), `install_region`, `push_device`, and `devices_mut` for the scan to fill them through, and two host tests; `unmap_region_at` and `put_irq` answer `DeviceError::{NoRegion,IrqSlot}` instead of `()` | A device the platform declares and one the root discovers share one inventory; its shape is host-tested rather than argued |
| `contracts/pwm-servo/v1`, `scripts/generate/generate-pwm-servo-bindings.py`, `components/proto` | A generated 32-byte request and 16-byte reply, the channel/period/pulse bounds, the count clock, the failsafe window, and seven statuses (`OK` 0, then `BAD_CHANNEL` … `MALFORMED` −1 … −6, distinct and negative by the schema's own `valid`); Rust and C bindings from one renderer; `valid_pwm_servo_request` is structural only, since each bound answers with its own status | Every cross-process format is a Zutai schema; both ends of the endpoint encode from it |
| `components/lib/src/nvt_pwm.rs` | Register offsets and the `PERIOD`/`EXT` pair for a pulse in a frame, pure and host-tested against the four widths the board observed | The driver's arithmetic is checked where a register cannot be |
| `components/services/nvt-pwm-driver` | Binds its declared device, maps the block's first page, refuses on a failed bind or a virtio magic at word 0, admits each request in the order channel → period → pulse → device with a named status each, programs `CTRL` 0 then the period pair then `ENABLE` or `LOAD`, reads every word back, and returns a channel driving above idle to the idle width after ten silent seconds; only channels 0–5 and the shared enable words are written | A request touches a register only after every bound passed; no write reaches PWM12's words |
| `components/slisp`, `scripts/check/check-slisp-core.py` | `(pwm ch us)` and `(pwm ch us period_us)` are an effect beside `spawn`, carried to slot 3 as one exchange and reported as `=> pwm ch us` or `! pwm <status>`; the host harness pins the `(pwm 0 1600)` request bytes that `components/proto/tests/pwm_servo.rs` pins from the Rust side | The shell's bytes and the driver's decoder agree by test |
| `components/system/init` | A product generation that declares `executable:nvt-pwm-driver` launches it before Slisp and supervises it as resident; the plain product graph declares none and launches none | Slisp's dependency on the driver holds at launch |
| `contracts/system-spec/v1/systems/sel4-pwm.zti`, `contracts/component-spec/v1/components/nvt-pwm-driver.zti`, `contracts/composition-inventory` | Generation 55: the product graph plus the driver, with `init-nvt-pwm-driver` at init's slot 10, `slisp-pwm-rpc` at Slisp's slot 3 and the driver's 0, its device and region at 1 and 2, a `monotonicRead` clock row, and a budget of one 4 KiB region and one mapping for device 0 | The driver's authority is declared, bounded, and frozen |
| `scripts/check/check-sel4-component-graph.py`, `just sel4_pwm_graph_check`, `check-sel4-boot-layout.py`, `generate-system-test-runs.py` | The product-graph checker gains a `--composition sel4-pwm` arm typing `(pwm 0 1600)` between `(+ 1 1)` and `sysinfo`, and a `--transcript` option; the boot-layout gate and the run records gain the plane | The plane is observed by the checker that owns the product graph, not by a new one |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The driver writes a register before a bound is checked, or serves with no device | `just sel4_pwm_graph_check` | `! pwm no-device` missing after `[nvt-pwm-driver] request ch=0 period_us=20000 pulse_us=1600`, or `[nvt-pwm-driver] device absent, refusing requests` missing |
| Init launches Slisp before the driver, or the root installs no quota | `just sel4_pwm_graph_check` | the `nvt-pwm-driver` markers out of order against Slisp's, or `SLIME_IO quota task=3 instance=nvt-pwm-driver devices=0` missing |
| The two encoders drift | `just slisp_core_check`, `just test_host` | `pwm request encoding drifted from the pinned vector`, or `the_pinned_request_vector_is_the_generated_encoding` |
| The register arithmetic drifts from the board's observed widths | `just test_host` | `the_pinned_servo_encodings_are_reproduced` |
| The inventory's shape regresses | `just test_sel4_root` | `a_declared_region_is_device_zero_at_offset_zero_and_nothing_else`, `devices_are_pushed_in_order_against_held_regions_and_the_ceiling` |
| The plane's capability layout drifts | `just sel4_boot_layout_check` | `sel4-pwm` against its frozen fixture |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 66 (29 product markers, 36 pwm markers, one order-independent) |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_pwm_graph_check` | Passed, kept as [`sel4-pwm-graph.log`](sel4-pwm-graph.log) (980 lines): `SLIME_ROOT generation admitted number=55 executables=7 instances=7`, `spawn authorized task=0 slot=10 component=nvt-pwm-driver grants=0`, `SLIME_IO quota task=3 instance=nvt-pwm-driver devices=0 shared_granule=0`, `spawned task=0 child=3 component=nvt-pwm-driver … endpoints=1`, Slisp task 4 with endpoints at slots 33, 36, 35, `SLIME_GRAPH healthy generation=55 … required=5 live=5 idle=5 failed=0`, `[nvt-pwm-driver] device absent, refusing requests`, `(+ 1 1)` → `=> 2`, `(pwm 0 1600)` → `[nvt-pwm-driver] request ch=0 period_us=20000 pulse_us=1600` → `! pwm no-device`, `sysinfo` → `[sysinfo] spawned through profile`, `component exit task=5 status=0`, `=> spawned sysinfo`. Four boots preceded it: the closure input `slisp.elf` had to be rebuilt (the generator does not), a failed `io_mmio_map` on an empty inventory was first treated as fatal, and the driver's per-idle-turn clock reads made the root print `SLIME_CLOCK served` between Slisp's echo and its answer, breaking the `(+ 1 1)\n=> 2` adjacency; the driver now reads the clock only with a device present and a channel live | Direct |
| `just sel4_component_graph_check` | Passed: the plain product graph's 29 markers unchanged with the modified init and Slisp | Direct |
| `just slisp_core_check` | Passed: the host harness prints the fourth line, `Slisp effects: pwm selection and the pinned request vector passed`, and the `sel4-slisp` plane holds | Direct |
| `just sel4_boot_layout_check` | Passed after `just sel4_boot_layout_bless` added `sel4-pwm` (7 slots); every other fixture unchanged | Direct |
| `just sel4_gate_control_check` | Passed: 48 gates reject 1986 mutated transcripts and layouts; the product-graph gate pinned at 66 markers (was 29) | Direct |
| `just io_driver_authority_check`, `just sel4_root_boot_check` | Passed: the moved inventory's scan path and the root's boot unchanged | Direct |
| `just test_sel4_root` | Passed, 242 (240 + the two inventory tests) | Direct |
| `just test_host` | Passed, with `pwm_servo` (4 tests) and `nvt_pwm` (3 tests) | Direct |
| `just contracts_check`, `just component_spec_check` (84 records), `just component_crate_split_check`, `just system_spec_check` (45 systems), `just system_composition_closure_check`, `just system_image_closure_check` (60 closures), `just system_test_run_check` (51 records), `just system_image_closure_aggregate_check` | Passed; the composition inventory first refused `sel4-pwm` out of sort order, fixed and rerun | Direct |
| `just generation_check` | Passed with the default `TMPDIR`; fails with `TMPDIR` below the repository root, filed as `01a09a42-491e-773c-bb1e-dabe78640b61` (deferred) | Direct |
| `just deny`, `just machete`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Passed. `fmt_check_all` failed once on the new files; `cargo fmt` was applied, the closures and run records regenerated for the formatted trees, and every closure-dependent gate, both product-graph arms, the boot-layout, Slisp core, lint, root-test, and host-test gates reran green on the formatted tree (the first rerun died when the host's disk filled: btrfs snapshots pinned every deleted extent, pruned by the operator) | Direct |

## Decisions

- Decision: the inventory lives in `device.rs`, not `graph_runtime/platform.rs`.
- Rationale: the plan asked for `declared` to be host-tested and cfg-free, but `graph_runtime` is a module of the binary alone, so nothing in it can carry a host test, and on QEMU `declared` would have had no caller until the board session — dead code under `-D warnings`. Moving the type makes the shape testable and leaves the scan where the platform is.
- Rejected alternative: `#[allow(dead_code)]` on a function nothing calls.
- Decision: the plane's owning gate is a second arm of `check-sel4-component-graph.py`, `just sel4_pwm_graph_check`, rather than `sel4_boot_layout_check` alone.
- Rationale: the boot-layout gate stops the boot when the layout block closes and never sees a driver marker, so the component spec's `passFailCriteria` could not name one; the product-graph checker already boots the product graph and types into Slisp, and `sel4-pwm` is that graph plus one driver, so the arm is a case of the checker that owns the product graph, which is what the verification discipline asks for. The Slisp round trip is observed on QEMU as a consequence.
- Rejected alternative: a new `check-sel4-pwm-plane.py`; a transcript kept as evidence with no gate.
- Decision: `build-sel4.py --pwm-graph` is not added here.
- Rationale: nothing on QEMU consumes it — the plane is built by closure identity like every other gate's — and the board gate that will select it is P6.D's; a variant with no consumer is plumbing.
- Decision: `STATUS_MALFORMED` (−6) joins the plan's six statuses.
- Rationale: a request that fails the structural validator has to be answered with something, and `BAD_CHANNEL` would misname it.
- Decision: the product Slisp digest in `slisp.zti` is refreshed in this change.
- Rationale: the shell's sources changed, and the digest is content-bound by the builder; the refresh is the same one `37e574bb` made.

## Open risks and follow-ups

- [ ] P6.D (`01a07bac-cbe6-7051-9da4-d2a1d223dfee`): the board half — the root's TOP/CG/WDT/PWM carve, clock and pinmux bring-up, `AuthorityInventory::declared` fed from the carve, `check-nt98690-pwm.py`, and the first register write to silicon.
- [ ] The driver's page also holds channels 6–12 and the shared enable words; no capability bound narrows a 4 KiB page, so the channel policy in the driver is the only guard. Recorded here as the known limit the plan named.
- [ ] `just generation_check` fails with `TMPDIR` below the repository root (the setting the closure gates need on this host) and passes with the default; filed and deferred as `01a09a42-491e-773c-bb1e-dabe78640b61`, unrelated to this change's inputs.
- [ ] The failsafe is exercised by no gate: QEMU binds no device, so no channel is ever live. It is observed on the board in P6.D or not at all.

## Artifacts and provenance

- Raw transcript: [`sel4-pwm-graph.log`](sel4-pwm-graph.log), `just sel4_pwm_graph_check` with `--transcript`.
- Plan of record: [`../2026-09-07-h1v1-esc-lane/plan.md`](../2026-09-07-h1v1-esc-lane/plan.md), Part C1.
- Related work items: IO8 `01a07bac-acf5-7574-b0f5-d80c0fde6cd9`; P6.PWM `01a07bac-83ad-7cfe-be0e-b589e74af74f`; P6.D `01a07bac-cbe6-7051-9da4-d2a1d223dfee`.
