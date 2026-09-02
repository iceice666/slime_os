# NT98690 (Novatek NS02201) H1V1 lane — meta plan + Session 2 plan (P6.B)

Three parts. **Part A** is the durable meta plan for all three sessions, now carrying the
board's *measured* facts and the corrections Session 1 and this session's exploration made
to it. **Part B** records Session 1's outcome in a few lines and points at its evidence.
**Part C** is the executable plan for Session 2: seL4 and `slime-root` booting on the H1V1.

Session 2's first task copies the updated Part A over the repository's
`devlog/2026-09-01-p6-nt98690-h1v1-lane/plan.md` (under `## Corrections`, never editing
the frozen body) so it survives independently of this file.

---

# Part A — Meta plan (durable)

## A1. Goal and exit conditions

Bring Slime OS to the Novatek NT98690 (= NS02201) **H1V1** camera board as a third physical
lane beside the Milk-V Duo (observed) and the Pi 5 (unobserved). Final exit (end of S3): the
H1V1 boots loader → seL4 → `slime-root` from an SD card through the **unmodified vendor
U-Boot**, never writing eMMC, and the resident Slisp shell answers typed UART0 input — every
claim backed by a fail-closed serial gate and a devlog entry, in the Duo lane's shape.

| Session | Milestone | Exit condition (observed) | Gates |
|---|---|---|---|
| S1 | **P6.A** environment + firmware-handoff probe | **Done 2026-09-02.** Probe placed by `booti` at `0x10000000`, reported EL2/A73/40-bit PA/12 MHz/GIC-400 352 lines, `PAYLOAD_OK`, PSCI reset returned the vendor banner unattended; 25 markers, 0 framing errors | `nt98690_payload_check`, `nt98690_boot_check`, `sel4_gate_control_check` |
| S2 | **P6.B** seL4 + `slime-root` | Same handoff boots the sample-plane seL4 image **three times** with byte-identical normalized traces; ordered `SLIME_ROOT`/`SLIME_TIMER`/`READY target_profile=aarch64-sel4-nt98690-h1v1`; the root resets the board through the SoC watchdog after each boot, so runs 2 and 3 need no operator | `sel4_nt98690_image_check`, `nt98690_sel4_check`, `sel4_gate_control_check` |
| S3 | **P6.C** interactive Slisp over UART0 | Product graph boots; `slisp> ` prompt; three typed commands answered; test-terminator byte triggers the same watchdog reset | `nt98690_slisp_check` |

