# IO9: bounded serial transmit, a MAVLink heartbeat contract, and a producer that keeps a one-second grid

| Field | Value |
|---|---|
| Date | 2026-09-15 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/serial-device/v1`, `contracts/mavlink-heartbeat/v1`, `components/proto`, `components/lib/src/uart16550.rs`, `components/services/uart16550-driver`, `components/applications/mavlink-heartbeat`, `components/system/init`, `contracts/system-spec/v1/systems/sel4-mavlink.zti`, `contracts/component-spec/v1/components/{uart16550-driver,mavlink-heartbeat}.zti`, `scripts/check/check-sel4-component-graph.py`, `scripts/lib/mavlink.py`, `scripts/generate/generate-{serial-device,mavlink-heartbeat}-bindings.py`, `roadmap/11-io-substrate.md` |
| Work items | 01a08ac5-14ba-7d23-a751-b06678692de9 |
| Gates | `just sel4_mavlink_graph_check`, `just sel4_boot_layout_check`, `just test_host`, `just contracts_check` |
| Trigger | No component could transmit on a serial port, and nothing paced a component in real time against the root's clock; a telemetry heartbeat needs both |
| Baseline | IO8's declared-device inventory and the `sel4-pwm` arm of the product-graph checker; `clock RATE_READ` already served under `monotonicRead` |

## Summary

On QEMU, where no port exists, the `sel4-mavlink` composition (generation 56, the product graph plus `uart16550-driver` and `mavlink-heartbeat`) admitted eight executables; init launched the driver at slot 10 as task 3 and the producer at slot 11 as task 4, both before Slisp; the root installed the driver's declared quota (`SLIME_IO quota task=3 instance=uart16550-driver devices=0`) and the producer's timer authority (`timers=2 badge=0x200`); the supervisor certified `required=6 live=6 idle=6 failed=0`. The driver bound nothing and printed `device absent, refusing requests`. The producer read the counter's rate from the root (`62500000`), and sent `seq=0` through `seq=5` at deadlines exactly 62 500 000 ticks apart, each after its deadline and each refused `no-device`; Slisp still evaluated `(+ 1 1)` and spawned `sysinfo`. On the host, the generated Rust codec and the hand-written Rust encoder reproduce the contract's three pinned frames, as does the checkers' Python encoder and decoder, and the 16550 register sequence is exercised against a scripted register model. IO9 closes on those observations; the first byte on a wire is a platform's, behind the driver's line seam.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/serial-device/v1`, `scripts/generate/generate-serial-device-bindings.py`, `components/proto` | A generated 64-byte request (`op`, `length`, 52 payload bytes) and 16-byte reply (`bytes_written`, signed `status`, transmitter-empty `detail`), seven distinct statuses pinned by the schema's `valid`; `valid_serial_request` admits one operation, a length in 1–52, and zeroed reserved and unused payload bytes; `valid_serial_reply` admits the status vocabulary and a byte count the request could carry; seven host tests | Every cross-process format is a Zutai schema, and every admissible encoding is canonical |
| `contracts/mavlink-heartbeat/v1`, `scripts/generate/generate-mavlink-heartbeat-bindings.py`, `scripts/lib/mavlink_heartbeat_contract.py` | The 21-byte MAVLink v2 HEARTBEAT layout, the declared identity, and three pinned frames (seq 0, 1, 255) as byte lists, rendered to Rust and Python from one renderer; `valid` requires each frame's header and payload bytes to say what the constants say | The frame a Slime heartbeat sends has one source of truth, which the Rust producer and the Python checkers both import |
| `components/proto/src/mavlink.rs`, `scripts/lib/mavlink.py` | CRC-16/MCRF4XX and `encode_heartbeat` in Rust over the generated codec; the Python module keeps its encoder and resynchronising decoder but takes every constant, the frame `struct`, and the pinned frames from the generated module, and loses a duplicated docstring paragraph; five Rust host tests | The checksum is arithmetic both sides implement and both must reproduce the pinned frames; no hand-written layout remains a source of truth |
| `components/lib/src/uart16550.rs` | 16550 configuration (drain, interrupts and modem control off, FIFOs, 8N1 through the divisor latch) with every latch read back and a mismatch named by register; polled transmit that waits for the holding register before each byte and for the transmitter to drain after the last; every wait bounded by four character times at the line's own rate, checked against the clock every sixteen status reads; eight host tests against a scripted register model | A register sequence nothing on QEMU can execute is still observed; a stuck line answers with how far it got rather than hanging, at any baud |
| `components/services/uart16550-driver` | Binds its declared device, maps the first page, refuses on a failed bind, a virtio magic, a line its `line.rs` does not name, or a layout outside the page; receives blocking and answers each call through the caller's reply capability; admits identity, operation, length, and canonical encoding, each with its own status, before the device | The driver above the seam is vendor-neutral; the line (clock, divisor, stride, access width) is the whole surface a platform supplies |
| `components/applications/mavlink-heartbeat` | Resolves its tick notification by name, reads the counter rate once, and sends one frame per period on a grid of deadlines a slow send or a late wake never shifts; arms at most one timer per wait and ignores wakes without the timer badge; names every reply status, and reports a reply that fails validation as `bad-reply` | A periodic producer's cadence is the root's clock, not the scheduler's; a refused send never stops the cadence |
| `components/system/init` | Any declared resident driver or producer (`pwm-servo-driver`, `uart16550-driver`, `mavlink-heartbeat`) is launched in that order before Slisp and supervised; the plain product graph declares none | Servers are live before their clients; the two-arm launch per optional component is one list |
| `contracts/system-spec/v1/systems/sel4-mavlink.zti`, component-spec records, `contracts/composition-inventory` | Generation 56: the product graph plus both components, `init-uart16550-driver` at init's slot 10 and `init-mavlink-heartbeat` at 11, `heartbeat-serial-rpc` at slot 0 of both, the driver's device and region at 1 and 2, the producer's tick notification signalled by init, two clock rows, and a budget of one 4 KiB region and one mapping | The new authority is declared, bounded, and frozen |
| `scripts/check/check-sel4-component-graph.py`, `just sel4_mavlink_graph_check`, `check-sel4-boot-layout.py`, `generate-system-test-runs.py`, `generate-system-image-closures.py`, `check-sel4-gate-controls.py` | A third `--composition` arm: typing waits for both Slisp's input wait and the fifth beat; the beats are their own ordered chain; a post-parse judges the grid by the producer's own arithmetic against the root's reported rate; the plane joins the boot-layout gate, the run records, and the closures; the gate-control pin moves from 66 to 107 | The plane is observed by the checker that owns the product graph, and its new markers are mutation-controlled |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The producer drifts, skips, or sends before its deadline | `just sel4_mavlink_graph_check` | `heartbeat N deadline D is not the previous deadline … plus one second`, `was sent at … before its deadline`, `missing beat` |
| The driver serves with no device, or init launches Slisp first | `just sel4_mavlink_graph_check` | a heartbeat status other than `no-device`, or the launch markers out of order |
| The two encoders or the contract's frames drift | `just test_host`, `just contracts_check` | `the_encoder_reproduces_every_pinned_vector`, or the generated bindings stale |
| The register sequence, readbacks, or deadlines regress | `just test_host` | `configuration_writes_the_line_in_order_with_the_latches_under_dlab`, `a_stuck_holding_register_times_out_with_the_bytes_already_written` |
| The wire validators loosen | `just test_host` | `dirty_reserved_and_unused_payload_are_rejected`, `every_status_is_distinct_and_only_named_statuses_are_admitted` |
| The plane's capability layout drifts | `just sel4_boot_layout_check` | `sel4-mavlink` against its frozen fixture |
| A marker is deleted or reordered without notice | `just sel4_gate_control_check` | The pinned count 107 |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `python3 scripts/check/check-sel4-component-graph.py --composition sel4-mavlink --transcript …` (`just sel4_mavlink_graph_check`) | Passed on the final tree, with `heartbeat vectors: 3 pinned frames encoded and decoded` before the boot; kept as [`sel4-mavlink-graph.log`](sel4-mavlink-graph.log) (1154 lines): `SLIME_ROOT generation admitted number=56 executables=8 instances=8`, `spawn authorized task=0 slot=10 component=uart16550-driver` then `slot=11 component=mavlink-heartbeat` then `slot=9 component=slisp`, `SLIME_IO quota task=3 instance=uart16550-driver devices=0 shared_granule=0`, `SLIME_CLOCK authority task=4 instance=mavlink-heartbeat … timers=2 badge=0x200`, `SLIME_GRAPH healthy generation=56 … required=6 live=6 idle=6 failed=0`, `[uart16550-driver] device absent, refusing requests`, `send seq=0` … `seq=5 status=no-device` at deadlines 62 500 000 ticks apart, `=> 2`, `=> spawned sysinfo` | Direct |
| Heartbeat lateness on that boot (`now − deadline`, ticks at 62 500 000 Hz) | 11 889, 668 241, 36 282, 34 110, 36 693: between 0.19 and 10.69 ms. This boot ran beside other gates; an earlier boot on a quiet machine stayed under 1.2 ms. Reported by the gate, not judged | Direct |
| First boot, before the arm was finalized | The same plane, cadence, and health; the arm then failed only on `sysinfo\n\[spawn-service\] request`, because a heartbeat line split the echoed command. That observation is the reason for the result-based markers above | Direct |
| `just sel4_component_graph_check`, `just sel4_pwm_graph_check` | Passed: both other arms' markers unchanged with the generalized init launch | Direct |
| `just sel4_boot_layout_check` | Passed after `just sel4_boot_layout_bless` added `sel4-mavlink` (8 slots): 33 plane layouts match their fixtures, every other fixture unchanged | Direct |
| `just sel4_gate_control_check` | Passed: 45 gates reject 1918 mutated transcripts and layouts, 8 identity cases and 4 runtime cases; the product-graph gate pinned at 107 markers (was 66) | Direct |
| `just test_host` | Passed, with `serial_device` (7), `mavlink_heartbeat` (5), and `uart16550` (8) | Direct |
| `just test_sel4_root` | Passed, 245/245 across 21 modules: no root change | Direct |
| `just contracts_check`, `just component_crate_split_check` (79 crates), `just system_image_closure_check`, `just system_image_builder_check`, `just system_test_run_check` (54 records), `just system_image_closure_aggregate_check` (56 closures) | Passed, after `python3 scripts/generate/generate-system-image-closures.py` and `just system_test_run_bless` for the new plane and the changed component tree | Direct |
| `just generation_check` | Passed: two isolated builds produced byte-identical `generation.bin` and `boot-store.bin` | Direct |
| `just io_driver_authority_check`, `just clock_authority_check`, `just sel4_root_boot_check` | Passed: the declared-device, clock, and root boot paths unchanged | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos`, `just deny`, `just machete`, `just devlog_check`, `just tasks_check` | Passed. `fmt_check_all` first asked for rustfmt layout in the new files, and `lint_all` refused a first init edit that had dropped the non-product shutdown path; both were fixed, and every gate in this table reran on the final tree | Direct |
| Closure and builder checks under the default temporary directory | Failed with `Disk quota exceeded` writing a kernel-loader build under a nearly full `/tmp` tmpfs; rerun green with `TMPDIR` on the repository's filesystem, outside the tree | Direct |
| Root unit tests for `clock RATE_READ` (`rate_follows_the_monotonic_bit`, the label-routing and request-shape tables in `ipc.rs`) | Present before this change and inside the 245 above | Inherited |

## Decisions

- Decision: no root change; the producer uses `clock RATE_READ`, already served under `monotonicRead` with its own root unit tests.
- Rationale: the work item was written when the syscall did not exist; it landed with the tree's source baseline, so IO9's exit clause about it is inherited evidence, not new work.
- Decision: the 16550 register sequence lives in `slime-components` over a register trait, and the driver crate carries only the protocol, the reply discipline, and the line seam.
- Rationale: on every plane in this tree the driver binds nothing, so a sequence in the binary would be code nothing executes or tests; behind a trait it is host-tested against a scripted register model. The stride and access width are part of the platform's line, because 16550-compatible ports differ in both.
- Rejected alternative: build-time environment knobs for the clock and divisor, which closure builds scrub and which would put a board fact in the general build; a `configure` operation in the protocol, which would put line facts in every client.
- Decision: each wait on the line is bounded in time, four character times at the line's rate, not in status reads.
- Rationale: a read count is baud- and bus-dependent: the same count is less than one character at a slow rate and hundreds at a fast one. The line already names its clock and divisor, so the bound follows from them, and the clock is read only while a device is bound.
- Decision: the driver receives blocking.
- Rationale: it has no time-driven duty, unlike the pwm driver's failsafe; a polling receive would spin a processor for nothing. The reply capability's lifetime is the same under either receive, so the answer still precedes the next one.
- Decision: the cadence is a fixed grid (`deadline += period`), never reset by a late beat.
- Rationale: resetting makes each deadline depend on scheduling, which neither a receiver nor a gate can check; a fixed grid is exact arithmetic, and a beat that finds its deadline passed simply goes out at once.
- Decision: the gate judges the grid by deadlines and `now >= deadline`, not by wall-clock intervals.
- Rationale: QEMU runs without instruction counting, so the counter follows the host and an interval bound would fail on a loaded machine. Observed lateness is recorded below as evidence, not gated.
- Decision: in the mavlink arm, Slisp's typed commands are asserted by their results (`=> 2`, the spawn request), not by an uninterrupted echo.
- Rationale: the producer and the root's clock service write to the same console every second; the first boot split the echoed `sysinfo` across a heartbeat line. The plain product arm keeps the uninterrupted-input claim.
- Decision: the serial-device reply carries no operation echo and the protocol defines one operation.
- Rationale: a signed status needs four bytes in the shared codec, and a 16-byte reply has no room for an echo that nothing reads; a status query has no caller on any plane.

## Open risks and follow-ups

- [ ] The line behind the seam: a platform that carries a 16550-compatible port supplies `line::config`, feeds `AuthorityInventory::declared` from its root's carve, and decodes the frames on the far end of the wire. Nothing on QEMU can.
- [ ] The register sequence has never run against silicon; its readback of `LCR` after clearing `DLAB` assumes a port whose latches read back, which the host model assumes too.
- [ ] Polled transmit holds the driver for the frame's wire time, about 3.6 ms at 57 600 baud for 21 bytes; a producer at a much higher rate would want interrupt-driven transmit, which needs an interrupt source in the driver's budget.
- [ ] The existing seL4 `CNode Copy/Mint/Move/Mutate: Source slot invalid or empty` diagnostics appear on this plane as they do on `sel4-pwm` (738 lines there, 861 here); they predate IO9 and no gate treats them as failure.

## Artifacts and provenance

- Raw transcript: [`sel4-mavlink-graph.log`](sel4-mavlink-graph.log), `just sel4_mavlink_graph_check`'s boot with `--transcript`.
- Related work item: IO9 `01a08ac5-14ba-7d23-a751-b06678692de9`.
