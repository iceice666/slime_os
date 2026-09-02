# P6.B — seL4 and `slime-root` on the H1V1

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Verified |
| Scope | `deps/sel4` (`src/plat/ns02201/`, `tools/dts/ns02201-h1v1.dts`, the Cortex-A73 option), `deps/rust-sel4` (`crates/sel4-kernel-loader/src/plat/ns02201/`), `sel4/pins.toml`, `sel4/config/ns02201-h1v1.cmake`, `contracts/target-profile/v1/`, `scripts/build/{build-sel4,build-nt98690-payload,build-generation,build-rpi5-media}.py`, `scripts/lib/{arm64_image,uboot_console,component_sdk}.py`, `scripts/check/{check-nt98690-sel4,check-nt98690-boot,check-sel4-pins,check-sel4-gate-controls,check-architecture-contract}.py`, `slime-root`, `just/{hardware,quality}.just` |
| Roadmap | P6.B |
| Gates | `just sel4_nt98690_image_check`, `just nt98690_sel4_check`, `just sel4_gate_control_check`, `just sel4_pin_check` |
| Trigger | [P6.A](../2026-09-01-p6a-nt98690-probe/index.md) closed with every value a kernel port needs measured on the board, and the roadmap's P6.B unblocked on them |
| Baseline | The pinned seL4 fork had no NT98690 platform and no Cortex-A73 option; the loader fork had no 16550 console; no target profile, platform record, or gate existed for the board; `slime-root` announced its target profile and reset the board only on the Milk-V Duo |

## Summary