Out of scope until P6.C closes: storage, network, display, generation management on this
board; any verified-kernel claim (this SoC is not in seL4's verified set).

## A2. Board / firmware facts — **now observed, not inferred**

Sources for the vendor side stay as before (`/srv/novatek/sdk/worktrees/h1v1-dev/`, TF-A
`plat/novatek/nvt_ns02201/`, U-Boot `nvt-ns02201_a64_pci_emmc_defconfig`). Every row marked
**observed** was read off the board on 2026-09-02; evidence in
`devlog/2026-09-01-p6a-nt98690-probe/{probe-boot,uboot-survey}.log`.

| Fact | Value | Status |
|---|---|---|
| Entry EL, MMU, caches | **EL2**, `sctlr = 0x30c50830` (M=0, C=0, I=0), `hcr_el2 = 0x2`, `cnthctl = 0x13`, `cntvoff = 0` | observed |
| Registers at entry | x0 = **relocated FDT** (`0x3ffc3000`, magic `d00dfeed`, size `0x3b000`), x1=x2=x3=0 | observed |
| CPU | `midr 0x411fd090` → ARM Cortex-**A73** r1p0; `mpidr 0x80000000` (core 0) | observed |
| PA range | `ID_AA64MMFR0.PARange = 2` → **40-bit** | observed |
| GIC | `ID_AA64PFR0.GIC = 0` → **GICv2, memory-mapped only**; `gicd_typer 0xfc6a` → **352 IRQs**, 4 CPU IFs, security ext; `gicd_iidr 0x0200143b` → ARM **GIC-400**; `gicd_ctlr = 0` at handoff. Banks: GICD `0x2_fff01000`, GICC `0x2_fff02000`, GICH `0x2_fff04000`, GICV `0x2_fff06000` (vendor tree, node `interrupt-controller@2,fff00000`, `arm,cortex-a7-gic`, maintenance `<1 9 0xf04>`) | observed |
| Timer | **`CNTFRQ_EL0 = 12 000 000` on the primary core**, corroborated by a line-rate estimate to 0.33%. The tree's `clock-frequency = <0xb71b00>` agrees. A7 risk 1 is **closed the other way**: no pinned override needed | observed |
| UART0 | `ns16550a` @ `0x2_f0130000` size `0x1000`, `reg-shift 2`, `reg-io-width 4`, SPI 43 level-high, 48 MHz clock, 115200 8N1, initialised by BL31; THR `+0x00`, LSR `+0x14` (THRE bit 5, DR bit 0) | vendor tree + probe used it |
| DRAM | 2 GiB at `0x0`; vendor `/memory` = 1 GiB; U-Boot `lmb`: memory `0x0..0x7fffffff`, reserved `0x1f00000-0x1ffffff` (BL31), `0x4800000-0xabfffff` (CMA), `0x7f9c4a40-0x7fffffff` (U-Boot+FDT+stack). U-Boot relocates itself to `0x7ff75000`, its FDT to `0x3ffc3000` for `booti`, vendor kernel to `0x0..0x1220000` | observed |
| `booti` placement | `relocated = ALIGN(ram_base=0, 2M) + text_offset` = **`text_offset`**; **no alignment requirement on `text_offset` itself** (`arch/arm/lib/image.c:70-80`). Vendor kernel: load `0x7c700040`, `text_offset 0` → moved to `0x0` — the model is confirmed by the vendor's own boot. Prints `Moving Image from` only when relocated ≠ load | observed |
| `${fdtcontroladdr}` | **`0x7f9c5ea0`**, holds `d00dfeed` (neither the loader's `0x100000` nor the `0x7fc741c0` the vendor `booti` prints) | observed |
| SD / eMMC | `mmc list`: **MMC0 = SD**, MMC1 empty, MMC2 = eMMC. `mmc dev 0` → `mmc0 is current device` (eMMC answers `mmc2(part 0) is current device`) | observed |
| Prompt / autoboot | `nvt: `, `bootdelay=0`, `bootcmd=nvt_boot`, `Hit any key to stop autoboot:  0` is printed; CR spam at 50 ms from before power-on wins the race | observed |
| Banner | `U-Boot 2021.10 (Jul 15 2026 - 11:20:07 +0800)`; TF-A `BL31: v2.2(release)` built Jul 7 2026; loader `LD_VER 01.09.04` | observed |
| Vendor Linux console | **none** — after `Starting kernel ...` the UART is silent; no `login:`, no shell → **no scriptable reboot**; every fresh boot needs a power cycle *unless the payload resets the board itself* | observed |
| PSCI | 0.2 via SMC; `SYSTEM_RESET` (SMC32 `0x84000009`) works from EL2 and returns the loader/TF-A/U-Boot chain unattended | observed |
| **SoC reset from non-secure world** (TF-A's own `nova_system_reset`, `RTC_PWBC_RESET 0` → watchdog branch) | `CG_BASE 0x2_f0020000`, `WDT_BASE 0x2_f0060000`. Sequence: `CG+0x9C |= 1<<4` (wdt clk reset release), `CG+0x400 |= 1<<24` (wdt clk enable), `WDT+0x0 = 0x5a960112`, `WDT+0x0 = 0x5a960113`, `WDT+0xC = 1` (manual reset). TF-A uses `mmio_write_64`. Vendor tree: `cg@2,f0020000 { compatible = "nvt,core_clk"; reg = <2 0xf0020000 0 0x10000>; }` | **vendor source; unobserved from NS world — C1 proves it** |
| Board switch | SW18 = `0x1001`; `0x0001` = loader rescue, **never** | vendor scripts |

## A3. Fork / upstream facts (seL4 `iceice666/seL4` @ `f25b760`, branch `slime-cv1800b-duo`; rust-sel4 `iceice666/rust-sel4` @ `070c6a3`, same branch name) — **corrected**

- `tools/hardware.yml`: `ns16550a` in the serial-console class (L197, `UART_PPTR`, 4 KiB); `arm,cortex-a7-gic` in GICv2 class (L16); `arm,armv8-timer` (L124); `arm,psci-0.2` (L228).
- `src/drivers/serial/config.cmake:11` registers `nvidia,tegra20-uart;ti,omap3-uart;snps,dw-apb-uart` → `tegra_omap3_dwapb.c` (THR `0x0`, LSR `0x14`, THRE bit 5, `volatile uint32_t*`, **no init**). **`ns16550a` is not in that list** → one-line add.
- **No `KernelArmCortexA73` exists anywhere.** Adding one touches **seven sites**, not three: `src/arch/arm/config.cmake` PA-bits chain (L11-38; A73 → `KernelArmPASizeBits40`, `KernelPaddrUserTop 1<<40`, which also selects `KernelAarch64VspaceS2StartL1` under hyp at L88-92, same as the A76 Pi 5), hypervisor `DEPENDS` (**L78-84**, not 109), cache-line list (**L238-244**, not 142-144); `configs/seL4Config.cmake` unset/default list (L135-153), `config_set(KernelArmCortexA73 ARM_CORTEX_A73 …)` (L189-198), `KernelArmCPU "cortex-a73"` chain (L219-239); and `libsel4/arch_include/arm/sel4/arch/constants_cortex_a73.h` (copy of `_a76.h`: 6 breakpoints + 4 watchpoints, `#error` guard on `CONFIG_ARM_CORTEX_A73`).
- **Mandatory per-platform dir**: `libsel4/sel4_plat_include/<KernelPlatform>/sel4/plat/api/constants.h` (referenced from `CMakeLists.txt:328,621`, `libsel4/CMakeLists.txt:89`) — the name must equal the `declare_platform` *name*.
- Platform precedent `src/plat/bcm2712/config.cmake` (upstream): `declare_platform(bcm2712 KernelPlatformRpi5 PLAT_BCM2712 KernelArchARM)`, `declare_seL4_arch(aarch64)`, `KernelArmCortexA76 ON`, `KernelArchArmV8a ON`, `config_set(KernelARMPlatform ARM_PLAT rpi5)`, `+crc`, two `KernelDTSList` appends, `declare_default_headers(TIMER_FREQUENCY 54000000 MAX_IRQ 320 NUM_PPI 32 TIMER drivers/timer/arm_generic.h INTERRUPT_CONTROLLER arch/machine/gic_v2.h KERNEL_WCET 10u)`, `add_sources(… gic_v2.c l2c_nop.c)`. `overlay-rpi5.dts` = `chosen { seL4,elfloader-devices; seL4,kernel-devices }` + vGIC maintenance IRQ + `/delete-node/ linux,cma`. **It contains no `seL4,boot-cpu`** (that was wrong in the S1 plan). Memory comes from `overlay-rpi5-2gb.dts`.
- Fork precedent `9a7d89bdd feat(cv1800b): add Milk-V Duo platform` = 5 files: `src/plat/cv1800b/{config.cmake, overlay-cv1800b-duo.dts}`, `tools/dts/cv1800b-duo.dts` (1005-line vendor dump), `libsel4/sel4_plat_include/cv1800b-duo/…/constants.h`, one PLIC guard line. Its overlay **replaces the memory node** (`/delete-node/ memory@80000000;` + `memory@80040000`) — the shape ns02201 needs.
- **physBase on aarch64 = base of the lowest surviving `memory` node, unaligned** (`tools/hardware/utils/memory.py:85-98`, `config.py` `KERNEL_PHYS_ALIGN` 0 for aarch64). No CMake knob. So the overlay's `memory@10000000` *is* what puts the kernel at `0x10000000`.
- Device untypeds = the inverse of (memory ∪ kernel devices) over `0..addrspace_max` (`get_addrspace_exclude`) — MMIO need not be listed in the DTS to be a user device untyped; listing documents it.
- rust-sel4 loader: plat arms in `crates/sel4-kernel-loader/src/plat/mod.rs` (`sel4_cfg_if!` on `PLAT_*`, plus a `#[cfg(false)] mod x;` rustfmt hack); `bcm2712/mod.rs` (60 lines) is the AArch64 template; **no 16550 driver crate** → inline `put_char`; fork precedent `1ef65fb1` = **2 files, 65 lines**. aarch64 `enter_kernel` asserts `CurrentEL == EL2` unconditionally. **Loader link base = `round_up(kernel_phys_end + 256 KiB, 4 KiB)`** from the installed `kernel.elf` (`build.rs:53-63`, `--image-base`); not configurable. Identity map covers `0..2^39` as device memory → MMIO above 4 GiB needs nothing. **The kernel's DTB is embedded at build time** (`{prefix}/support/kernel.dtb` → payload region below the root image at the top of the last memory range); U-Boot's x0 is ignored.
- Pin enforcement (`check-sel4-pins.py::check_submodules`): submodule HEAD == pinned commit, `origin` URL == pinned repository, **no uncommitted changes**; `deps/sel4/VERSION == [sel4].release`; `rust-toolchain.toml` pins `[rust_sel4].toolchain`. Local commits satisfy it; reproducibility elsewhere needs them pushed.

## A4. Fixed decisions and names — **one deviation from S1**

| Decision | Value / rationale |
|---|---|
| **seL4 platform name** | **`ns02201-h1v1`** everywhere (`declare_platform(ns02201-h1v1 KernelPlatformNS02201H1V1 PLAT_NS02201_H1V1 KernelArchARM)`, `KernelPlatform "ns02201-h1v1"`, `libsel4/sel4_plat_include/ns02201-h1v1/`, `src/plat/ns02201/` directory). *Deviation from S1's `ns02201`*: the fork's own precedent names the platform for the board (`cv1800b-duo`), the DTS is board-specific, and one name for the seL4 platform, the Slime platform (`sel4/config/ns02201-h1v1.cmake`, pins `[ns02201_h1v1]`, build dirs), and the loader's `PLAT_NS02201_H1V1` removes a mapping. |
| Target profile | `aarch64-sel4-nt98690-h1v1` (id 8, arch 2, abi 4, page 2, features 66, elfMachine 183, `aarch64-sel4-minimal.json`, `qemuBinary ""`) |
| Marker prefix | `SLIME_NT98690` for board-specific root lines (reset request), `SLIME_ROOT`/`SLIME_TIMER`/`SLIME_GRAPH` unchanged |
| Kernel memory window | `memory@10000000 { reg = <0 0x10000000 0 0x30000000> }` (768 MiB): above every vendor reservation, below the FDT relocation zone and U-Boot, inside the 1 GiB the vendor tree declares. Widen only from observed BootInfo. Kernel physBase `0x10000000`; loader at `round_up(kernel_end + 256K, 4K)`; root image + DTB at the top (~`0x3fxxxxxx`, clobbering U-Boot's dead FDT copy, which nothing reads after `booti`). |
| Boot image format | Loader ELF flattened by PT_LOAD walk; the **arm64 `Image` header overwrites the first 64 bytes**, which are the runtime-dead ELF header (the lowest PT_LOAD has `p_offset 0`; ELF64 header is exactly 64 bytes). `text_offset = image base = fatload address`, `code0 = b entry`, `image_size = flat length`. 4 KiB alignment suffices (A2 `booti` row). |
| Kernel config | `KernelArmHypervisorSupport ON` (loader asserts EL2; board enters at EL2), `KernelIsMCS OFF`, `KernelMaxNumNodes 1`, `KernelVerificationBuild OFF`, `KernelDebugBuild ON`, `KernelPrinting ON`, `KernelArmExportPCNTUser/PTMRUser ON`; `TIMER_FREQUENCY 12000000`, `MAX_IRQ 352`, `NUM_PPI 32`, `gic_v2.h`, `arm_generic.h`, no `KernelArmMachFeatureModifiers` |
| Kernel console | `tegra_omap3_dwapb.c` via `ns16550a` in its compatible list; firmware-initialised; **the only output path for `slime-root`** (`seL4_DebugPutChar`) |
| Root timer | unchanged: `TIMER_IRQ = 30` (CNTP, non-secure EL1 physical, under hyp), `frequency_hz()` reads `CNTFRQ_EL0` — no override |
| **Autonomous reset (moved from S3 into S2)** | Root drives the SoC watchdog sequence TF-A uses (A2 last row) through two mapped device granules (CG page, WDT page), mirroring `request_cv1800b_cold_reset`. Proven first from U-Boot (C1). Fallback if non-secure writes cannot reset: operator power cycle per boot, Pi 5 convention (printed instruction + wall-clock window), and P6.B's roadmap wording amended. |
| Seed DTS | **Hand-trimmed** `tools/dts/ns02201-h1v1.dts` (~150 lines: root, cpus, psci, timer, gic, uart0, cg, wdt, memory, reserved-memory/atf), not the 13,389-line vendor dump (PCIe `ranges`, nine UARTs, vendor string-typed `clock-frequency` are not what seL4's hardware tool should have to survive). Node names without commas. Every node cites its vendor source. |
| Gate shape | New `scripts/check/check-nt98690-sel4.py`: a **distinct marker contract** the shared tamper control must own as its own `GATES` entry (a module exposes one contract; `nt98690_boot` is pinned at 25). It imports `uboot_console`, `sel4_gate_markers`, and `load_script`s `check-nt98690-boot.py` (staging helpers) and `check-sel4-sample-plane.py` (`check_transcript`), duplicating nothing. Duo precedent + the improvement the Duo lacks (tamper control). |
| Fork branches | `slime-ns02201-h1v1` in both submodules, stacked on the Duo commits. Pins bump to the new heads. |

## A5. Reuse map (verified paths)

Console/U-Boot: `scripts/lib/uboot_console.py` (`Console`, `reach_uboot`, `send_command`, `report_transcript`); staging: `check-nt98690-boot.py` (`load_profile`, `read_words`, `check_deployed_bytes`, `BANNER_PATTERN`, banner-line truncation at the recovery window). Markers: `scripts/lib/sel4_gate_markers.py` (`chains_from_gate`, `match_marker_contract`); tamper: `check-sel4-gate-controls.py::GATES` + `literal_for` grammar. Three-boot precedent: `check-duo-sel4.py` (`COMMON_BOOT_MARKERS`/`BOOT_MARKERS`, `FAILURE_MARKERS`, `DYNAMIC_FIELDS`, `normalize`, `capture_boot`, byte-identity check; `sample.check_transcript`). Flattening: `build-rpi5-media.py` (`read_load_segments`, `flatten`, `encode_branch`, `check_entry_is_code`, `MRS_MPIDR_EL1 = 0xD53800A0`), `build-duo-payload.py::build_sel4` (identity chain: image identity `sha256` → payload identity `elf_sha256`, `target_profile` check). Header: `scripts/lib/arm64_image.py::pack_header` (no caller yet). Host build: `build-sel4.py` (`Platform` 14 fields, `PLATFORMS`, `configure_and_install_sel4`, `build_application`, `build_loader`, `package_image`, `write_manifest` board branch needing `platform/soc/serial/serial_baud/boot_files`). Pins: `check-sel4-pins.py` (`CONFIG_PATHS`, `PREFIX_PATHS`, `rebuild`, `expected_cmake_values`, `check_profile` cv1800b block incl. DTS fact assertions). Profiles: `contracts/target-profile/v1/schema.zt`, `just boot_gen`, `check-architecture-contract.py::EXPECTED_PROFILES`, `build-generation.py::SEL4_TARGET_PROFILES/prefix_by_profile`, `generation_fabric.py::SEL4_TARGET_PROFILES`, `scripts/lib/component_sdk.py::PROFILE_PLATFORMS`. Root: `build.rs` cfgs, `platform_timer.rs::request_cv1800b_cold_reset`, `main.rs` reset granule mapping (782-795, statics 441-445, call site 1049-1050, `request_duo_cold_reset` 348-359), `graph_runtime/services.rs:1460` READY marker cfg. Recipes: `just/hardware.just` Duo block; `just/product.just::sel4_root_boot_check`; `just/planes-mechanism.just::sel4_sample_check`.

## A6. Observations → S2 consequences (all now measured)

| Observation | Value | Consequence |
|---|---|---|
| `el` | 2 | `KernelArmHypervisorSupport ON`; loader's EL2 assert satisfied |
| placement | exact, no `Moving Image` | header scheme carries the seL4 image unchanged |
| `midr_part`, `parange` | `0xd09`, 2 | `KernelArmCortexA73` → 40-bit PA, S2-starts-at-L1 vspace |
| `cntfrq` vs estimate | 12 000 000 vs 11 960 950 | `TIMER_FREQUENCY 12000000`; root reads the register |
| `gicd_typer` | `0xfc6a` | `MAX_IRQ 352`; `gic_v2.h`; GICD read above 4 GiB works |
| `pfr0.GIC` | 0 | no GICv3 sysreg path; memory-mapped GICv2 only |
| banner after PSCI reset | returns unattended | recovery detection = banner on serial (no network needed) |
| vendor console after boot | silent | no scriptable reboot → root-driven reset is what makes three boots autonomous |

## A7. Risks carried forward

1. ~~Primary `CNTFRQ_EL0` unset~~ — **closed** (12 MHz observed).
2. `text_offset` drift silently moves the image — gate treats `Moving Image from` as failure; the loader base is read from the built ELF, never hard-coded.
3. **Non-secure watchdog write may be blocked** (TZPC) — C1 proves or refutes before any root code depends on it; fallback documented.
4. ~~U-Boot strings~~ — **closed** (all observed).
5. No `echo`/`fdt`/`go`: prompt-as-sentinel only — unchanged.
6. ~~Remote UART latency~~ — moot, the operator runs the gate on the board's host with a local tty.
7. ~~Nix~~ — **closed**.
8. Root virtio scan at `0x0a00_0000` maps a RAM page as device memory and finds nothing → one stable `devices=0` line; harmless, left as is.
9. **Fork push rights**: `gh` on this host is `CG-AA`, which has **no push access** to `iceice666/{seL4,rust-sel4}` (verified 2026-09-02: no pending invitation, `repo` scope present, permission lookup 403). Commits land locally; pushing is the operator's step unless `CG-AA` is added as a collaborator.
10. Every marker regex must be `literal_for`-instantiable; runtime-varying values are normalized, not asserted.
11. `check-sel4-sample-plane.py::check_transcript` must accept a board transcript — the Duo proved it does for RV64; verify on the first aarch64 run rather than assume.

## A8. Session 3 outline (revised)

**S3 — P6.C**: generalise `slime_duo_uart`/`SLIME_DUO_UART_PADDR` (+ the three profile-equality panics in `slime-root/build.rs`) to board-neutral names; reuse `DwApbInput` for UART0 RX (register-identical: `0x00`/`0x14`, 32-bit); product graph image for the profile; test-terminator (`0x1d`) → the S2 watchdog reset (already built); `check-nt98690-slisp.py`, `just nt98690_slisp_check`; devlog; roadmap P6 status + README. The reset mechanism is **no longer S3 work**.

---

# Part B — Session 1 outcome (P6.A, complete)

Landed on `main` as `b12533f…33af04a` (2026-09-01/02). Probe, builder, console library, gate
(25 markers, in `GATES`), pins, roadmap P6, two devlog entries. Board session found and fixed
two gate defects before the scored run (device-tree pre-flight asserted nothing; recovery
window swallowed the vendor's next `Moving Image from`), then **passed on the named H1V1**.
Evidence and the observed facts: `devlog/2026-09-01-p6a-nt98690-probe/`. Roadmap P6.A:
Complete. Memory: `~/.claude/projects/-space-slime-os/memory/nt98690-h1v1-lane.md`.

---

# Part C — Session 2 plan (P6.B: seL4 and `slime-root` on the H1V1)

## Context

P6.A qualified the firmware handoff and measured every value a kernel port needs. P6.B
builds on it: the fork gains an `ns02201-h1v1` platform, the loader gains a 16550 console arm,
Slime gains the target profile and platform record, the P6.A builder learns to wrap the
loader ELF in the header it already qualified, and a new gate boots the sample-plane image
three times over the same `booti` handoff and requires byte-identical normalized traces.
The root resets the board through the SoC watchdog after each boot — the Duo's shape —
so the operator power-cycles once. Roadmap P6.B is already unblocked with these facts.

Decisions taken with the user on 2026-09-02: **root-driven watchdog reset in P6.B** (pulled
forward from P6.C); **fork commits on new `slime-ns02201-h1v1` branches** — pushed by the
operator, since this host's `gh` identity cannot push (A7.9); swap to "I push" if `CG-AA`
is added as a collaborator. Scoped **out**: an early-fault control boot (the Duo's is
RTC-specific — `CNTP_CVAL` accepts any deadline, and P6.B's roadmap text does not require
one); a physical wrong-target boot (`check-architecture-contract.py::check_cross_profile_rejection`
already exercises the new profile pair on the host, which is what "fail explicitly rather than
guessed" asks for); UART RX (P6.C).

## C0. First tasks

1. Append the Part A corrections to `devlog/2026-09-01-p6-nt98690-h1v1-lane/index.md` under
   `## Corrections` and replace `plan.md` with this file's Part A (the plan is a living
   sibling, the entry body is frozen). Update the memory file's status line.
2. `git -C deps/sel4 checkout -b slime-ns02201-h1v1 f25b760`;
   `git -C deps/rust-sel4 checkout -b slime-ns02201-h1v1 070c6a3`. Slime branch
   `p6b-sel4-nt98690-h1v1` from `main` (`33af04a`).
3. Reference profile green before touching the root: `just sel4_root_boot_check`,
   `just sel4_sample_check`, `just test_sel4_root` (211).

## C1. Board session 0 — prove the watchdog reset from non-secure world (one power cycle)

Extend `check-nt98690-boot.py` with `--reset-probe` (read-only except the five writes TF-A
itself performs): `reach_uboot`; `md.l 0x2f002009c 1` → OR `1<<4` → `mw.l`; `md.l 0x2f0020400 1`
→ OR `1<<24` → `mw.l`; `mw.l 0x2f0060000 0x5a960112`; `mw.l 0x2f0060000 0x5a960113`;
`mw.l 0x2f006000c 1`; then the existing silent banner window. If no banner within 30 s, retry
the same with `.q` (TF-A writes 64-bit). Print which width reset the board; write the transcript.
Bundle (`wormhole`) as in S1; the operator runs it and returns `reset-probe.log`.

Outcome pins `[ns02201_h1v1].reset_write_width` (32 or 64) and decides C6.3's helper width.
If neither resets: A4's fallback (manual cycles; `--manual-reset` mode in C5; roadmap wording).

## C2. seL4 fork — one commit, `9a7d89bdd`-shaped

Files (all under `deps/sel4/`):

1. **`tools/dts/ns02201-h1v1.dts`** — trimmed from `dtc -I dtb -O dts` of
   `/srv/novatek/sdk/worktrees/lamb-h1v1/output/nvt-evb.bin` (237,373 bytes; the tree the board
   runs — `bdinfo` `fdt_size 0x39f40`). Keep: root props (`model`, `compatible = "novatek,ns02201"`,
   `#address-cells = <2>`, `#size-cells = <2>`, `interrupt-parent`), `cpus` (4× `arm,cortex-a73`,
   `enable-method = "psci"`, `reg = <0 N>`), `psci { arm,psci-0.2; smc }`, `timer { arm,armv8-timer;
   interrupts = <1 13 0x308 1 14 0x308 1 11 0x308 1 10 0x308>; clock-frequency = <12000000>; always-on }`,
   `interrupt-controller@2fff01000` (four banks as A2, `interrupts = <1 9 0xf04>`, `phandle`),
   `uart@2f0130000` (ns16550a; `reg = <2 0xf0130000 0 0x1000>`; `interrupts = <0 43 4>`; `reg-shift`;
   `reg-io-width`; `clock-frequency = <48000000>`), `cg@2f0020000` and `wdt@2f0060000`
   (`reg` only, documented as the reset path; user device untypeds either way), `memory@0`
   (vendor 1 GiB, replaced by the overlay), `reserved-memory { atf@1f00000 { reg; no-map } }`.
   Header comment: source path, what was dropped and why, P6.A observation cross-references.
2. **`src/plat/ns02201/config.cmake`** — A4 kernel-config row, bcm2712 shape, every number
   with its P6.A citation in a comment. DTS list: `tools/dts/ns02201-h1v1.dts` then
   `${CMAKE_CURRENT_LIST_DIR}/overlay-ns02201-h1v1.dts`.
3. **`src/plat/ns02201/overlay-ns02201-h1v1.dts`** — `chosen { seL4,elfloader-devices =
   &{/uart@2f0130000}, &{/psci}, &{/timer}; seL4,kernel-devices = &{/uart@2f0130000},
   &{/interrupt-controller@2fff01000}, &{/timer}; }`; `/delete-node/ memory@0;`;
   `memory@10000000 { device_type = "memory"; reg = <0 0x10000000 0 0x30000000>; }`.
4. **`libsel4/sel4_plat_include/ns02201-h1v1/sel4/plat/api/constants.h`** — `#pragma once`,
   `#include <sel4/config.h>`, `#include <sel4/arch/constants_cortex_a73.h>`.
5. **`libsel4/arch_include/arm/sel4/arch/constants_cortex_a73.h`** — from `_a76.h`; guard
   `CONFIG_ARM_CORTEX_A73`; A73 TRM: 6 breakpoints, 4 watchpoints (same counts).
6. **`src/drivers/serial/config.cmake:11`** — `"nvidia,tegra20-uart;ti,omap3-uart;snps,dw-apb-uart;ns16550a"`.
7. **`src/arch/arm/config.cmake`** — `elseif(KernelArmCortexA73)` in the PA chain (40-bit, like A76);
   `OR KernelArmCortexA73` in the hypervisor `DEPENDS` (L83) and the 64-byte cache-line list (L238-240).
8. **`configs/seL4Config.cmake`** — `KernelArmCortexA73` in the unset list (L135-153),
   `config_set(KernelArmCortexA73 ARM_CORTEX_A73 "${KernelArmCortexA73}")` (L189-198),
   `elseif(KernelArmCortexA73) set(KernelArmCPU "cortex-a73" …)` (L219-239).

Commit message: why the platform is missing, where each value was measured, why the DTS is
trimmed, why `ns16550a` reuses the DW-APB driver. Verify through C4's build: `kernel.elf`
`readelf -l` first PT_LOAD paddr `0x10000000`; `support/platform_gen.yaml` memory
`0x10000000..0x40000000`; `gen_config.json` has `PLAT_NS02201_H1V1`, `ARM_CORTEX_A73`,
`ARM_PA_SIZE_BITS_40`, `ARM_HYPERVISOR_SUPPORT`, `MAX_NUM_NODES 1`, `PRINTING`.

## C3. rust-sel4 fork — one commit, `1ef65fb1`-shaped (2 files)

1. **`crates/sel4-kernel-loader/src/plat/ns02201/mod.rs`** (~50 lines): `const UART_BASE: usize =
   0x2_f013_0000;` `THR = 0x00`, `LSR = 0x14`, `THRE = 1 << 5`; `put_char` = spin on
   `read_volatile(LSR) & THRE` then `write_volatile(THR, c as u32)`;
   `put_char_without_synchronization` identical (no lock needed for a polled port; document);
   `init()` empty — BL31 configured the port, exactly as the probe relied on;
   `init_per_core` → `reset_cntvoff()` under `sel4_cfg_bool!(ARM_HYPERVISOR_SUPPORT)`;
   `start_secondary_core` → `crate::arch::drivers::psci::start_secondary_core` (unused at
   `MAX_NUM_NODES 1`; PSCI 0.2 `CPU_ON` is what TF-A implements). Doc comment cites the seL4
   DTS node and P6.A's probe as the proof the address and register layout print.
2. **`crates/sel4-kernel-loader/src/plat/mod.rs`** — `else if #[sel4_cfg(all(ARCH_ARM,
   PLAT_NS02201_H1V1))] { #[path = "ns02201/mod.rs"] mod imp; }` + `#[cfg(false)] mod ns02201;`.

`rustfmt --check` on both files (the existing cv1800b arm is mis-indented upstream of us;
leave it). Verify through C4: `build_loader` prints the `--image-base`.

## C4. Slime host build

1. **`sel4/config/ns02201-h1v1.cmake`** — `cv1800b-duo.cmake` shape (plain `set … CACHE`), the
   ten settings of A4's kernel-config row, with a comment on why hyp and on the memory window.
2. **`scripts/build/build-sel4.py`** — fifth `Platform` `NS02201_H1V1` (`name="ns02201-h1v1"`,
   `build_dir=BUILD_ROOT/"sel4-ns02201-h1v1"`, `prefix_dir=…"-prefix"`,
   `target_profile="aarch64-sel4-nt98690-h1v1"`, `pins_section="ns02201_h1v1"`,
   `observed_prefix_section="observed_prefix_ns02201_h1v1"`, `random_seed="slime-sel4-ns02201-h1v1"`,
   `qemu_dtb=False`, `architecture="aarch64"`, `root_target_key="root_target"`,
   `child_target_name="aarch64-sel4-minimal.json"`, `loader_target_key="loader_target"`,
   `cross_compiler_environment="CROSS_COMPILER_PREFIX"`); add to `PLATFORMS`. No per-platform
   env branch is needed in `build_application` (timer from the register; no RX in P6.B).
3. **`sel4/pins.toml`** — `[ns02201_h1v1]` product half after the observed block: `platform =
   "ns02201-h1v1"`, `sel4_arch = "aarch64"`, `hypervisor = true`, `mcs = false`, `nodes = 1`,
   `verification_build = false`, `debug_build = true`, `printing = true`, `export_pcnt_user = true`,
   `export_ptmr_user = true`, `timer_irq = 30`, `timer_frequency_hz = 12000000`, `max_irq = 352`,
   `sel4_memory_base = "0x10000000"`, `sel4_memory_size = "0x30000000"`, `usable_memory_bytes`
   (from the first `platform_gen.yaml`), `reset_cg_base = "0x2f0020000"`, `reset_wdt_base =
   "0x2f0060000"`, `reset_write_width` (from C1), `boot_files = ["slime-nt98690-probe.bin",
   "slime-sel4-sample-ns02201-h1v1.bin"]`; new `[observed_prefix_ns02201_h1v1]` blessed once,
   deliberately, in the commit that adds the platform (the docs' rule: a later mismatch is a
   finding, never re-blessed to pass). Bump `[sel4].commit` and `[rust_sel4].commit`; extend
   `[rust_sel4]`'s comment with the third platform.
4. **`scripts/check/check-sel4-pins.py`** — `CONFIG_PATHS`, `PREFIX_PATHS`, the `rebuild` dict
   (`just sel4_nt98690_image_check`); `expected_cmake_values` arm for `ns02201_h1v1`
   (`KernelArmHypervisorSupport`, `KernelArmExportPCNTUser`, `KernelArmExportPTMRUser`);
   `check_profile` block modelled on cv1800b's: cmake == pins, `cpu == "cortex-a73"`,
   `timer_frequency_hz == 12_000_000 == cntfrq_el0_primary_hz`, `max_irq == 352 == gic_irqs`,
   `timer_irq == 30`, `serial == "uart0-ns16550a-0x2f0130000"`, DTS facts from
   `deps/sel4/tools/dts/ns02201-h1v1.dts` (`compatible = "ns16550a";`, `reg = <0x02 0xf0130000
   0x00 0x1000>;`, `reg-shift = <0x02>;`, `reg-io-width = <0x04>;`), overlay memory node ==
   `sel4_memory_base/size`, `boot_files`.
5. **Target profile** — `contracts/target-profile/v1/schema.zt` entry `AARCH64_SEL4_NT98690_H1V1`
   (id 8, A4 row, same base texts as `AARCH64_SEL4_QEMU_VIRT`, comment on why a distinct identity);
   `just boot_gen`; `check-architecture-contract.py::EXPECTED_PROFILES`;
   `build-generation.py` (`SEL4_NT98690_TARGET_PROFILE`, `SEL4_TARGET_PROFILES`,
   `prefix_by_profile` → `build/sel4-ns02201-h1v1-prefix`); `generation_fabric.py::SEL4_TARGET_PROFILES`
   (check what it gates before adding; it lists aarch64 profiles only);
   `scripts/lib/component_sdk.py::PROFILE_PLATFORMS` (platform, prefix, pins section) — not
   `DEFAULT_PROFILES`.
6. **`just/hardware.just`** — after the P6.A block: `sel4_nt98690_image_check: sel4_pin_check` →
   `build-sel4.py --platform ns02201-h1v1 --skip-pin-check`; `nt98690_sel4_check serial="":
   sel4_root_boot_check sel4_sample_check` → `check-nt98690-sel4.py {{serial}}` (the aarch64
   reference passes first, as the roadmap requires).
7. **`scripts/build/build-nt98690-payload.py --sel4 [--image …] [--output-stem …]`** —
   promote `read_load_segments`, `flatten`, `encode_branch` from `build-rpi5-media.py` into
   `scripts/lib/arm64_image.py` (the docstring's "second consumer" moment; `flatten` returns
   `(bytes, base, entry, end)` and no longer writes the entry word — rpi5 applies its own 4-byte
   `b`, the H1V1 its 64-byte header; `just rpi5_media_check` must give the same `kernel8.img`
   sha256). `--sel4` path (Duo `build_sel4` shape): require `build/slime-sel4-sample-ns02201-h1v1.{elf,identity.json}`,
   `target_profile` match, `elf_sha256` match; `segments[0].offset == 0` and `file_size >= 64`
   (the ELF header is what the arm64 header overwrites — say so); `base % 4096 == 0`; `base >=
   sel4_memory_base`, `end <= base + size`, outside `RESERVED_REGIONS`; `image[:64] = pack_header(
   code0=encode_branch(base, entry), text_offset=base, image_size=len(image))`; generalize
   `check_image_header(image, load, entry, *, landing_word, reserve_slack)` so the loader's
   branch target is checked to land on `MRS_MPIDR_EL1` and the probe keeps its rules; write
   `build/nt98690-payload/<stem>.bin` + `<stem>.identity.json` (`load_address = base`,
   `elf_sha256`, `generation_identity`, …). The probe keeps `identity.json`. `boot_file()`'s
   one-file rule becomes "the requested stem is in `boot_files`".

## C5. The gate — `scripts/check/check-nt98690-sel4.py` (new)

- Header docstring: why it is its own checker (A4 gate-shape row) and what it borrows.
- Constants: `PLATFORM = "ns02201-h1v1"`, `TARGET_PROFILE`, `SAMPLE_STEM =
  "slime-sel4-sample-ns02201-h1v1"`, `RUNS = 3`, `BOOT_TIMEOUT_SECONDS = 180`,
  `RESET_MARKER = r"SLIME_NT98690 reset request kind=wdt"`.
- **`REQUIRED_MARKERS`** (single chain; every regex in the `literal_for` vocabulary; verify the
  root lines against a fresh `just sel4_sample_check` QEMU transcript before freezing):
  the five U-Boot markers from P6.A (`is current device`, `edfe0dd0`, `\d+ bytes read in \d+ ms`,
  `Loading Device Tree to `, `Starting kernel \.\.\.`); `Starting loader`; `Entering kernel`;
  `Booting all finished, dropped to user space`; `SLIME_ROOT allocator slots=[1-9]\d*
  untypeds=[1-9]\d* bytes=[1-9]\d*`; `SLIME_TIMER acquired irq=30 freq_hz=12000000`;
  `SLIME_TIMER delivered badge=0x1 polls=\d+`; `SLIME_TIMER OK`; `SLIME_ROOT generation admitted
  number=\d+ executables=4 instances=4 grants=6 `; `SLIME_GRAPH activated instances=2`;
  `SLIME_TIMER phase=post-graph-start delivered badge=0x1 polls=\d+`; `SLIME_TIMER
  phase=post-graph-start OK`; `SLIME_ROOT READY target_profile=aarch64-sel4-nt98690-h1v1`;
  `RESET_MARKER`; `U-Boot 2021\.10`. (~19)
- **`FAILURE_MARKERS`**: the Duo's eleven + `Moving Image from`, `Bad Linux ARM64 Image magic!`,
  `SLIME_NT98690 reset failed`, `SLIME_ROOT FATAL .*` (already first in the Duo list).
- `main()`: `--serial` (fail closed naming P6.B), `--no-build`, `--evidence-dir`, `--manual-reset`
  (C1 fallback). Build: `build-sel4.py --sample-plane --platform ns02201-h1v1` then
  `build-nt98690-payload.py --sel4`; verify both identities. Per run `capture_boot`: `reach_uboot`
  (run 1: the operator power-cycles when told; runs 2-3: the board is already rebooting from the
  watchdog and the CR spam catches the prompt) → staging exactly as P6.A (`mmc dev 0`,
  `md.l ${fdtcontroladdr} 1` fail-fast, `fatload mmc 0:1 <base> <stem>.bin` with byte count ==
  file, `md.l` head/tail compare, `booti <base> - ${fdtcontroladdr}`) → read in 0.5 s slices until
  `RESET_MARKER`, a failure marker, or timeout. Recovery: runs 1-2 — the next iteration's
  `reach_uboot`, extended to **return the text it collected**, supplies the banner line (truncated
  at end of line as P6.A does); run 3 — the P6.A silent banner window, so the last recovery is
  observed without a keystroke and the board proceeds to its vendor Linux. Per run: marker
  contract (`match_marker_contract`), `sample.check_transcript(transcript)`, `framing_errors == 0`,
  `normalize` (Duo `DYNAMIC_FIELDS` verbatim; keep `SLIME_`/`[init]`/`[sample-` lines; truncate at
  `RESET_MARKER`), write `sample-run-N.log` + `.normalized.log`. After three: byte-identical or
  fail. Summary claim names the profile, the run count, framing total, and the recovery mode.
- Register `("nt98690_sel4", "check/check-nt98690-sel4.py", <count>)` in `GATES`.
- Board-host bundle grows by: this gate, `check-sel4-sample-plane.py` and whatever it imports
  (`harness.py`, …), `sel4_gate_markers.py`, the sample `.bin` and both identity files. Prove it
  runs standalone with system Python (as S1 did) before wormholing.

## C6. `slime-root`

1. **`build.rs`** — `if target_profile == "aarch64-sel4-nt98690-h1v1" { rustc-cfg=slime_ns02201_h1v1 }`;
   emit `slime_physical_target` for both physical profiles; declare both check-cfgs.
2. **`graph_runtime/services.rs:1460`** — `#[cfg(slime_cv1800b_duo)]` → `#[cfg(slime_physical_target)]`
   on the `SLIME_ROOT READY target_profile=` line (compiled in only for the Duo today).
3. **`platform_timer.rs`** — `#[cfg(slime_ns02201_h1v1)] pub fn request_ns02201_watchdog_reset(cg:
   MappedGranule, wdt: MappedGranule) -> bool`: `cg.read32(0x9c) | 1<<4 → write32`,
   `cg.read32(0x400) | 1<<24 → write32`, `wdt.write32(0x0, 0x5a96_0112)`, `wdt.write32(0x0,
   0x5a96_0113)`, `wdt.write32(0xc, 1)`. If C1 showed only 64-bit writes reset, add
   `read64`/`write64` to `MappedGranule`/`DeviceRegion` beside the 32-bit pair. Doc: TF-A
   `nvt_ns02201/pm.c::nova_system_reset`, `novatek_def.h` (`CG_BASE`, `WDT_BASE`, `ATF_CG_RESET_OFS
   0x9C`, `ATF_WDT_RST 4`, `ATF_CG_ENABLE_OFS 0x400`, `ATF_WDT_POS 24`, `MAN_RST_OFS 0xC`).
4. **`main.rs`** — `#[cfg(slime_ns02201_h1v1)]` `NS02201_CG_PADDR = 0x2_f002_0000`,
   `NS02201_WDT_PADDR = 0x2_f006_0000`; two scratch pages + `DeviceRegion::map` granules (mirror
   782-795); statics like `DUO_RESET_REGISTERS`; `request_ns02201_reset(cg, wdt) -> !` printing
   `SLIME_NT98690 reset request kind=wdt`, calling the helper, printing `SLIME_NT98690 reset
   failed` + spinning on `false` (mirror 348-359); call it at the sample-plane completion site
   (1049-1050) under `#[cfg(slime_ns02201_h1v1)]`. **Also** from `fatal!`: when the statics are
   `Some`, attempt the reset before `suspend_self` — on a bench with no scriptable reboot, a
   wedged FATAL costs a power cycle, and the Duo never needed this only because its Linux answers
   `reboot`.
5. Host tests unchanged in count (`just test_sel4_root` = 211); `fmt_check_all`, `lint_all`.

## C7. Verification (in order)

Host, inside `nix develop`: `just boot_gen` → `just contracts_check` → `just generation_check` →
`just sel4_pin_check` → `just sel4_nt98690_image_check` (first build; inspect `readelf -l`
physBase, `platform_gen.yaml`, `gen_config.json`; bless `[observed_prefix_ns02201_h1v1]` in this
commit) → `python3 scripts/build/build-nt98690-payload.py --sel4` (header, base, identity) →
`just rpi5_media_check` (byte-identical `kernel8.img` after the flatten move) → `just
sel4_root_boot_check`, `just sel4_sample_check` (aarch64 reference unchanged by the root diff) →
`just test_sel4_root` (211) → `just sel4_gate_control_check` (47 gates) → `just ruff`, `typos`,
`fmt_check_all`, `lint_all`, `devlog_check`; `rustfmt --check` on the fork files.
Board: C1 (one cycle) → `nt98690_sel4_check <serial>` (one cycle, then two autonomous) →
evidence back by wormhole.

## C8. Devlog, roadmap, memory

- `devlog/2026-09-02-p6b-sel4-nt98690-h1v1/index.md` (Kind Change; Scope names `deps/{sel4,
  rust-sel4}`, `sel4/pins.toml`, `sel4/config/ns02201-h1v1.cmake`, `contracts/target-profile/v1/`,
  `scripts/{build,check,lib}/`, `slime-root`; Gates `sel4_nt98690_image_check`, `nt98690_sel4_check`,
  `sel4_gate_control_check`; Verification rows in P3.E's shape, failed campaigns as rows; Decisions:
  reset pulled from P6.C, platform naming, trimmed DTS, no early-fault, push rights) with
  `reset-probe.log`, `sample-run-{1,2,3}.log` + `.normalized.log`; register in `devlog/README.md`.
- Roadmap P6.B: `#### Exit condition` written now, `**Exit condition (observed):**` after the run;
  P6.C loses its reset deliverable; `## Sequencing`; `roadmap/README.md` node text.
- Memory: status, branch names, push state, what S3 inherits.

## C9. Stop conditions (Defect entries, not improvisation)

Kernel silent after `Entering kernel` (UART_PPTR / driver — `--monitor` first); `Moving Image
from` (base drift — the identity's `load_address` vs the fatload address); `SLIME_TIMER FAIL`
(PPI 30 assumption: confirm hyp in `gen_config.json` and `CNTHCTL_EL2`); reset marker printed
but no banner (C1 fallback); normalized traces differ (inspect the diff before touching
`DYNAMIC_FIELDS`); `sample.check_transcript` rejects a board transcript (A7.11).

---

# Part D — Session 3 plan (P6.C: interactive Slisp over UART0), appended 2026-09-02

Part A's `## A8. Session 3 outline` is superseded by this part. Corrections to Part A made
by Session 3's exploration:

1. **The generalisation A8 named is wider than `build.rs`.** The real blocker is
   `scripts/build/build-sel4.py`'s product-UART env branch, which gates on
   `platform is CV1800B_DUO` *and* parses `serial` with `uart0-dw-apb-(0x…)` — a regex the
   H1V1's `uart0-ns16550a-0x2f0130000` does not match. And the H1V1's post-graph
   `request_ns02201_reset()` in `slime-root/src/main.rs` is unconditional, missing the Duo's
   `not(<uart cfg>)` guard that lets a product graph stay resident.
2. **A7 risk 9 (fork push rights) is closed the good way.** The forks live at
   `github.com/CG-AA/{seL4,rust-sel4}` with `slime-ns02201-h1v1` pushed at the pinned
   hashes, and Slime `main` is pushed to `CG-AA/slime_os`. P6.C needs no fork changes.
3. The Duo's slisp gate (`check-duo-slisp.py`) is **not** in the shared tamper control's
   `GATES` — it exposes no `REQUIRED_MARKERS`. P6.C's gate is written to be eligible, which
   constrains its marker grammar to `literal_for`'s vocabulary (newlines spelled `\n`,
   matched against a CR-stripped view of the transcript).

## D1. Board-neutral product-UART build inputs (one commit, Duo behavior unchanged)

Rename the inputs the roadmap calls board-named: env `SLIME_DUO_UART_PADDR` →
`SLIME_PRODUCT_UART_PADDR` (cfg `slime_duo_uart` → `slime_product_uart`), env
`SLIME_DUO_TEST_TERMINATOR` → `SLIME_PRODUCT_TEST_TERMINATOR` (cfg likewise),
`build-sel4.py` flag `--duo-test-terminator` → `--test-terminator`, identity key
`duo_test_terminator` → `test_terminator`. Genuinely Duo-specific things keep Duo names
(`SLIME_DUO_TIMEBASE_HZ`, `SLIME_DUO_EARLY_FAULT`, `request_duo_test_reset`, the RTC cold
reset and its `kind=cold` markers). `build.rs`'s guard becomes "physical board profiles
only"; `build-sel4.py`'s env branch covers both boards with a per-platform serial kind
(`dw-apb` / `ns16550a`); `main.rs` gains `request_ns02201_test_reset() -> !` (prints
`SLIME_NT98690 test terminator accepted`, calls `request_ns02201_reset()`), installs it
under `all(slime_product_test_terminator, slime_ns02201_h1v1)`, and guards both post-graph
resets with `not(slime_product_uart)`. `DwApbInput` keeps its name; its doc comment goes
board-neutral (both boards pin reg-shift 2 / io-width 4). Rename-neutrality is checked by
sha256 comparison of the two Duo graph images before and after.

## D2. H1V1 product image and payload

`build-sel4.py --component-graph --platform ns02201-h1v1 --test-terminator` over the
existing `sel4.zti` product composition (6 executables / 6 instances / 11 grants,
`bootAction = "product"` ⇒ resident dispatcher); the aarch64 product Slisp builds against
`build/sel4-prefix` exactly as the QEMU-arm product does. `[ns02201_h1v1].boot_files` gains
`slime-sel4-ns02201-h1v1-test-terminator.bin`; `build-nt98690-payload.py` propagates
`variant` and `test_terminator` into the payload identity.

## D3. The gate — `scripts/check/check-nt98690-slisp.py`

Own checker; borrows `uboot_console`, the P6.A staging helpers, and `sel4_gate_markers`.
Single ordered `REQUIRED_MARKERS` chain (~30, frozen against a fresh QEMU product-graph
transcript and a stand-in rehearsal), P6.B's failure set plus the resident-graph failures
(`SLIME_GRAPH exhausted`, the Slisp exit line, `[slisp] repl done`, `! input`). One scored
boot, one operator power-cycle: P6.A staging → boot markers through
`[slisp] resident input wait` → type `(define answer 40)`, `(+ answer 2)`, `sysinfo` →
resident checkpoint at 32768 iterations → `(+ answer 3)` → framing check → terminator
`0x1d` → `SLIME_NT98690 test terminator accepted` → `reset request kind=wdt` → banner.
Duo-parity assertions: exactly one resident-wait line, exactly one healthy line,
`framing_errors == 0` before the terminator. Evidence `slisp-session.log` +
`slisp-identities.json`. Registered in `check-sel4-gate-controls.py::GATES`.

## D4–D7. Recipe, records, verification, stop conditions

`just nt98690_slisp_check serial="": slisp_core_check sel4_component_graph_check`; the
S1/S2-shaped operator bundle, proven standalone before wormholing. Roadmap 07 gains P6.C's
`#### Verification target` and `#### Exit condition` up front and the observed exit after
the run; the stale header and P6-umbrella status lines are corrected. A new devlog entry
carries the session evidence; the memory file's stale push-state lines are fixed. Host
verification: reference gates, rename-neutrality hashes, `duo_gate_control_check`, the
contract/generation/pin/image/payload chain, `sel4_gate_control_check` with the new gate,
ruff/typos/fmt/lint/devlog. Stop conditions are Defect entries: RX without echo, repeated
or missing resident-wait, a slow resident checkpoint, a swallowed terminator,
`SLIME_NT98690 reset failed`, a marker `literal_for` cannot instantiate, or any QEMU
reference shift under the rename.
