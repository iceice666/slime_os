# NT98690 (Novatek NS02201) H1V1 lane — plan of record

This is the plan the P6 lane is being executed from, copied into the
repository at the start of the work so that later sessions read it here
rather than rediscovering the board. Part A is durable: it is the set of
verified facts, fixed names, and decisions that P6.B and P6.C build on, and
every fact in it cites the vendor or upstream source it came from. Part B is
the session that produced [this entry](index.md), and is kept as written so
that what was planned can be compared against what the accompanying Change
entry records as actually done.

Two parts. **Part A** is the durable meta plan for all three sessions: every verified fact,
decision, name, and pointer a later session needs so it never re-explores. **Part B** is the
executable plan for Session 1. Session 1's first task copies Part A into the repository (a
Decision devlog entry) so it survives independently of this plan file.

---

# Part A — Meta plan (durable; copy into the repo in S1 step B0)

## A1. Goal and exit conditions

Bring Slime OS to the Novatek NT98690 (= NS02201) **H1V1** camera board as a third physical
lane beside the Milk-V Duo (observed) and the Pi 5 (unobserved). Final exit (end of S3): the
H1V1 boots loader → seL4 → `slime-root` from an SD card through the **unmodified vendor
U-Boot**, never writing eMMC, and the resident Slisp shell answers typed UART0 input — every
claim backed by a fail-closed serial gate and a devlog entry, in the Duo lane's shape.

| Session | Milestone | Exit condition (observed) | Gates |
|---|---|---|---|
| S1 | **P6.A** environment + firmware-handoff probe | A bare-metal AArch64 probe placed by `booti` at the pinned address prints EL/ID/timer/GIC/FDT facts and `check … = ok` verdicts on UART0, ends `SLIME_NT98690 PAYLOAD_OK`, resets via PSCI, and the vendor U-Boot banner returns with no operator action; observed facts written to pins | `just nt98690_payload_check`, `just nt98690_boot_check <serial>`, `just sel4_gate_control_check` |
| S2 | **P6.B** seL4 + `slime-root` | Same handoff boots the seL4 image; ordered `SLIME_ROOT …`/`SLIME_TIMER …`/`SLIME_ROOT READY target_profile=aarch64-sel4-nt98690-h1v1` three times with byte-identical normalized traces, autonomous reset each time | `just sel4_nt98690_image_check`, `just nt98690_sel4_check <serial>` |
| S3 | **P6.C** interactive Slisp over UART0 | Product graph boots; `slisp> ` prompt; three typed commands answered; test-terminator byte triggers reset | `just nt98690_slisp_check <serial>` |