The H1V1 gains a seL4 platform in the pinned fork, a console arm in the loader
fork, a target profile, a platform record with its own prefix and pinned hashes,
a builder that wraps the packaged loader ELF in the arm64 `Image` header P6.A
qualified, a root task that resets the board through the SoC watchdog when the
sample plane completes, and a gate that boots the sample-plane image three
times over the same `booti` handoff and requires byte-identical normalized
traces. Every value in the kernel configuration was measured by P6.A rather
than assumed. The board session closed the milestone on 2026-09-02: three boots, three
identical normalized traces, and the root's watchdog reset followed by the
vendor firmware's banner every time.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| seL4 fork | `ns02201-h1v1` platform: a device tree trimmed from the tree the board runs, an overlay placing the kernel's 768 MiB window at `0x10000000`, `KernelArmCortexA73` at the seven sites the A76 occupies, `ns16550a` on the DW-APB serial driver, the mandatory `libsel4` platform include | The kernel for this board is built from the board's own facts, in the fork Slime pins |
| rust-sel4 fork | `crates/sel4-kernel-loader/src/plat/ns02201/mod.rs`: an inline polled 16550 at `0x2f0130000`, PSCI secondaries, `cntvoff` reset under hypervisor | The loader prints through the port the probe proved, without a driver crate that does not exist |
| Pins | `[ns02201_h1v1]` product half, `[observed_prefix_ns02201_h1v1]`, both fork commits bumped; `kernel_config_sha256` re-pinned on the four existing platforms | A fork commit that declares a platform and a CPU option adds two `false` keys to every kernel config; every other pinned artifact stayed byte-identical, and the rule is now written beside the pin |
| Target profile | `aarch64-sel4-nt98690-h1v1`, id 8, admitted by the generation builder, the architecture contract, and the component SDK map | The board's generation is its own exact identity, not the QEMU reference's |
| Host build | Fifth `Platform` in `build-sel4.py`; `sel4/config/ns02201-h1v1.cmake`; a `check-sel4-pins.py` block asserting pins against the CMake config, the probe's measurements, and the fork's own device tree | Three sources that drift independently are checked against each other |
| Image | `build-nt98690-payload.py --sel4`: the arm64 header overwrites the loader's runtime-dead ELF header, `text_offset` is the link base; the ELF flattening helpers moved from `build-rpi5-media.py` into `scripts/lib/arm64_image.py` | One flattener for both AArch64 boards, byte-identical to what the Pi 5 media builder produced before the move |
| Root | `slime_ns02201_h1v1` and `slime_physical_target` cfgs; the READY line naming the target profile compiled for every physical board; two mapped granules and `request_ns02201_watchdog_reset` performing TF-A's own sequence; a fatal condition resets the board once the granules are mapped | A completed proof, or a fatal, returns the board to its firmware on a bench with no other way to reboot it |
| Gate | `check-nt98690-sel4.py`: P6.A's staging sequence, three boots, the sample plane's own transcript assertions, normalization, byte-identity, recovery observed through the next prompt for runs one and two and in silence for the last; pinned at 19 markers in the shared tamper control | The Duo lane's three-boot claim, with the tamper control the Duo's gate never had |
| Reset probe | `check-nt98690-boot.py --reset-probe`: TF-A's five watchdog writes from the U-Boot prompt, 32-bit then 64-bit | Whether the non-secure world may reset this board is observed before root code depends on it |
| Host tests | `just test_sel4_root` cross-compiles for AArch64 and runs under `qemu-aarch64` on non-AArch64 hosts | The gate the repository names for root changes runs on this host, on the architecture the root ships on |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The board's pins drift from its CMake config, the probe's measurements, or the fork's device tree | `check-sel4-pins.py::check_profile` block for `ns02201-h1v1` | `just sel4_pin_check` fails naming the disagreeing value |
| A marker is deleted, reordered, or a failure marker tolerated | `check-sel4-gate-controls.py` entry `nt98690_sel4` pinned at 19 | `just sel4_gate_control_check` fails |
| The flattener move changed the Pi 5 image | The moved helpers reproduce the pre-move bytes of `build/slime-sel4.elf` flattened (1,103,992 bytes) | byte inequality |
| The wrapped image's header disagrees with the loader | `check_image_header` plus the landing-word check on `mrs x0, mpidr_el1` | `build-nt98690-payload.py --sel4` fails |
| A fork bump moves hashes it should not | Only `kernel_config_sha256` moved on every existing platform, reproduced exactly by removing the two new keys | any other pinned artifact moving is a finding, per the pins' own comment |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_root_boot_check`, `just sel4_sample_check`, `just test_sel4_root` on the untouched tree | Root and sample planes passed; the host tests could not compile on this x86_64 host (libsel4's AArch64 inline assembly under a host-target clang), which the amended recipe fixes | Direct |
| `just sel4_pin_check` | Passed with both fork commits, the new platform block, and the re-pinned config hashes | Direct |
| `python3 scripts/build/build-sel4.py --platform ns02201-h1v1 [--sample-plane]` | Kernel at physBase `0x10000000`, memory `0x10000000..0x40000000`, `ARM_CORTEX_A73`, `ARM_PA_SIZE_BITS_40`, `ARM_HYPERVISOR_SUPPORT`, `TIMER_FREQUENCY 12000000`; loader linked at `0x10286000`; root, generation, and both images built without warnings | Direct |
| `python3 scripts/build/build-nt98690-payload.py --sel4` | 1,285,200-byte image, `text_offset 0x10286000`, `code0` branching to `0x1028a5d8` which holds `mrs x0, mpidr_el1`; the probe path unchanged (sha `1046ed43…`) | Direct |
| The flattener move | Byte-identical output on `build/slime-sel4.elf` before and after | Direct |
| `just test_sel4_root` (amended recipe, `qemu-aarch64`) | 214 passed, 0 failed | Direct |
| `just sel4_gate_control_check` | 47 gates reject 1822 mutated transcripts; every marker of the new gate instantiates | Direct |
| Rehearsal of `check-nt98690-sel4.py` against a stand-in board replaying the QEMU sample-plane transcript with per-run jitter | Three runs: contract, sample-plane assertions, and wire clean; normalized traces identical; each run's recovery evidence is exactly the firmware banner line; no `Moving Image` in any transcript. Not board evidence: a rehearsal of the loop | Direct, synthetic |
| `just sel4_root_boot_check`, `just sel4_sample_check`, `just generation_check` over the re-pinned tree | Passed: the QEMU reference kernel is byte-identical, only its config JSON gained the two keys, and every reference plane still boots | Direct |
| `just contracts_check`, `just ruff`, `just typos`, `just fmt_check_all`, `just lint_all`, `just devlog_check` | Passed on the committed tree | Direct |
| `--reset-probe` on the named board | Not run as a separate step: the operator went straight to the gate, whose three runs each end with the root's `reset request kind=wdt` followed by the firmware banner -- the reset from the non-secure world, observed three times from EL0 through seL4 rather than once from U-Boot | Direct; the three transcripts |
| `just nt98690_sel4_check /dev/ttyUSB0` on the named board, 2026-09-02 | **Passed.** Each run: `fatload` read 1,285,200 bytes in 66 ms; the loader saw memory `0x10000000..0x40000000` and copied the kernel to `0x10000000`; `Booting all finished, dropped to user space`; `SLIME_ROOT allocator slots=3047 untypeds=26 bytes=798838800`; `SLIME_TIMER acquired irq=30 freq_hz=12000000`, delivered and serviced before and after graph activation; the generation admitted with `executables=4 instances=4 grants=6`; `SLIME_ROOT READY target_profile=aarch64-sel4-nt98690-h1v1`; `SLIME_NT98690 reset request kind=wdt`; `U-Boot 2021.10`. Raw transcripts 120,204 bytes each, normalized traces 9,448 bytes each and byte-identical; the operator's normalized files equal this host's re-normalization of the raw logs; 0 framing errors on a local tty; no `Moving Image` in any run | Direct; [`sample-run-1.log`](sample-run-1.log), [`sample-run-2.log`](sample-run-2.log), [`sample-run-3.log`](sample-run-3.log), and their `.normalized.log` siblings |
| Re-verification of the operator's evidence on this host | The gate's own contract, the sample plane's `check_transcript`, and `normalize` re-run over the three raw logs: all pass, and reproduce the operator's normalized files exactly | Direct |

