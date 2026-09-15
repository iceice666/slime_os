# IO8: a declared device, the pwm-servo protocol, and a driver whose register model is the platform's

| Field | Value |
|---|---|
| Date | 2026-09-14 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/device.rs`, `slime-root/src/graph_runtime/platform.rs`, `contracts/pwm-servo/v1`, `components/proto`, `components/lib/src/servo_failsafe.rs`, `components/services/pwm-servo-driver`, `components/runtime/{c/component_runtime.c,include/slime/component_runtime.h}`, `components/slisp`, `components/system/init`, `contracts/system-spec/v1/systems/sel4-pwm.zti`, `contracts/component-spec/v1/components/{pwm-servo-driver,slisp}.zti`, `scripts/check/check-sel4-component-graph.py`, `scripts/check/check-slisp-core.py`, `scripts/generate/generate-pwm-servo-bindings.py`, `roadmap/11-io-substrate.md` |
| Work items | 01a07bac-acf5-7574-b0f5-d80c0fde6cd9 |
| Gates | `just sel4_pwm_graph_check`, `just sel4_boot_layout_check`, `just slisp_core_check`, `just test_sel4_root`, `just test_host`, `just contracts_check` |
| Trigger | IO1 grants a driver its hardware only through the virtio-mmio scan, so a fixed-address block could not be granted to a userspace driver at all; the first consumer is a servo PWM output |
| Baseline | IO1's inventory virtio-only; the `sel4` product graph of three resident services (`just sel4_component_graph_check`, 29 markers) |

## Summary

On QEMU, where no PWM block exists, the `sel4-pwm` composition (generation 55, the product graph plus `pwm-servo-driver`) admitted seven executables, init launched the driver at slot 10 as task 3 before Slisp, the root installed its declared quota (`SLIME_IO quota task=3 instance=pwm-servo-driver devices=0`: the inventory holds no device on this platform), the driver bound nothing and printed `device absent, refusing requests`, Slisp came up with its three endpoints, the supervisor certified `required=5 live=5 idle=5 failed=0`, and the gate typed `(+ 1 1)`, `(pwm 0 1600)`, and `sysinfo` at the prompt: the shell answered `=> 2`, the driver logged `request ch=0 period_us=20000 pulse_us=1600` and the shell reported `! pwm no-device`, and `sysinfo` still spawned and exited cleanly. Behind that plane: the inventory the platform declares is a host-tested type with a `declared` constructor, the pwm-servo protocol is a Zutai contract rendered to Rust and C, the two encoders agree on one pinned request, the driver's failsafe is a pure host-tested module, and the driver asks a platform's register model for three operations through a seam this tree fills with a model that attaches to nothing. IO8 closes on that observation; the first register write is a platform's, behind that seam.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `slime-root/src/device.rs`, `graph_runtime/platform.rs` | `AuthorityInventory` and `AuthorityDevice` move from the binary's platform module into the device module, gain `declared(region)` (one pre-carved region is device 0 at offset 0), `install_region`, `push_device`, and `devices_mut` for the scan to fill them through, and two host tests; `unmap_region_at` and `put_irq` answer `DeviceError::{NoRegion,IrqSlot}` instead of `()` | A device the platform declares and one the root discovers share one inventory; its shape is host-tested rather than argued |
| `contracts/pwm-servo/v1`, `scripts/generate/generate-pwm-servo-bindings.py`, `components/proto` | A generated 32-byte request and 16-byte reply, the channel/period/pulse bounds, the count clock, the failsafe window, and seven statuses (`OK` 0, then `BAD_CHANNEL` … `MALFORMED` −1 … −6, distinct and negative by the schema's own `valid`); Rust and C bindings from one renderer; `valid_pwm_servo_request` is structural only, since each bound answers with its own status | Every cross-process format is a Zutai schema; both ends of the endpoint encode from it |
| `components/lib/src/servo_failsafe.rs` | Per-channel silence deadlines over caller-supplied milliseconds: armed by `accepted` for the one channel the device accepted a request on, disarmed at or below idle, listed by `due`, cleared by `returned`, and pushed out by the retry interval, not the window, by `deferred`; five host tests | The failsafe's policy is observed on the host, where the no-device QEMU plane cannot reach it |
| `components/services/pwm-servo-driver` | Binds its declared device, maps the block's first page, refuses on a failed bind, on a virtio magic at word 0, or on a page its register model does not know; admits each request in the order channel → period → pulse → device with a named status each; asks the model to program or disable a channel only after every bound passed; returns a channel driving above idle to the idle width once the device has accepted no request for that channel for ten seconds, retrying a refused return each second; answers each request through the caller's reply capability; programs only channels 0–5 | A request touches a register only after every bound passed; one channel's traffic, or a refused request, never holds another's output; a client that stops receiving cannot park the driver |
| `components/services/pwm-servo-driver/src/block.rs` | The register model seam: `Model::attach(page)`, `program(channel, period_us, pulse_us)`, `disable(channel)`. The model this tree carries attaches to nothing, so on every plane here the driver refuses; a platform with a block supplies its model in this module and nothing above changes | What a page's words mean is the platform's; the driver above the seam is vendor-neutral and the seam is the whole surface a model must provide |
| `components/slisp`, `scripts/check/check-slisp-core.py` | `(pwm ch us)` and `(pwm ch us period_us)` are an effect beside `spawn`, carried to slot 3 as one call and reported as `=> pwm ch us` or `! pwm <status>`; the host harness pins the `(pwm 0 1600)` request bytes that `components/proto/tests/pwm_servo.rs` pins from the Rust side | The shell's bytes and the driver's decoder agree by test |
| `components/runtime/c/component_runtime.c`, `component_runtime.h` | `slime_endpoint_call`, one `seL4_Call` beside the send-then-receive `slime_endpoint_exchange`, both validating the answer through one `collect_reply`; Slisp's pwm effect uses the call, its spawn effect keeps the exchange because init and the spawn service answer with a send | A C client of a server that polls and replies is parked on the reply atomically, so the server never waits on it |
| `components/system/init` | A product generation that declares `executable:pwm-servo-driver` launches it before Slisp and supervises it as resident; the plain product graph declares none and launches none | Slisp's dependency on the driver holds at launch |
| `contracts/system-spec/v1/systems/sel4-pwm.zti`, `contracts/component-spec/v1/components/pwm-servo-driver.zti`, `contracts/composition-inventory` | Generation 55: the product graph plus the driver, with `init-pwm-servo-driver` at init's slot 10, `slisp-pwm-rpc` at Slisp's slot 3 and the driver's 0, its device and region at 1 and 2, a `monotonicRead` clock row, and a budget of one 4 KiB region and one mapping for device 0 | The driver's authority is declared, bounded, and frozen |
| `scripts/check/check-sel4-component-graph.py`, `just sel4_pwm_graph_check`, `check-sel4-boot-layout.py`, `generate-system-test-runs.py` | The product-graph checker gains a `--composition sel4-pwm` arm typing `(pwm 0 1600)` between `(+ 1 1)` and `sysinfo`, and a `--transcript` option, beside the `--platform` arm the x86-64 bring-up gave it; the boot-layout gate and the run records gain the plane | The plane is observed by the checker that owns the product graph, not by a new one |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The driver asks the model to write before a bound is checked, or serves with no device | `just sel4_pwm_graph_check` | `! pwm no-device` missing after `[pwm-servo-driver] request ch=0 period_us=20000 pulse_us=1600`, or `[pwm-servo-driver] device absent, refusing requests` missing |
| Init launches Slisp before the driver, or the root installs no quota | `just sel4_pwm_graph_check` | the `pwm-servo-driver` markers out of order against Slisp's, or `SLIME_IO quota task=3 instance=pwm-servo-driver devices=0` missing |
| The two encoders drift | `just slisp_core_check`, `just test_host` | `pwm request encoding drifted from the pinned vector`, or `the_pinned_request_vector_is_the_generated_encoding` |
| The silence window is shared across channels or extended by refused traffic, or a refused return waits a whole window | `just test_host` | `traffic_for_another_channel_does_not_extend_a_deadline`, `a_refused_return_is_retried_after_the_retry_interval_not_a_window` |
| The driver answers with a blocking send, or Slisp stops calling, so the round trip no longer pairs a call with a reply | `just sel4_pwm_graph_check` | `! pwm no-device` missing after the request line. It observes the pairing, not the starvation itself, which needs a live channel no QEMU plane has |
| The inventory's shape regresses | `just test_sel4_root` | `a_declared_region_is_device_zero_at_offset_zero_and_nothing_else`, `devices_are_pushed_in_order_against_held_regions_and_the_ceiling` |
| The plane's capability layout drifts | `just sel4_boot_layout_check` | `sel4-pwm` against its frozen fixture |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 66 (29 product markers, 35 pwm markers, two order-independent) |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-sel4-component-graph.py --composition sel4-pwm --transcript …` (`just sel4_pwm_graph_check`) | Passed on the first boot, kept as [`sel4-pwm-graph.log`](sel4-pwm-graph.log) (981 lines): `SLIME_ROOT generation admitted number=55 executables=7 instances=7`, `spawn authorized task=0 slot=10 component=pwm-servo-driver grants=0`, `SLIME_IO quota task=3 instance=pwm-servo-driver devices=0 shared_granule=0`, Slisp task 4 with endpoints at slots 33, 36, 35, `SLIME_GRAPH healthy generation=55 … required=5 live=5 idle=5 failed=0`, `[pwm-servo-driver] device absent, refusing requests`, `(pwm 0 1600)` → `[pwm-servo-driver] request ch=0 period_us=20000 pulse_us=1600` → `! pwm no-device`, `=> spawned sysinfo` | Direct |
| `just sel4_component_graph_check` | Passed: the plain product graph's 29 markers unchanged with the modified init and Slisp | Direct |
| `just slisp_core_check` | Passed: the host vectors (including the pinned `(pwm 0 1600)` request bytes) and the `sel4-slisp` plane | Direct |
| `just sel4_boot_layout_check` | Passed after `just sel4_boot_layout_bless` added `sel4-pwm` (7 slots): 32 plane layouts match their fixtures, every other fixture unchanged | Direct |
| `just sel4_gate_control_check` | Passed: 45 gates reject 1873 mutated transcripts and layouts, 8 identity cases and 4 runtime cases; the product-graph gate pinned at 66 markers (was 29) | Direct |
| `just io_driver_authority_check`, `just sel4_root_boot_check` | Passed: the moved inventory's scan path (exact mediated MMIO, bounded IRQ authority, ungranted denial) and the root's boot unchanged | Direct |
| `just test_sel4_root` | Passed, 245 (243 + the two inventory tests) | Direct |
| `just test_host` | Passed, with `pwm_servo` (4 tests) and `servo_failsafe` (5 tests) | Direct |
| `just contracts_check`, `just component_spec_check` (85 records), `just component_crate_split_check` (77 crates), `just system_spec_check`, `just system_image_closure_check` (55 closures), `just system_test_run_check` (53 records), `just system_image_closure_aggregate_check` | Passed, after `python3 scripts/generate/generate-system-image-closures.py` and `just system_test_run_bless` for the new plane and the changed tree identities. The run-record and aggregate gates were red on the baseline before this change and are fixed in the same branch, see [`../2026-09-14-source-gates-after-61/`](../2026-09-14-source-gates-after-61/index.md) | Direct |
| `just generation_check` | Passed: two isolated builds produced byte-identical `generation.bin` and `boot-store.bin` | Direct |
| `just deny`, `just machete`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just devlog_check`, `just tasks_check` | Passed. `fmt_check_all` first asked for one line of the driver wrapped; after `cargo fmt` the closures and run records were re-rendered for the changed components tree and every closure-dependent gate above reran green, the pwm transcript kept from that rerun. `lint_all` now also lints the x86-64 root and needs `just x86_64_sel4_image_check`'s prefix first | Direct |

## Decisions

- Decision: the inventory lives in `device.rs`, not `graph_runtime/platform.rs`.
- Rationale: `declared` should be host-tested and cfg-free, but `graph_runtime` is a module of the binary alone, so nothing in it can carry a host test, and on QEMU `declared` would have had no caller — dead code under `-D warnings`. Moving the type makes the shape testable and leaves the scan where the platform is.
- Rejected alternative: `#[allow(dead_code)]` on a function nothing calls.
- Decision: the register model is a seam in the driver crate, and this tree's model attaches to nothing.
- Rationale: how a block packs a frame and pulse into its words, latches a running channel, and reads back is a platform's register layout, which a vendor-neutral tree neither has nor should carry; the driver's protocol, admission order, failsafe, and reply discipline do not depend on it. Three operations are the whole surface: `attach` decides whether the page is a block the model knows, `program` and `disable` write and read back. A platform supplies its model in `block.rs` and the composition, gate, and checker above it are unchanged.
- Rejected alternative: a generic model parameterised by register offsets, which cannot express how a block packs or latches; a driver crate per platform, which would duplicate the protocol and the failsafe.
- Decision: the plane's owning gate is a second arm of `check-sel4-component-graph.py`, `just sel4_pwm_graph_check`, rather than `sel4_boot_layout_check` alone.
- Rationale: the boot-layout gate stops the boot when the layout block closes and never sees a driver marker, so the component spec's `passFailCriteria` could not name one; the product-graph checker already boots the product graph and types into Slisp, and `sel4-pwm` is that graph plus one driver, so the arm is a case of the checker that owns the product graph, which is what the verification discipline asks for.
- Rejected alternative: a new `check-sel4-pwm-plane.py`; a transcript kept as evidence with no gate.
- Decision: `STATUS_MALFORMED` (−6) joins the six statuses the protocol started with.
- Rationale: a request that fails the structural validator has to be answered with something, and `BAD_CHANNEL` would misname it.
- Decision: a failsafe return the device refuses is retried every second, a driver constant rather than a schema constant.
- Rationale: the retry interval is the driver's recovery policy, not something a client encodes or relies on; each attempt prints a report, so a persistent fault retried on every scheduling turn would flood the console, and a whole window would leave the actuator at its last output ten seconds at a time.
- Rejected alternative: a `failsafeRetryNs` schema constant; retrying on every idle turn.
- Decision: the driver answers through the caller's reply capability, and Slisp's pwm effect calls it.
- Rationale: `slime_rt::send` is a blocking rendezvous that never reports `ERR_WOULDBLOCK`, so a client stopped between its send and its receive would hold the driver, and every live channel, indefinitely. Under the non-MCS kernel a non-blocking receive that takes a caller installs its reply capability like a blocking one, and the reply cannot block; the contract names the exchange one call. The capability lives only until the next receive, so the answer precedes the next poll.
- Rejected alternative: `try_send`, which drops the answer when the client has not yet reached its receive and reports nothing.
- Decision: Slisp's spawn effect keeps `slime_endpoint_exchange`.
- Rationale: init and the spawn service answer with a send, so a call against them would wait on a reply that never comes; neither has a time-driven duty.
- Decision: the supervision-collected record is order-independent on the pwm arm as it is on the product arm.
- Rationale: the root collects `sysinfo`'s detached supervision record while Slisp prints its own reply; neither waits on the other, and the product arm already learned that pinning the order fails on whichever runs second.

## Open risks and follow-ups

- [ ] The register model behind the seam: a platform that carries a PWM block supplies `Model` in `block.rs`, feeds `AuthorityInventory::declared` from its root's carve, and observes the pad on a board gate. Nothing on QEMU can.
- [ ] The driver's page also holds whatever else the block's first 4 KiB carries; no capability bound narrows a page, so the channel policy in the driver is the only guard. Recorded here as the known limit.
- [ ] Slisp's spawn exchange and the servers that answer with a send (`spawn-service`, `sel4-generation-manager`, `sel4-filesystem-service`) keep the send-then-receive shape. None has a time-driven duty, so no failsafe is at stake; moving them to call/reply is separate work.
- [ ] The failsafe's register path is exercised by no QEMU gate: QEMU binds no device, so no channel is ever live. Its policy is host-tested in `servo_failsafe`; its returns to idle on silicon are a platform's board gate to observe.

## Artifacts and provenance

- Raw transcript: [`sel4-pwm-graph.log`](sel4-pwm-graph.log), `just sel4_pwm_graph_check`'s boot with `--transcript`.
- Related work item: IO8 `01a07bac-acf5-7574-b0f5-d80c0fde6cd9`.