Out of scope until P6.C closes: storage, network, display, generation management on this
board; any verified-kernel claim (this SoC is not in seL4's verified set).

## A2. Verified board / firmware facts (do not re-derive)

Paths under `/srv/novatek/sdk/worktrees/h1v1-dev/` unless noted. H1V1 model config:
`configs/Linux/cfg_690_IPC_EMMC_RAMDISK_CM2504/` (`nvt-*.dtsi`, `ModelConfig.mk`). Built vendor
DTB: `/srv/novatek/sdk/worktrees/lamb-h1v1/output/nvt-evb.bin` (`dtc -I dtb -O dts`). Companion
repo with the proven serial-driven U-Boot loop: `~/nt98690-ubuntu/`.

| Fact | Value | Source |
|---|---|---|
| CPU | 4× `arm,cortex-a73` (ARMv8.0-A), 1 cluster, `enable-method = "psci"` | `nvt-basic.dtsi` |
| Firmware chain | BootROM → Novatek loader (`LD98690A.bin`, binary) → TF-A 2.2 BL31 → U-Boot 2021.10 (`nvt-ns02201_a64_pci_emmc_defconfig`) → `nvt_boot` → `booti` | `BSP/trusted-firmware-a/plat/novatek/nvt_ns02201/`, `BSP/u-boot/board/novatek/` |
| EL at payload entry | **EL2**, AArch64, MMU off, D-cache disabled; **x0 = relocated FDT, x1=x2=x3=0** | `bl31_setup.c:84`, no `CONFIG_ARMV8_SWITCH_TO_EL1`, `arch/arm/lib/bootm.c` |
| PSCI | 0.2 via SMC; `SYSTEM_RESET` (watchdog bit `ATF_WDT_RST` at `CG_BASE+ATF_CG_RESET_OFS`) and `CPU_ON` (hold page `0x01FFF000`) implemented | `nvt_ns02201/pm.c:101-205` |
| GIC | GICv2 `arm,cortex-a7-gic`: GICD `0x2_fff01000`, GICC `0x2_fff02000`, GICH `0x2_fff04000`, GICV `0x2_fff06000`; TF-A already ran distif/cpuif init; PMU SPIs up to 311 | `nvt-basic.dtsi`, `novatek_def.h:27-28` |
| Generic timer | `arm,armv8-timer`, PPIs 13/14/11/10; DTS `clock-frequency = <12000000>`; **TF-A writes CNTFRQ_EL0 only on secondaries** (`plat_helpers.S:156-159`); primary value unknown → S1 observes and calibrates | `nvt-basic.dtsi`, TF-A |
| UART0 | `ns16550a` @ `0x2_f0130000`, `reg-shift=2`, `reg-io-width=4` (THR `0x00`, LSR `0x14`: THRE bit 5, TEMT bit 6, DR bit 0), 48 MHz, 115200 8N1, initialised by BL31; no `stdout-path` | `nvt-peri.dtsi:38-50`, `bl31_setup.c:65` |
| DRAM | 2 GiB physical at `0x0`; vendor `/memory` = `0x0..0x4000_0000` → U-Boot `ram_base=0`, `ram_size=1 GiB`. Carve-outs: `0x0–0x2_0000` core2entry, `0x10_0000` loader DTB, `0xA0_0000` SHMINFO, `0x100_0000` loader, **`0x1F0_0000–0x1FF_FFFF` BL31 (no-map)**, `0x480_0000–0xAC0_0000` CMA pools, `0x7C70_0000` kernel staging, `0x7E00_0000–0x8000_0000` U-Boot | `nvt-mem-tbl.dtsi`, `ModelConfig.mk`, `nvt_ivot_common.c` |
| TZASC | protects only BL31 against DMA masters; the CPU can access all DRAM | `nvt_tzasc_ns02201.c:592` |
| MMIO | `0x2_f000_0000–0x2_ffff_0000` (above 4 GiB) | `novatek_def.h:35-36` |
| **`booti` relocation (Novatek patch)** | `dst = gd->ram_base` unconditionally → image always moved to **`text_offset`** (`ALIGN(0,2M)+text_offset`); header flags bit 3 ignored; `image_size==0` ⇒ assumes 16 MiB/`0x80000`; prints `Moving Image from … to …` only when `relocated_addr != load address` | `arch/arm/lib/image.c` under `CONFIG_ARCH_NOVATEK`, `cmd/booti.c` |
| **FDT argument mandatory** | `booti <addr> - ${fdtcontroladdr}`; omitting panics (`FDT and ATAGS support not compiled in`); `${fdtcontroladdr}` = loader DTB (~`0x10_0000`, `OF_PRIOR_STAGE`); U-Boot relocates the FDT top-down under `bootm_size` (1 GiB) and passes the copy in x0 | `common/bootm.c`, `arch/arm/lib/bootm.c`, defconfig 679/684 |
| U-Boot commands | present: `booti`, `mmc`, `fatload/fatls`, `tftpboot/ping`, `md/mw`, `reset`, `setenv` (volatile: `ENV_IS_NOWHERE`); **absent: `go`, `bootelf`, `bootm`, `echo`, `fdt`, `loadb/loady`** | defconfig |
| Autoboot | `bootdelay=0` (one `tstc()` poll), `bootcmd=nvt_boot`, prompt `nvt: `; interrupt = spam `\r` from before power-on — proven by `~/nt98690-ubuntu/_recovery_h1v1/raw_flash/flash_emmc_raw.py::get_prompt`, `diag_emmc_ptbl.py::catch_uboot` | defconfig 333/340/425 |
| SD handling | `mmc dev 0` **before** `fatload mmc 0:1` (fatload on an unprobed slot can hang; expect `mmc0 is current device`); SD = MBR + one FAT32 partition, label `NVTFW` (`~/nt98690-ubuntu/scripts/flash_sd_card.sh` mode 3) | vendor scripts |
| Board switch | SW18 = `0x1001` (eMMC 8-bit) with SD inserted; `0x0001` = loader rescue — **never** | `~/nt98690-ubuntu/project_env.sh:47-48` |
| Recovery | `reset` at the prompt or PSCI reset from the payload → loader → U-Boot → vendor Linux; nothing on eMMC touched; floor = SD Rescue with `/srv/novatek/sdk/archive/blobs/recovery/LD98690A.H1V1.bin` | `boot_test.py`, `docs/boards/h1v1.md` |
| Serial | `/dev/ttyUSB0` 115200 in vendor scripts; loader prints benign `RRRR…` runs (not a hang); no board pinout doc exists in the SDK | `flash_h1v1.sh:27`, `docs/h1v1-bringup-troubleshooting.md:45` |
| Ethernet | eth0/eth1 (`nvt,synopsys_eth`) enabled; U-Boot TFTP viable as a second injection path (defaults 192.168.1.99/.11) | `nvt-peri.dtsi:266`, defconfig |

## A3. Verified upstream/fork facts (seL4 16.0.0 fork `iceice666/seL4` @ f25b760; rust-sel4 fork `iceice666/rust-sel4` @ 070c6a3)

- `tools/hardware.yml`: console class includes `ns16550a`; GICv2 class includes `arm,cortex-a7-gic`; timer `arm,armv8-timer`; PSCI `arm,psci-0.2`.
- `src/drivers/serial/config.cmake` maps `snps,dw-apb-uart`/`ti,omap3-uart`/`nvidia,tegra20-uart` → `tegra_omap3_dwapb.c` (THR `0x0`, LSR `0x14`, THRE bit 5) but **not `ns16550a`** → S2 adds `ns16550a` to that `declare_driver` list (one line).
- `src/arch/arm/config.cmake:13-36` picks PA bits from plain `KernelArmCortexA{35,53,55,57,72,76}` variables (no A73); the same set appears in the hypervisor `DEPENDS` (line 109) and cache-line list (142-144) → S2 adds `KernelArmCortexA73` with PA bits from the S1-observed `PARange`. Compiler flag is `-march=${KernelArmArmV}${KernelArmMachFeatureModifiers}` (no `-mcpu`).
- Platform precedent `src/plat/bcm2712/config.cmake`: `declare_platform(bcm2712 KernelPlatformRpi5 PLAT_BCM2712 KernelArchARM)`, `KernelArmCortexA76 ON`, `KernelArchArmV8a`, `+crc`, `declare_default_headers(TIMER_FREQUENCY 54000000 MAX_IRQ 320 INTERRUPT_CONTROLLER arch/machine/gic_v2.h TIMER drivers/timer/arm_generic.h …)`, DTS list `tools/dts/rpi5b.dts` + `src/plat/bcm2712/overlay-rpi5.dts` (`seL4,elfloader-devices`, `seL4,kernel-devices`, `seL4,boot-cpu`, vGIC maintenance IRQ). The fork's own `src/plat/cv1800b/` is the in-project precedent for adding a platform.
- rust-sel4 loader: plat arms `qemu_arm_virt`, `bcm2711`, `bcm2712`, `riscv_generic` selected by `sel4_cfg_if!` on `PLAT_*`; `Plat` trait = `init/init_per_core/put_char/put_char_without_synchronization/start_secondary_core`; `bcm2712/mod.rs` ≈ 60 lines (`sel4_pl011_driver` + PSCI); **no 16550 driver crate** → S2 inlines a 16550 `put_char` in `plat/ns02201/mod.rs`; aarch64 `enter_kernel` **asserts CurrentEL == EL2** (`src/arch/arm/arch/aarch64/mod.rs`) — satisfied by `booti` at EL2; loader link address = kernel phys end + 256 KiB (`build.rs`, `--image-base`); loader markers `Starting loader`, `Entering kernel`.

## A4. Fixed decisions and names

| Decision | Value / rationale |
|---|---|
| Names | Slime platform **`ns02201-h1v1`** (`sel4/config/ns02201-h1v1.cmake`, pins `[ns02201_h1v1]`, `[observed_prefix_ns02201_h1v1]`) — SoC codename + board like `cv1800b-duo`; target profile **`aarch64-sel4-nt98690-h1v1`** (id 8, arch 2, abi 4, page 2, features 66, elfMachine 183, cargoTarget `deps/rust-sel4/support/targets/aarch64-sel4-minimal.json`, qemuBinary `""`) — board-facing like `riscv64-sel4-milkv-duo`; seL4 platform **`ns02201`** (`declare_platform(ns02201 KernelPlatformNS02201 PLAT_NS02201 KernelArchARM)`); marker prefix **`SLIME_NT98690`**; recipes/scripts **`nt98690_*`**, `check-nt98690-*.py`, `build-nt98690-payload.py`, `tools/nt98690/payload/`; roadmap **P6** (P6.A/P6.B/P6.C) in `roadmap/07-architecture-portability.md` |
| Boot handoff | SD FAT32 `NVTFW` + vendor U-Boot: `mmc dev 0` → `fatload mmc 0:1 L <file>` → `booti L - ${fdtcontroladdr}`. Payload carries a 64-byte arm64 `Image` header with `text_offset = L`, `image_size = full span`, `flags = 0xA`, so Novatek's forced relocation is a no-op; `Moving Image from` is a **failure marker**. No U-Boot rebuild, no eMMC write. |
| Load address L | **`0x1000_0000`** (S1 probe and S2 images). 256 MiB-aligned, 84 MiB above the last carve-out (`0xAC0_0000`), far below the FDT relocation zone and U-Boot, inside U-Boot's 1 GiB. S2 sets the seL4 `memory` node to start at `0x1000_0000` (kernel physBase); loader link = kernel end + 256 KiB; `text_offset` computed from the flattened ELF, never hard-coded. |
| Kernel config (S2) | mirror `qemu-arm-virt.cmake`: `KernelArmHypervisorSupport ON` (EL2 entry; loader requires EL2), `KernelIsMCS OFF`, `KernelMaxNumNodes 1`, `KernelVerificationBuild OFF`, `KernelDebugBuild ON`, `KernelPrinting ON`, `KernelArmExportPCNTUser/PTMRUser ON` (root's CNTP/PPI 30 path) |
| seL4 memory window (S2) | `memory@10000000 { reg = <0 0x10000000 0 0x30000000> }` (768 MiB) inside the vendor-visible 1 GiB; widen only from observed BootInfo |
| Kernel serial (S2) | reuse `tegra_omap3_dwapb.c` by adding `ns16550a` to its compatible list; DTS keeps the vendor `compatible = "ns16550a"` |
| Timer frequency | kernel `TIMER_FREQUENCY` and root frequency from the **S1-observed** `CNTFRQ_EL0` cross-checked against the UART-clock calibration; stale/zero ⇒ pinned override in `platform_timer.rs::frequency_hz()` (precedent: Duo's `SLIME_DUO_TIMEBASE_HZ`) |
| Autonomous recovery | S1/S2 payloads end with PSCI `SYSTEM_RESET` (SMC32 `0x8400_0009`); S3's root (EL0, no SMC) writes the same watchdog bit TF-A uses (`CG_BASE + ATF_CG_RESET_OFS`, `ATF_WDT_RST`) via a mapped MMIO granule, like the Duo's RTC reset |
| Remote UART | gates accept `--serial /dev/ttyUSBn` **or** `--serial tcp:HOST:PORT` (socat/ser2net bridge + ssh tunnel); framing-error counting only on a local tty (reported "unobservable" over TCP) |
| Verification discipline | one new gate per new execution environment (`check-nt98690-boot.py`); U-Boot console code lives in `scripts/lib/uboot_console.py` used by the **new** gate; the three Duo gates keep their inline code until a Duo is on a bench (recorded follow-up); tamper control via `check-sel4-gate-controls.py::GATES` (rpi5 precedent), so every marker regex must be `literal_for`-instantiable |
| Payload toolchain | `nix shell nixpkgs#pkgsCross.aarch64-embedded.buildPackages.{gcc,binutils}` (bare-metal `aarch64-none-elf`), mirroring `build-duo-payload.py::nix_shell`'s attribute pattern |

## A5. Slime-side reuse map (verified paths)

- Serial/U-Boot driving: `scripts/check/check-duo-boot.py` — `open_serial` (PARMRK), `class Console {write, read_for, framing_errors}`, `reach_uboot(console, prompt, window)`, `load_and_start`, `report_transcript`, `check_transcript`, `monitor`, fail-closed `--serial`.
- AArch64 flattening: `scripts/build/build-rpi5-media.py` — `read_load_segments`, `flatten`, `encode_branch` (line ~135), `check_entry_is_code`, `elf_entry`. Duo: `build-duo-payload.py::read_load_segments/flatten_sel4/encode_jal/nix_shell/load_profile/check_link_address/build_binary`.
- Probe precedent: `tools/duo/payload/smoke.S` + `smoke.ld` (banner, hex prints, `PAYLOAD_OK`).
- Recipes: `just/hardware.just` (`duo_payload_check`, `duo_boot_check serial=""` with `{{ if serial == "" { "" } else { "--serial " + serial } }}`, `duo_serial_monitor`, `sel4_duo_image_check`, `rpi5_media_check`).
- Gate control: `scripts/check/check-sel4-gate-controls.py::GATES` (rpi5 entry line 76) + `literal_for` grammar (literals, `\d+`, `[1-9]\d*`, `[0-9a-f]+`, `[0-9a-f]{n}`, `[0-9]{n}`, `[0-9a-fx]+`, `[^ ]+`, `\s+`, backreferences).
- Pins/build (S2): `sel4/pins.toml` `[cv1800b_duo]`, `[bcm2712_rpi5]`; `scripts/build/build-sel4.py` `Platform` dataclass (14 fields), `PLATFORMS`, `build_application` (`platform is CV1800B_DUO` branches; `SLIME_DUO_UART_PADDR` regex `uart0-dw-apb-(0x…)`), `build_loader`, `package_image`, `write_manifest`; `scripts/check/check-sel4-pins.py` (`CONFIG_PATHS`, `PREFIX_PATHS`, `expected_cmake_values`, `check_profile` per-platform blocks).
- Profile (S2): `contracts/target-profile/v1/schema.zt` (+ regenerate `boot-contracts/src/generated/target_profile.rs`, `scripts/lib/boot_contracts.py`), `scripts/build/build-generation.py` (`SEL4_TARGET_PROFILES`, `prefix_by_profile`), `scripts/check/check-architecture-contract.py::EXPECTED_PROFILES`.
- Root (S2/S3): `slime-root/build.rs` cfgs (`slime_duo_uart`/`SLIME_DUO_UART_PADDR` → generalise to `slime_board_uart`/`SLIME_BOARD_UART_PADDR`), `slime-root/src/device.rs::DwApbInput` (RBR `0x0`, LSR `0x14` — **register-identical to UART0**), `platform_timer.rs` (aarch64 CNTP/PPI 30, `read_cntfrq`), `graph_runtime/platform.rs::probe_authority_devices` (virtio scan at `0x0a00_0000` degrades to an empty inventory when the paddr is not a device untyped — harmless on the board; cfg-gate to the QEMU profile in S2).
- Roadmap/devlog: `roadmap/07-architecture-portability.md` P3.D/P3.E/P3.F section shapes, profile table lines 13-19, `## Sequencing`; `roadmap/README.md` track table + mermaid + "Physical bring-up sequencing"; `devlog/TEMPLATE.md`, `devlog/README.md` (Decision: Summary/Changes/Decisions/Open risks/Artifacts; Change adds Regression guards/Verification; `Roadmap` ids must exist; `Gates` must be real just targets; siblings linked from Artifacts); `just devlog_check`; `_typos.toml` already excludes `devlog/*.log`.

## A6. Go/no-go facts S1 observes (they parameterise S2)

| Observation | Pass | S2 consequence |
|---|---|---|
| `el` = 2 | EL2 | keep `KernelArmHypervisorSupport ON`; if 1 → non-hyp config and check the loader's EL1 path |
| `base` == `0x10000000`, no `Moving Image` | placed | header scheme valid; S2 loader image uses the same header |
| `x0` in DRAM, `fdt_magic` = `d00dfeed` | valid | recorded; seL4 ignores x0 |
| `midr_part` = `0xd09`; `parange` | A73; 0x2→40-bit, 0x4→44-bit | `KernelArmCortexA73` PA-bits block |
| `cntfrq` vs `cnt_hz_est` | both ≈ 12 000 000 | `TIMER_FREQUENCY`; mismatch ⇒ pinned override (CNTFRQ_EL0 is not writable below EL3) |
| `check cnt_advance` = ok | counter runs | else stop: timer unusable |
| `gicd_typer`, `gic_irqs`, `gicd_iidr` | ≥ 352 lines | `MAX_IRQ`, `max_irq` pin |
| GICD read above 4 GiB succeeded | yes | loader identity map / kernel device window must cover >32-bit PAs |
| `sctlr_el2` MMU off, `hcr_el2`, `cnthctl_el2`, `cntvoff_el2`, `pfr0` GIC bits = 0 | recorded | loader entry assumptions; GICv2 confirmed |
| U-Boot banner after PSCI reset | returns | autonomous recovery for S2's repeat loop; vendor console end state (root shell vs `login:`) decides whether `reboot` is scriptable |

## A7. Risks carried forward

1. Primary `CNTFRQ_EL0` may be unset (DTS `clock-frequency` is a broken-firmware hint in Linux).
2. Novatek `booti` relocation: any drift in `text_offset` silently moves the image; gate treats `Moving Image` as failure.
3. Manual power cycles: no relay; each run costs one unless the vendor console offers a scriptable `reboot`.
4. Exact U-Boot strings (`mmc0 is current device`, `Loading Device Tree to`, banner) must be confirmed from the survey log before markers are frozen.
5. No `echo`/`fdt`/`go`: prompt-as-sentinel only; no jump without a valid Image header.
6. Remote UART over SSH adds latency to the CR-spam window; start spam before power-on; budget retries.
7. Nix install needs root on this server; the supported environment is the flake dev shell.
8. Root `probe_authority_devices` scans QEMU's `0x0a00_0000` on every AArch64 build — harmless, cfg-gate in S2.

## A8. Session 2 and 3 outlines (plan them in their own sessions from this file)

**S2 — P6.B**: fork seL4: `src/plat/ns02201/{config.cmake, overlay-ns02201-h1v1.dts}`, `tools/dts/ns02201-h1v1.dts` (from `dtc -I dtb -O dts output/nvt-evb.bin`, trimmed to cpus/psci/timer/gic/uart0/memory/reserved-memory with the A4 memory node), `ns16550a` in `src/drivers/serial/config.cmake`, `KernelArmCortexA73`; fork rust-sel4: `crates/sel4-kernel-loader/src/plat/ns02201/mod.rs` (inline 16550 `put_char`) + `plat/mod.rs` arm; bump `[sel4]`/`[rust_sel4]` pins; Slime: `sel4/config/ns02201-h1v1.cmake`, `Platform` record, pins product keys + `[observed_prefix_ns02201_h1v1]`, `check-sel4-pins.py` block (incl. `[ns02201_h1v1]` cross-checks), profile id 8 + regen + `EXPECTED_PROFILES` + `build-generation.py`, `build-nt98690-payload.py --sel4` (flatten loader ELF via `arm64_image.py` + `read_load_segments/flatten`), `check-nt98690-sel4.py` (3 sample boots, normalized identical, early-fault control), root: cfg-gate virtio scan, timer override if needed; `just sel4_nt98690_image_check`, `just nt98690_sel4_check`; `crc32`/`tftpboot` if the S1 survey shows them; devlog Change entry.

**S3 — P6.C**: generalise `slime_duo_uart`/`SLIME_DUO_UART_PADDR` → `slime_board_uart`/`SLIME_BOARD_UART_PADDR` (+ per-controller serial regex in `build-sel4.py`), reuse `DwApbInput` for UART0 RX, product graph image for the profile, test-terminator reset via the CG watchdog MMIO, `check-nt98690-slisp.py`, `just nt98690_slisp_check`; devlog Change entry; roadmap P6 status + README.

---

# Part B — Session 1 plan (P6.A: environment + firmware handoff probe)

## B0. Persist the meta plan (first task)
1. `devlog/2026-09-01-p6-nt98690-h1v1-lane/index.md` (Kind **Decision**, Status Proposed, Roadmap `P6, P6.A, P6.B, P6.C`, Gates `nt98690_payload_check`, `nt98690_boot_check`) with sibling `plan.md` = Part A verbatim; Decisions section = A4 + the S1/S2/S3 split + "eMMC untouched" + "Duo gate refactor deferred"; register in `devlog/README.md`. The P6 roadmap headings (B8) must exist before `just devlog_check` passes.
2. Memory: `~/.claude/projects/-space-slime-os/memory/nt98690-h1v1-lane.md` (type project) pointing at the devlog entry and this plan file; add the `MEMORY.md` line.

## B1. Environment bootstrap on this server (no board needed)
1. `sudo -n true` — Nix needs root once; if unavailable, stop and report (no ad-hoc toolchain path: it would leave the payload outside the reproducibility contract).
2. Install Nix with flakes (Determinate installer, or official `--daemon` + `experimental-features = nix-command flakes`); new shell; `nix --version`.
3. `git submodule update --init --recursive`; confirm `deps/sel4` = `f25b760…`, `deps/rust-sel4` = `070c6a3…` (`sel4/pins.toml`).
4. `nix develop --command just --list` (installs both pinned Rust toolchains); `nix develop --command just sel4_pin_check` must pass.
5. `nix shell nixpkgs#pkgsCross.aarch64-embedded.buildPackages.gcc nixpkgs#pkgsCross.aarch64-embedded.buildPackages.binutils --command aarch64-none-elf-gcc --version` — record the version (goes into identity.json).
6. If time: `just sel4_qemu_image_check` + `just sel4_root_boot_check` (baseline green; otherwise first action of S2).
7. Board access (operator): on the UART host `socat -d -d TCP-LISTEN:5000,bind=127.0.0.1,reuseaddr,fork FILE:/dev/ttyUSB0,b115200,cs8,raw,echo=0`; here `ssh -N -L 5000:127.0.0.1:5000 <user>@<uart-host>`; gate endpoint `tcp:127.0.0.1:5000`. One client at a time (never monitor + gate together). SD card + SW18 `0x1001`.

## B2. Probe sources — `tools/nt98690/payload/probe.S`, `probe.ld`
- `probe.ld`: `ENTRY(_start)`, `PAYLOAD_BASE = 0x10000000;`, `.text : { KEEP(*(.text.start)) *(.text .text.*) }`, `.rodata`, `. = ALIGN(16); . += 0x1000; __stack_top = .; __image_end = .; __image_size = __image_end - PAYLOAD_BASE;`, `/DISCARD/ .comment .note.* .eh_frame`. No `.data`/`.bss`.
- `probe.S` (AArch64, modelled on `tools/duo/payload/smoke.S`):
  1. **arm64 Image header at `_start`** (64 bytes): `b entry` · `.long 0` · `.quad PAYLOAD_BASE` (text_offset) · `.quad __image_size` · `.quad 0xa` (LE, 4K, bit 3) · `.quad 0,0,0` · `.ascii "ARM\x64"` (offset 56) · `.long 0`.
  2. `entry`: `msr daifset,#0xf`; save x0–x3 → x19–x22; `adr x23,_start`; `mrs x24,CurrentEL; lsr x24,x24,#2`; `sp = __stack_top`; install `vbar_el2` (or `vbar_el1` if EL1); `isb`. 64-bit constants via `movz/movk` (no literal pools).
  3. Print lines `SLIME_NT98690 <name padded to 10> = 0x%016lx` (`\r\n`), banner first: `=== SLIME_NT98690 probe: entry reached ===` (neutral — the EL claim is measured, not asserted), then `el`, `base`, `pc`, `x0`, `x1`, `x2`, `x3`, `fdt_magic` (LE word at x0, `rev`, only if `0x1000 <= x0 < 0x80000000`, else all-ones), `fdt_size`, `midr`, `midr_part` (`ubfx #4,#12`), `mpidr`, `mmfr0`, `parange` (`& 0xf`), `pfr0`, `cntfrq`, `cntpct0`, **calibration burst** (`SLIME_NT98690 calib      = ` + 230 `.` between two `LSR.TEMT`-synchronised `cntpct` reads; 230 × 86.8 µs ≈ 20 ms → `cnt_hz_est = cnt_delta * 50`), `cntpct1`, `cnt_delta`, `cnt_hz_est`, `sctlr_el2`/`sctlr_el1`, `hcr_el2`, `cnthctl_el2`, `cntvoff_el2`, `gicd_ctlr`, `gicd_typer`, `gicd_iidr` (32-bit loads at `0x2fff01000+0/4/8`), `gic_irqs` (`32*((typer&0x1f)+1)`).
  4. Verdict lines: `SLIME_NT98690 check placement   = ok|FAIL` (x23 == PAYLOAD_BASE), `check el2`, `check fdt_magic`, `check mmu_off` (`sctlr.M == 0`), `check cnt_advance` (cntpct1 > cntpct0), `check gicd` (typer ∉ {0, 0xffffffff}); then `SLIME_NT98690 PAYLOAD_OK` or `PAYLOAD_FAIL`.
  5. Exit: `SLIME_NT98690 reset request kind=psci`, wait `TEMT`, `x0 = 0x84000009; x1=x2=x3=0; smc #0`; on return print `SLIME_NT98690 reset failed x0 = …`, `wfe` loop.
  6. Vectors (`.balign 2048`, 16 × `b fault`): `fault` prints `SLIME_NT98690 FAULT esr=… elr=… far=…` then the same reset sequence.
  7. UART helpers: base `0x2f0130000`, THR `+0x00`, LSR `+0x14`; `putc` polls THRE (bit 5), 32-bit stores; `puts`, `put_hex64`.

## B3. Header lib + payload builder
- New `scripts/lib/arm64_image.py`: `MAGIC = 0x644d5241`, `HEADER = struct.Struct("<IIQQQQQQII")`, `Header` dataclass, `parse_header`, `pack_header`, `encode_branch` (moved from `build-rpi5-media.py`, which imports it; regression `just rpi5_media_check` → `build/rpi5-media/kernel8.img` sha256 unchanged).
- New `scripts/build/build-nt98690-payload.py` (shape of `build-duo-payload.py`): `nix_shell()` copied; `load_profile()` → `[ns02201_h1v1]` requiring `board, soc, payload_load_address, dram_base, firmware_memory_size, boot_files`; `check_link_address()` (`PAYLOAD_BASE` from `probe.ld` == pin, `% 0x200000 == 0`, outside `RESERVED = ((0x0, 0x02000000), (0x04800000, 0x0AC00000), (0x7C000000, 0x80000000))`, below `firmware_memory_size`); `build_binary()` (`aarch64-none-elf-gcc -x assembler-with-cpp -march=armv8-a -nostdlib -nostartfiles -ffreestanding -Wl,--build-id=none -T probe.ld`, `objcopy -O binary` → `build/nt98690-payload/slime-nt98690-probe.bin`); `check_image_header()` (`code0` decodes to a `b` inside the image past the header, `code1 == 0`, `text_offset == load`, `len <= image_size <= len + 0x2000`, `flags == 0xa`, magic, `res5 == 0`, ELF entry == load); `identity.json` (`board, soc, target_profile="aarch64-nt98690-bringup", march, load_address, entry_address, text_offset, image_size, flags, payload_bytes, payload_sha256, boot_file, toolchain`); prints the operator copy instruction (nothing writes block devices).

## B4. Shared U-Boot console lib + the gate
1. New `scripts/lib/uboot_console.py` lifted from `check-duo-boot.py`: `open_serial` verbatim; `Console(endpoint, baud, fail)` — `tcp:HOST:PORT` → socket (`framing_errors = None`), else tty with PARMRK counting; `write`, `read_for`, `flush_input`, `describe`, `close`; `reach_uboot(console, prompt, window, *, key=b"\r", interval=0.05)` — pre-step: `\r`, read 1.5 s: prompt → `reset\r`; `# `/`$ ` → `reboot\r`; `login:` → fail with operator instruction; else log "power-cycle now"; then spam `key` until `prompt`, drain, confirm the prompt answers a bare CR (4 tries); `send_command(console, command, prompt, timeout)`; `report_transcript`; `check_transcript(transcript, required, failures)`; `monitor`. Duo gates untouched (follow-up recorded).
2. New `scripts/check/check-nt98690-boot.py` (justified: new execution environment). Module-level `REQUIRED_MARKERS`, `FAILURE_MARKERS`, `check_transcript` for the gate control. `main()`: `--serial` (fail closed naming P6.A), `--timeout` 300, `--monitor`, `--transcript`, `--no-build`; `load_pins()` → `[ns02201_h1v1]` requiring `board, soc, serial_baud, payload_load_address, boot_partition, boot_files, uboot_prompt, uboot_banner, uboot_select_device, uboot_launch`; build → `check_identity()` (sha256 vs identity.json, `parse_header(bin).text_offset == pin`); `reach_uboot`; `send_command("mmc dev 0")` require `mmc0 is current device`; `send_command("md.l ${fdtcontroladdr} 1")` require `edfe0dd0`; `fatload mmc 0:1 0x10000000 slime-nt98690-probe.bin` require `(\d+) bytes read` == `payload_bytes`; `md.l 0x10000000 0x10` and tail-block `md.l` must equal the image's first/last 64 bytes; launch `uboot_launch.replace("{load}", …)` → `booti 0x10000000 - ${fdtcontroladdr}`; read until `reset request kind=psci` / `PAYLOAD_FAIL` / `FAULT` / 30 s; then **send nothing** and wait for `uboot_banner` ≤ 90 s, +3 s, stop; `check_transcript` over the launch window; print a facts table parsed from `SLIME_NT98690 <name> = 0x…` lines; write `--transcript`.
   `REQUIRED_MARKERS` (ordered, `literal_for`-compatible; ~20): `mmc0 is current device` · `\d+ bytes read in \d+ ms` · `Loading Device Tree to 0{8}[0-9a-f]{8}` · `Starting kernel \.\.\.` · banner · `SLIME_NT98690 el         = 0x0{15}2` · `base       = 0x0{8}10{7}` · `x0         = 0x0{8}[0-9a-f]{8}` · `fdt_magic  = 0x0{8}d00dfeed` · `midr_part  = 0x0{13}d09` · `parange    = 0x0{15}[0-9a-f]{1}` (tighten after run 1) · `cntfrq     = 0x[0-9a-f]{16}` (tighten) · `check cnt_advance = ok` · `cnt_hz_est = 0x[0-9a-f]{16}` · `check mmu_off     = ok` · `gicd_typer = 0x0{8}[0-9a-f]{8}` (tighten) · `gic_irqs   = 0x[0-9a-f]{16}` · `PAYLOAD_OK` · `reset request kind=psci` · `U-Boot 2021\.10`.
   `FAILURE_MARKERS`: `Moving Image from`, `Bad Linux ARM64 Image magic!`, `Wrong Image Format for booti command`, `Did not find a cmdline Flattened Device Tree`, `ERROR: can't get kernel image`, `MMC Device 0 not found`, `Failed to load 'slime-nt98690-probe.bin'`, `"Synchronous Abort" handler`, `Resetting CPU`, `SLIME_NT98690 FAULT`, `SLIME_NT98690 PAYLOAD_FAIL`, `SLIME_NT98690 reset failed`, `= FAIL`. Confirm every U-Boot wording against the survey log (B7.1) before freezing.
3. Register `("nt98690_boot", "check/check-nt98690-boot.py", <final marker count>)` in `scripts/check/check-sel4-gate-controls.py::GATES`.

## B5. Recipes — `just/hardware.just` (after the Duo block, same comment register)
`nt98690_payload_check` → `python3 scripts/build/build-nt98690-payload.py`; `nt98690_boot_check serial="": nt98690_payload_check` → `python3 scripts/check/check-nt98690-boot.py {{ if serial == "" { "" } else { "--serial " + serial } }}`; `nt98690_serial_monitor serial timeout="180"` → `--monitor --serial {{ serial }} --timeout {{ timeout }}`.

## B6. Pins — `sel4/pins.toml` `[ns02201_h1v1]` (board facts only; product keys arrive in S2)
`soc = "ns02201"` (NT98690), `board = "novatek-h1v1"`, `cpu = "cortex-a73"`, `cpus = 4`, `entry_el = 2` (observed), `serial = "uart0-ns16550a-0x2f0130000"`, `serial_reg_shift = 2`, `serial_reg_io_width = 4`, `serial_clock_hz = 48000000`, `serial_baud = 115200`, `dram_base = "0x0"`, `dram_size = "0x80000000"`, `firmware_memory_size = "0x40000000"`, `atf_base = "0x01f00000"`, `atf_size = "0x00100000"`, `interrupt_controller = "arm-gic-v2-0x2fff01000"`, `gicd_base/gicc_base/gich_base/gicv_base`, `dts_timer_frequency_hz = 12000000`, `psci = "0.2-smc"`, `uboot_version = "2021.10-novatek-ns02201"`, `uboot_prompt = "nvt: "`, `uboot_banner = "U-Boot 2021.10"` (exact text from the survey log), `uboot_select_device = "mmc dev 0"`, `uboot_launch = "booti {load} - ${fdtcontroladdr}"`, `payload_load_address = "0x10000000"`, `boot_partition = "mmc 0:1"`, `sw18_boot_position = "0x1001"`, `boot_files = ["slime-nt98690-probe.bin"]`; after the observed run: `cntfrq_el0_primary`, `counter_hz_estimated`, `parange`, `midr`, `gic_irqs`, `gicd_iidr`. Provisional values carry `# provisional: <source>; P6.A replaces`. Each key's source in the comment header.

## B7. Board sessions (operator present)
1. **Survey first**: `just nt98690_serial_monitor tcp:127.0.0.1:5000 180` across a power-cycle → `vendor-boot.log` (loader banner, exact U-Boot banner, `nvt: `, vendor Linux end state: root shell vs `login:`). Then catch the prompt and run `help`, `bdinfo`, `printenv fdtcontroladdr`, `printenv fdt_high`, `printenv bootcmd`, `mmc dev 0`, `fatls mmc 0:1`, `ping <uart-host>` → `uboot-survey.log`. Pin `uboot_banner`; note `crc32`/network availability for S2.
2. Card: `~/nt98690-ubuntu/scripts/flash_sd_card.sh <dev> 3` with a dir holding the `.bin` (FAT32 `NVTFW`); `sha256sum` on the mounted card == `identity.json.payload_sha256`; insert; SW18 `0x1001`.
3. Run 1 (loose markers) via the script directly with `--transcript build/nt98690-payload/run1.log`; read the facts table.
4. Tighten `parange`/`cntfrq`/`gicd_typer` markers to observed values; fill observed pins.
5. Run 2 (tight markers) → transcript committed as `probe-boot.log` beside the P6.A devlog entry.

## B8. Roadmap + devlog
- `roadmap/07-architecture-portability.md`: profile-table row `aarch64-sel4-nt98690-h1v1` (Role "Second physical AArch64 bring-up target; probe stage"; Machine "Named Novatek NT98690/NS02201 H1V1, 4× Cortex-A73, vendor TF-A 2.2 + U-Boot 2021.10"; Baseline "AArch64, 4 KiB granule, EL2 firmware entry, GICv2 above 4 GiB, generic timer with observed CNTFRQ, 16550 UART0, SD FAT32 `booti` handoff at a pinned text_offset, PSCI reset"); `## P6: Novatek NT98690 (NS02201) H1V1 physical lane` with `### P6.A — H1V1 environment bootstrap and EL2 probe handoff evidence` (Status / Depends on P5 and the P3.D–P3.F precedents / Deliverables / Required checks / Verification target / Exit condition (observed) / Evidence), `### P6.B — seL4 and slime-root on the H1V1` (planned; A8 scope), `### P6.C — Interactive Slisp over UART0 on the H1V1` (planned); `## Sequencing` item 9; header Status mentions P6.A.
- `roadmap/README.md`: track row gains P6.A; mermaid node `H1V1["P6 NT98690 H1V1 lane\nP6.A probe observed"]` with `P5 --> H1V1`, `Duo -.->|precedent| H1V1`; one sentence in "Physical bring-up sequencing".
- Devlog: the Decision entry (B0) and `devlog/2026-09-01-p6a-nt98690-probe/index.md` (Kind Change, Status Verified, Roadmap `P6.A`, Gates `nt98690_payload_check`, `nt98690_boot_check`, `sel4_gate_control_check`; Regression guards = gate control + builder header check + gate identity/`md.l` checks; Verification = run 1, run 2, hashes; the A6 table filled with observed values; Artifacts `probe-boot.log`, `vendor-boot.log`, `uboot-survey.log`, identity hashes). Register both in `devlog/README.md`; `just devlog_check`.

## B9. Verification (inside `nix develop`)
`just nt98690_payload_check` · `just nt98690_boot_check` without serial → nonzero P6.A message · `just sel4_gate_control_check` (every regex instantiates; chain rejects deletion/reorder/failure) · `just duo_gate_control_check` (Duo untouched) · `just rpi5_media_check` (flattener/`encode_branch` move byte-identical) · `just sel4_pin_check` · `just devlog_check` · `just ruff` · `just typos` · `just nt98690_boot_check tcp:127.0.0.1:5000` twice (B7.3/B7.5). Optional pre-board smoke: a `-DSLIME_QEMU_VIRT` assembly variant (PL011 at `0x09000000`) under `qemu-system-aarch64 -M virt,virtualization=on -cpu cortex-a73 -kernel …` — expect `check placement = FAIL`, everything else printing; catches assembler/header mistakes before a card round-trip.
Then branch `p6a-nt98690-h1v1-probe`, commit, PR (change / claim / risk / review surface / verification).

## B10. Stop conditions (record as Defect entries, do not improvise)
Board never reaches `nvt: ` (adapter/wiring → monitor first); `Moving Image` appears (placement scheme wrong); `el` ≠ 2; `check cnt_advance = FAIL`; PSCI reset does not return the U-Boot banner (S2 must plan a different recovery).