## Decisions

- **The root resets the board itself, in P6.B.** The Duo lane's three-boot gate is autonomous because its root drives the RTC; the H1V1's analogue is the watchdog sequence its own TF-A performs, and the roadmap's "recovers autonomously after each" was written for exactly that. Pulling it forward from P6.C costs two mapped granules and thirty lines; leaving it would have cost the operator three power cycles per attempt on a board whose Linux has no console to reboot from. The non-secure world's ability to write those registers is a fact the probe establishes first.
- **The seL4 platform is named `ns02201-h1v1`** everywhere, not `ns02201` as the S1 plan said: the fork's own precedent names the platform for the board (`cv1800b-duo`), the device tree is board-specific, and `libsel4/sel4_plat_include/<name>` must match the `declare_platform` name, so one name removes a mapping.
- **The device tree is trimmed, not the vendor dump.** The tree the board runs is 13,389 lines of camera pipeline, nine UARTs, and PCIe with three-cell child addresses; the kernel needs eight nodes. Every kept value is verbatim and cites its source; nothing the hardware generator would have had to survive is left in.
- **The arm64 header overwrites the ELF header.** The loader links with its ELF header in the lowest PT_LOAD, dead once loaded, and sixty-four bytes long -- the header's own size. `text_offset` is therefore the link base and `booti`'s `ALIGN(0, 2 MiB) + text_offset` places the image where it was linked, with no alignment requirement on the value, which the vendor's `booti_setup` shows and the vendor kernel's own relocation confirmed.
- **The gate is its own checker.** The shared tamper control owns one marker contract per module and pins its count; the P6.A probe's is pinned at 25. The seL4 gate borrows the console, the staging sequence, the banner recovery, and the sample plane's assertions, and owns only its contract and its loop.
- **No early-fault boot and no physical wrong-target boot.** The Duo's early-fault control is RTC-specific (`CNTP_CVAL` accepts any deadline), and P6.B's roadmap text does not require one; cross-profile rejection is exercised on the host by `check-architecture-contract.py` for the new profile pair.
- **Fork pushes are the operator's.** This host's GitHub identity has no push access to the forks (no pending invitation, `repo` scope present, permission lookup 403). Both branches are `slime-ns02201-h1v1`, stacked on the Duo commits, committed locally and pinned by hash.

## Open risks and follow-ups

- [x] **The board booted seL4 three times**; the transcripts are committed here.
- [x] The watchdog reset from the non-secure world works with 32-bit writes from EL0 through seL4: every run's reset request was followed by the firmware banner. A transcript cannot show whether an operator also cycled power between runs; the operator's account is recorded under Corrections once given. The separate `--reset-probe` was not needed and remains available as a bench tool.
- [x] `check-sel4-sample-plane.py::check_transcript` accepted the board's own transcripts, on the board's own architecture.
- [ ] The fork branches are local until pushed; the pins name commits `1b93edf5…` and `20905bef…`.
- [ ] The root's virtio scan at `0x0a00_0000` maps a RAM page as device memory on this board and reports `devices=0`; harmless and stable, left as is.

## Artifacts and provenance

- Fork commits: seL4 `1b93edf532c5b53e607b4f4f9fa226f78a63ec13`, rust-sel4 `20905bef6b93b7d863ac6dc5d7053f96aa09765a`, both on `slime-ns02201-h1v1`.
- Prefix hashes: `sel4/pins.toml [observed_prefix_ns02201_h1v1]`.
- Board evidence, received from the operator by wormhole and re-verified here: [`sample-run-1.log`](sample-run-1.log) / [`sample-run-1.normalized.log`](sample-run-1.normalized.log), [`sample-run-2.log`](sample-run-2.log) / [`sample-run-2.normalized.log`](sample-run-2.normalized.log), [`sample-run-3.log`](sample-run-3.log) / [`sample-run-3.normalized.log`](sample-run-3.normalized.log).
- Plan of record: [`../2026-09-01-p6-nt98690-h1v1-lane/plan.md`](../2026-09-01-p6-nt98690-h1v1-lane/plan.md), Part C.
- Related roadmap item: [P6.B](../../roadmap/07-architecture-portability.md#p6b--sel4-and-slime-root-on-the-h1v1).
