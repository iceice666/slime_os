# H1V1 ESC lane — meta plan (Part A), Session 1 (Part B), Sessions 2–3 (Part C)

Drive a common drone ESC with standard servo PWM from Slime OS on the Novatek NT98690
(NS02201) H1V1, commanded from the resident Slisp shell: `(pwm 0 1600)` spins the motor fast,
`(pwm 0 1200)` slow, `(pwm 0 0)` stops it. Three sessions, three PRs, in the P6 lane's shape,
cut so that **board-specific work never rides in the general PR**:

| PR | Session | Content | Upstream flavour |
|---|---|---|---|
| 1 | S1 | Bench probe from the vendor U-Boot prompt; the board facts pinned; the pad found | NT98690-specific, self-contained (touches only `nt98690` files, pins, devlog) |
| 2 | S2 | Declared-device inventory in the root, `pwm-servo` contract, Novatek PWM driver component, Slisp builtin, `sel4-pwm` composition, all observed on QEMU | **General**: no `slime_ns02201_*` cfg, no pins, no board gate |
| 3 | S3 | H1V1 carve + clock/pinmux bring-up, build inputs from pins, the board gate, the observed exit, P6.D records | NT98690-specific |

**Part A** is the durable half (facts with sources, fixed names, reuse map, go/no-go, risks).
**Part B** is Session 1. **Part C** is Sessions 2 and 3. Session 1's first repository task copies
Part A into this entry's `plan.md` so it outlives the planning file.

## Context

- P6.A/B/C are complete and upstream: the H1V1 boots seL4 + `slime-root` from SD through its
  unmodified vendor U-Boot and answers typed Slisp input over UART0. Storage, network, display,
  sensors, and actuators on this board are explicitly unclaimed
  (`roadmap/07-architecture-portability.md` P6; `roadmap/README.md` L35, L52).
- Nothing in the tree touches PWM or GPIO. `robot-actuator` is a simulated call-plane server.
  The IO substrate (IO1) has every grant kind a driver needs, but physical-address resolution
  is virtio-only.
- User decisions (2026-09-07): common drone ESC → standard servo PWM, frame rate a request
  parameter; evidence wanted = "I can control the servo, see it go fast then slow" → a servo
  tester / servo / the ESC itself is enough, **no logic analyzer**; the pinout supplied is the
  SoC pinmux table (`NT98690_PinMux_table_20231101.xlsx - MainPinMux.csv`), which confirms pad
  functions but not which pads reach a connector; command path = Slisp; roadmap home =
  **P6.D + an IO-track item**; PRs split general vs board-specific.
- Local repo: `main` == upstream `main` (58fec41, after PR #24, MyQue store cutover);
  submodules on the per-platform loader layout; old lane tip tagged `archive/p6-lane-presquash`.
  Every session branches from `main`.

---

# Part A — Meta plan (durable)

## A1. Goal and exit conditions

| Session | Milestone | Exit condition (to be observed) | Gates |
|---|---|---|---|
| S1 | **Bench probe + facts** | From the `nvt:` prompt a raw register sequence produces servo PWM on a named, header-reachable pad; a servo tester / servo follows 1000 → 1500 → 2000 us; the ESC arms and runs fast then slow; pinmux/clock reset state and the GPIO-readback question are observed and pinned. No Slime code runs. | `check-nt98690-boot.py --pwm-probe` / `--gpio-probe` (bench modes; no recipe, by precedent) |
| S2 | **General mechanism** (IO-track item) | The root can install a *declared* (non-probed) device into the IO1 inventory; a Zutai `pwm-servo` protocol has generated Rust and C; `nvt-pwm-driver` binds, maps, guards, serves the protocol with a failsafe; Slisp has `(pwm ch us)`; the `sel4-pwm` composition boots on QEMU with the driver reporting `device absent` and health green; every host/QEMU gate is green. | `contracts_check`, `generation_check`, `sel4_boot_layout_check`, `sel4_component_graph_check`, `slisp_core_check`, `io_driver_authority_check`, `sel4_root_boot_check`, `test_sel4_root`, `test_host`, `sel4_gate_control_check`, fmt, lint, ruff, typos, devlog, tasks |
| S3 | **P6.D — servo PWM on the H1V1** | One resident session on the named board: `(pwm 0 1000)`, `(pwm 0 1600)`, `(pwm 0 1200)`, `(pwm 0 0)`; the driver's register readbacks match the pinned words after each; the terminator resets the board; 0 framing errors; transcript committed. The motor going fast then slow is an operator observation, labelled. | `just nt98690_pwm_check <serial>`, `sel4_gate_control_check`, `nt98690_slisp_check` (P6.C stays green) |

Out of scope: DShot/OneShot (this block cannot bit-bang them), IRQ delivery to userspace
(free-running PWM needs none), sampling the pad from Slime (follow-on if S1 proves it
possible), ROS/fabric integration.

## A2. Hardware facts — sources verified 2026-09-07

Bases: PWM `0x2_F012_0000` (8 KiB), CG `0x2_F002_0000`, TOP `0x2_F001_0000`,
PAD `0x2_F003_0000`, GPIO `0x2_F004_0000`. `K/` = `/srv/novatek/sdk/worktrees/h1v1-dev/BSP/linux-kernel/`,
`U/` = `…/BSP/u-boot/`, `C/` = `…/configs/Linux/cfg_690_IPC_EMMC_RAMDISK_CM2504/`.

| Fact | Value | Status / source |
|---|---|---|
| PWM CTRL(ch) | `ch*8+0x00`, [15:0] cycle count, **0 = free run** | `K/drivers/pwm/pwm-nvtivot.c:73`, `U/drivers/pwm/nvt_pwm.c:20-22`, `K/…/plat-ns02201_a64/pwm.h:80-82` |
| PWM PERIOD(ch) | `ch*8+0x04` = rise[7:0] \| fall[15:8] \| base_period[23:16] \| invert bit 28; high from `rise` to `fall`; `base_period` = clocks per period (no −1); `rise<=fall<=base_period` | `pwm-nvtivot.c:162`, `nvt_pwm.c:118-128`, `pwm.h:74-79` |
| PWM EXT_PERIOD(ch) | `0x230+ch*4`, upper 8 bits of each field; **channels 0–7 only**; always concatenated → must be written | `pwm-nvtivot.c:170-173`, `nvt_pwm.c:131`, `K/drivers/pwm/Kconfig:375-377` |
| ENABLE / DISABLE / LOAD | `0x100` W1S (reads back live) / `0x104` W1C / `0x108` latch a new period while free-running | `pwm-nvtivot.c:78-80,250-253,319-324` |
| PWM clock | 120 MHz `fix120m`; divider CG+0x30 [13:0] ch0–3, [29:16] ch4–7; **field = divisor − 1** (3..16383) | `K/include/dt-bindings/clock/nvt-ns02201.h:68`, `C/nvt-clock.dtsi:4090-4218`, `K/drivers/clk/novatek/nvt-clk-provider.c:906-913` |
| PWM clock gate | CG+0x84 bit ch (1 = on), channels 0–11 | `nvt-ns02201.h:95`, `nvt-clock.dtsi:4098` |
| PWM shared reset | CG+0xA4 **bit 0, shared by all channels**, already released by U-Boot; **never clear** | `nvt-ns02201.h:110`, `nvt-clock.dtsi:4101`, `U/drivers/pwm/nvt_pwm_ns02201.c:295` |
| PWM APB clock | no software gate; nothing to enable | `C/nvt-clock.dtsi:4070-4088` |
| **PWM12 = CPU core-voltage regulator** | P_GPIO[42]; TOP+0x1C[19:16]=1; gate CG+0x8C bit 22; divider CG+0x324; driven by U-Boot at boot (`power_ctl`). **Never touch PWM+0x60/0x64, PWM+0x104 bit 12, CG+0xA4, CG+0x8C, CG+0x324, TOP+0x1C.** | `U/arch/arm/mach-novatek/nvt_ns02201_a64/nvt-otp.c:157-159`, `clock.c:909-912`, `nvt_pwm.c:252-306`; pinmux CSV row `P_GPIO42 … PWM12 (for Core power adjust)` |
| Pinmux PWM<ch> → P_GPIO[ch], ch 0–7 | TOP+0x18 field [4ch+3:4ch] = 1; TOP+0xA8 bit ch = 0 (0 = FUNCTION, 1 = GPIO) | `K/…/plat-ns02201_a64/top_reg.h:165-179,509-547`, `K/drivers/pinctrl/novatek/ns02201/ns02201_pinmux_host.c:3534-3535`, `nvt_pwm_ns02201.c:41-45` |
| Pinmux state at Slime entry | **not** the dtsi: U-Boot has no pinctrl; only Linux applies it; the loader blob is opaque. Expect TOP+0x18 = 0. | `U/configs/nvt-ns02201_a64_pci_emmc_defconfig:1121,209` — **unobserved; S1 reads it** |
| Vendor kernel PWM | `CONFIG_PWM` not set — `pwm-nvtivot.c` never runs on this board; documentation only | `K/arch/arm64/configs/ns02201_ipc_a64_pci_defconfig_emmc_release:2974` |
| P_GPIO[0..19] pad rail | **3.3 V** (`PAD_3P3V`), set by U-Boot `power_init()`; default pull-down | `C/nvt-peri-dev.dtsi:8`, `U/board/novatek/nvt-ns02201-a64/ns02201_hw_init.c:344-365`; CSV rows `P_GPIO0..5` |
| P_GPIO[0..5] reservations | none (`nvt-top.dtsi` `pwm = <0x0>`; `i2cIII = 0x111` does not use I2C15–17; `nvt-gpio.dtsi` empty) | `C/nvt-top.dtsi`, `K/include/dt-bindings/pinctrl/nvt-ns02201-pinctrl.h:170-177,209-227` |
| Package pins | P_GPIO0..5 are dedicated pins (INT[58..63]); alternates UART8/9, I2C15–17, I2S2/3, SN2 CCIR input | pinmux CSV rows 107–112 |
| GPIO block, P_GPIO bank (bank 1) | DATA +0x04, DIR +0x34 (1 = out), SET +0x64, CLR +0x94; bit n = P_GPIO[n] | `K/…/plat-ns02201_a64/nvt-gpio.h:20-39`, `K/drivers/gpio/gpio-nvt.c:433-556` |
| GPIO DATA reflects a FUNCTION-muxed pad? | **unknown** — S1 tests it | — |
| Physical header for P_GPIO[0..5] | **unknown on disk**; P_GPIO[31..33,42] are wired (remote, console UART, core PWM), so the bank is partly broken out. **S1 finds it.** | — |
| U-Boot bench commands | `md/mw`, `sleep`, `reset`; **`pwm` command present** (never needed: it forces 30 MHz and clears the pinmux); `power_ctl` present (**never run**); no `gpio`, `echo`, `go` | `U/configs/…defconfig:491,602,520,1142`, `U/drivers/pwm/nvt_pwm.c:160-249,354-364`, `nvt_pwm_ns02201.c:49-52` |
| Servo encoding at 1 MHz (divider field 119) | 50 Hz: base 20000 = 0x4E20; 1500 us → fall 0x05DC, rise 0 → PERIOD `0x0020DC00`, EXT `0x004E0500`; 1600 us → PERIOD `0x00204000`, EXT `0x004E0600`; 1200 us → PERIOD `0x0020B000`, EXT `0x004E0400`; 1000 us → PERIOD `0x0020E800`, EXT `0x004E0300`. 1 count = 1 us. | arithmetic over the rows above |
| Device untyped carve order | retypes monotonically upward; existing carves CG → WDT → UART. PWM (0x2f012…) **after WDT, before UART**; TOP (0x2f001…) **before CG**. | `slime-root/src/main.rs:786-871`, `object_allocator.rs:1241-1264` (`DeviceFramePassed`) |

## A3. Slime facts — verified 2026-09-07

- IO1 grant kinds `device`/`mmioRegion`/`interruptSource`/`dmaAccount`, rights `mapMmio`/`irqAck`/`dmaPin`/`dmaRelease` (`boot-contracts/src/generation.rs:60-133`); the device **ordinal** is the IO1 budget's `device` field (`slime-root/src/graph_runtime.rs:421-453`, `services/io_resource.rs:340-380` ties device/region/irq ids to `quota.device + 1`); `MAX_IO_DEVICES = 2` (`slime-root/src/device.rs:683`).
- Physical resolution is virtio-only: `probe_authority_devices` (`slime-root/src/graph_runtime/platform.rs:62-125`) scans `VIRTIO_MMIO_BASE` with `VirtioMmio::probe`; `AuthorityDevice { region, offset }` (`platform.rs:4-7`), `AuthorityInventory { regions, devices, irqs, len }` (`:9-14`); called at `main.rs:1037-1044` only when the generation carries an `io-resource-budget` object. `Sel4IoAdapter::map_mmio` maps one 4 KiB granule at offset 0 for `PageExclusive` grants (`services/io_resource.rs:58-90`) via `DeviceRegion::map_child`, which evicts the root's own mapping. `install_driver` calls `grant_irq_source` unconditionally (check that `irqSources = 0` is tolerated). Userspace IRQ delivery is not wired — irrelevant, no IRQ needed.
- Precedents: direct page map `components/testkit/io-driver-probe/src/main.rs:119`; service loop `components/services/virtio-blk-driver/src/main.rs` (slots PEER 0, DEVICE 1, MMIO 2; `componentAbi`; polls endpoint + notifications; `peer_command()`); manifest grants `contracts/generation-manifest/v1/compositions/sel4-storage.zti:63-114,212-242,360-371`; compositions are generated from `contracts/system-spec/v1/systems/*.zti` (`scripts/generate/generate-generation-from-spec.py`, `scripts/lib/system_spec.py`); inventory `contracts/composition-inventory/v1/inventory.zti` (`systemSpec`, `baseline`, `owningGate`); boot-layout fixtures `contracts/boot-layout/v1/fixtures/` (`just sel4_boot_layout_check`/`_bless`, planes listed in `scripts/check/check-sel4-boot-layout.py`); clock authority precedent `contracts/generation-manifest/v1/compositions/sel4-clock-authority.zti:95-131`.
- The H1V1 product image is the `sel4` composition (no budget, no fabric); selected by `build-sel4.py --component-graph` via `VARIANT_MANIFESTS` (`scripts/build/build-sel4.py:230-237,755-795`); product gates on `variant == GRAPH_VARIANT` at `:905-916` (product UART env), `:930-942` (external Slisp), `:1326-1334` (terminator); board facts reach the root as `SLIME_PRODUCT_*` env parsed from `sel4/pins.toml [ns02201_h1v1]` and read with `const_parse_hex_usize(env!(…))` (`main.rs:535-536`).
- Slisp is freestanding C (`components/slisp/{slisp.h,slisp.c,main.c,host_main.c}`; slots INPUT 1, SPAWN_SERVICE 2); its outward calls are `slime_endpoint_exchange`, `slime_input_read`, `slime_debug_write`, `slime_exit` (`components/runtime/include/slime/component_runtime.h:22-32`); `contracts/spawn/v1` + `scripts/generate/generate-spawn-bindings.py` render both Rust and the C header `components/runtime/include/slime/spawn.h`. Spawn commands are 16-byte binding names with no arguments, so `(pwm ch us)` needs a builtin over a new endpoint slot. The external Slisp `contentHash` is overwritten at build (`build-generation.py:687-692`).
- Component addition: workspace glob + mandatory `[profile.release.package.slime-component-<name>]` stanza (`Cargo.toml:37-42`, model `:235-238`); `build.rs` = `slime_build_support::configure()` only; `contracts/component-spec/v1/components/<name>.zti`; `just component_spec_check`, `component_crate_split_check`.
- Board gates: `scripts/check/check-nt98690-{boot,sel4,slisp}.py`; `--reset-probe` (`check-nt98690-boot.py:459-492`) is the bench-mode shape; console lib `scripts/lib/uboot_console.py`; one marker contract per checker pinned in `scripts/check/check-sel4-gate-controls.py::GATES` (47 gates; `nt98690_boot` 25); `literal_for` bounds the regex grammar; bench modes have no `just` recipe.
- Records: MyQue store (`myque new`, UUID filenames, `parent`/`depends`; `just tasks_check` needs the nix dev shell); no open backlog item today. Roadmap: P6 explicitly does not own actuators (its boundary sentence will be amended for one channel); the IO track owns the mechanism. `devlog/TEMPLATE.md` front matter: Date, Kind, Status, Scope, Work items (UUIDs), Gates, Trigger, Baseline.
- Evidence discipline: no precedent for evidence a transcript cannot carry. A printed string is not an observation; unobserved conclusions are `[INFERENCE]`; operator steps are named and counted; a missing board fails, never skips.

## A4. Fixed decisions and names

| Decision | Value / rationale |
|---|---|
| Channel / pad | **PWM0 on P_GPIO[0]** by default (16-bit, 3.3 V, unclaimed; ch0–3 share one divider). S1 may substitute another of PWM0–5; the choice is pinned, never hard-coded. |
| PWM clock | **1 MHz** (divider field 119): 1 count = 1 us; 50 Hz = 20000 counts; frame rate is a request parameter (2000..20000 us). |
| Signal | rise 0, fall = pulse_us, base = period_us; free run (CTRL 0); update = PERIOD+EXT then LOAD bit; disable = DISABLE bit (pad idles low). |
| Never-touch set | CG+0xA4, CG+0x8C, CG+0x324, TOP+0x1C, PWM ch 8–12 registers, PWM+0x104 bit 12. Every CG/TOP write is read-modify-write. |
| Mechanism split | Root: carve + map the PWM page, program clock gate/divider and pinmux once at launch from pinned values (CG and TOP carry far more authority than "PWM"). Driver: PWM page only, channel policy, protocol, failsafe. |
| Component name | `nvt-pwm-driver` (IP-named like `virtio-blk-driver`); protocol `pwm-servo`; grants `pwm-block-device`, `pwm-block-mmio`, `slisp-pwm-rpc`; composition `sel4-pwm`; variant `--pwm-graph`. |
| Markers | `SLIME_NT98690 pwm bringup …` (root, S3); `SLIME_ROOT io authority inventory devices=1 mode=declared` (root, S2); `[nvt-pwm-driver] …` with literal `check <name> = ok` readbacks; Slisp `=> pwm 0 1600` / `! pwm <status>`. |
| Failsafe | driver programs 1000 us if a pulse above idle is live and no command arrives within `FAILSAFE_NS` = 10 s (monotonic clock via `clockAuthority`); a fired failsafe is a failure marker in the gate. |
| Pins keys (`[ns02201_h1v1]`) | S1 adds `pwm_base`, `pinmux_top_base`, `gpio_base`, `pad_base`, `pwm_clock_divider = 119`, `pwm_clock_hz = 1000000`, and after the bench `pwm_channel`, `pwm_pad`, `pwm_pad_header`, `pwm_pinmux_at_prompt`, `gpio_data_reflects_function_pad`, `pwm_probe_observed`. S3 consumes them. |
| Work items | lane parent (S1) → children: S1 bench, IO-track "declared-device authority and the pwm-servo protocol" (S2), **P6.D "Servo PWM on the H1V1"** (S3, `depends` on the IO item). |
| PR flow | PR-S1 (board bench) → PR-S2 (general) → PR-S3 (board). S2 depends on S1 only for facts, not code, so S2 can start while the bench waits. |

## A5. Reuse map (verified paths)

Console/U-Boot: `scripts/lib/uboot_console.py` (`Console`, `reach_uboot`, `send_command`, `report_transcript`); bench-mode shape `check-nt98690-boot.py::reset_probe` (L459–492), `read_register` (L453), `read_words` (L315), `wait_for_banner` (L429), `load_profile` (L248). Staging/session: `check-nt98690-slisp.py::{stage_and_launch, read_until, type_command, contract_view}`; build chain `build-sel4.py --component-graph --platform ns02201-h1v1 --test-terminator` → `build-nt98690-payload.py --sel4 --image … --output-stem …`; pins `boot_files` + `check-sel4-pins.py` list (~L550). Markers/tamper: `scripts/lib/sel4_gate_markers.py`, `check-sel4-gate-controls.py::{GATES, literal_for}`. Root primitives: `device::DeviceRegion::{map, remap, map_child, granule}`, `MappedGranule::{read32, write32}`, `ScratchPage::claim`, `FreePage`; H1V1 carve block `main.rs:786-871`; helper shape `platform_timer.rs:242-274`. IO1: `platform.rs::{probe_authority_devices, AuthorityInventory}`, `services/io_resource.rs::{install_driver, Sel4IoAdapter::map_mmio}`, wrappers `slime_rt::{io_device_bind, io_mmio_map, monotonic_read}`. Component skeleton: `components/services/virtio-blk-driver/{Cargo.toml,build.rs}`, `contracts/component-spec/v1/components/virtio-blk-driver.zti`. Contract with C header: `contracts/spawn/v1/`, `scripts/generate/generate-spawn-bindings.py`. Slisp: `main.c::{spawn_command, evaluate_line}`, `slisp.c::spawn_effect`, `host_main.c` effect vectors.

## A6. Go / no-go observations S1 must produce

| Observation | Decides |
|---|---|
| Which of P_GPIO[0..5] reaches a header/test point | the pinned channel/pad, or **no-go** for this board (fallback: D_GPIO[2..3] `_2` variants at 3.3 V, or a soldered lead) |
| TOP+0x18, TOP+0xA8, CG+0x84, CG+0xA4 bit 0, CG+0x30 at the prompt | the root's bring-up sequence (what it may assume vs must set) |
| Servo follows 1000 → 1500 → 2000 us; ESC arms at 1000 us, runs faster at 1600 than 1200 | signal encoding, polarity, frame-rate tolerance |
| GPIO DATA (+0x04 bit ch) toggles while the pad is FUNCTION-muxed and PWM runs | whether a follow-on can sample the pad from Slime without a wire |
| Vendor banner after `reset` and after a power cycle | the never-touch set was respected |

## A7. Risks

1. **Pad reachability** — the only fact no source answers; S1 settles it (meter from the known console pads; ask the vendor for the schematic in parallel).
2. **Core-rail PWM12** — a blind write to CG+0xA4/0x8C or PWM+0x104 drops the CPU voltage: RMW only, never-touch set, banner check afterwards.
3. **8 KiB block, one 4 KiB grant** — page 0 holds everything channels 0–7 need; pinned in the driver's doc comment.
4. **The driver's page includes channel 12 and the shared enable/disable bits** — no capability bound expresses a per-channel restriction; driver policy (bits 0–7 only) is the guard. Recorded as a known limit.
5. **Evidence class** — motor speed is not serial-observable; the exit conditions rest on register readbacks and the reset; motion is an operator observation.
6. **QEMU coverage of `sel4-pwm`** — its QEMU run must never bind a virtio transport as a PWM block: the driver's virtio-magic guard refuses; the boot-layout QEMU invocation attaches no virtio device (verify in S2).
7. **ESC frame-rate tolerance** — 50 Hz default; 400 Hz stays a request parameter.

---

# Part B — Session 1: bench probe and facts (no Slime code)

Goal: observe servo PWM on a named H1V1 pad from the vendor `nvt:` prompt, find the pad, and
pin every fact S3's root code will depend on. One PR, one devlog Audit entry (plus the lane
Decision entry), two power cycles. Nothing here writes a block device; the SD card is not needed.

## B0. Bench prerequisites

- Board on the bench with UART0 at `/dev/ttyUSB0`, SW18 at `0x1001`, power switch reachable.
- A servo tester or a servo with its own 5 V supply; signal pad is 3.3 V, so only signal +
  common ground go to the board. The ESC (propeller off, own battery, common ground) after the
  servo test passes.
- A multimeter for continuity and the pad-find step.
- **No logic analyzer.** The claim you want — the motor responds and goes fast then slow — is an
  operator observation the servo tester and the ESC give directly; the transcript carries the
  register readbacks. An analyzer would only add a measured pulse width to the record.

## B1. Host work before the board (one commit each)

1. **Work items.** `myque new` the lane parent and the S1 child (tags per Part A4); the devlog's
   `Work items` field needs the UUIDs. `just tasks_check` green (no open backlog item today).
2. **Lane record.** A `Decision` entry with this `plan.md` beside it: Kind `Decision`,
   Status `Proposed`, `plan.md` = Part A verbatim; register in `devlog/README.md`;
   `just devlog_check`.
3. **Bench modes in `scripts/check/check-nt98690-boot.py`** (same environment, same
   `md.l`/`mw.l` protocol, no marker contract → beside `--reset-probe`; no `just` recipe):
   - Constants beside the `RESET_*` block, each cited to A2: `PWM_BASE`, `PWM_CTRL(ch)`,
     `PWM_PERIOD(ch)`, `PWM_EXT_PERIOD(ch)`, `PWM_ENABLE`, `PWM_DISABLE`, `PWM_LOAD`,
     `CG_PWM_CLK_DIV0 = 0x30`, `CG_PWM_CLK_EN = 0x84`, `CG_PWM_RESET = 0xA4`,
     `CG_PWM12_CLK_EN = 0x8C`, `TOP_BASE`, `TOP_PWM_MUX = 0x18`, `TOP_PWM12_MUX = 0x1C`,
     `TOP_PGPIO_FUNC = 0xA8`, `GPIO_BASE`, `GPIO_P_DATA = 0x04`, `GPIO_P_DIR = 0x34`,
     `GPIO_P_SET = 0x64`, `GPIO_P_CLR = 0x94`, `PAD_BASE`, `PAD_P1_STATUS = 0x210`,
     `PWM_PROBE_DIVIDER = 119`.
   - Pure helpers with the documented example asserted: `pwm_period_words(rise, fall, base)`
     (1500 us @ 20000 → `0x0020DC00`, `0x004E0500`) and `cg_divider_word(current, channel, field)`.
   - `read_modify_write(console, prompt, address, clear_mask, set_mask) -> (before, after)` over
     `md.l` + `mw.l` via `read_register`. **Every CG and TOP write goes through it.**
   - `pwm_survey(console, prompt) -> dict`: read-only `md.l` of TOP+0x18, TOP+0xA8, TOP+0x1C,
     CG+0x30, CG+0x84, CG+0x8C, CG+0xA4, PWM+0x100, GPIO+0x04, PAD+0x210; prints
     `[pwm] <name> = 0x…`. Asserts the **core-rail invariants** before any write: CG+0xA4 bit 0
     == 1, CG+0x8C bit 22 == 1, TOP+0x1C[19:16] == 1, PWM+0x100 bit 12 == 1; mismatch → `fail`
     without writing.
   - `gpio_probe(console, prompt, channel, cycles, hold)` (`--gpio-probe CH`): mux P_GPIO[ch] to
     GPIO (TOP+0xA8 |= bit), output (GPIO+0x34 |= bit), alternate GPIO+0x64 / GPIO+0x94 with
     `sleep <hold>` between, printing `[gpio] P_GPIO<ch> high` / `low` for the meter; restore.
   - `pwm_probe(console, prompt, channel, period_us, pulses, hold, samples)` (`--pwm-probe`,
     `--pwm-channel` 0..5 default 0, `--pwm-period-us` default 20000, `--pwm-pulse-us` default
     `1000,1500,2000`, `--pwm-hold-seconds` default 8, `--pwm-samples` default 64):
     1. `reach_uboot`, `pwm_survey`.
     2. Program raw, in order (never `pwm config`): CG+0x84 |= bit ch; CG+0x30 field = 119;
        PWM+0x104 = bit ch; PWM+CTRL(ch) = 0; PERIOD/EXT for the first pulse; TOP+0x18 field ch =
        1; TOP+0xA8 &= ~bit ch; PWM+0x100 = bit ch. Read back and print `check pwm_enable = ok`,
        `check pwm_period = ok`, `check pwm_ext = ok`.
     3. Per pulse: PERIOD+EXT, PWM+0x108 = bit ch, `[pwm] pulse_us=<n> hold=<s>s: observe now`,
        `sleep <hold>`, then sample GPIO+0x04 bit ch `samples` times → `[pwm] gpio_data
        samples=<n> high=<k>`.
     4. Disable (PWM+0x104 = bit ch, readback → `check pwm_disable = ok`), restore TOP/CG words to
        the surveyed values, re-check the invariants, U-Boot `reset`, `wait_for_banner`.
     5. `--transcript` writes the raw capture (the existing `finally` does it for every mode).
   - `--dry-run`: print the exact command sequence with RMW steps marked; paste into the PR.
   - Module docstring: the two bench modes and the never-touch set.
4. **Pins.** `sel4/pins.toml [ns02201_h1v1]`: `pwm_base`, `pinmux_top_base`, `gpio_base`,
   `pad_base`, `pwm_clock_divider = 119`, `pwm_clock_hz = 1000000`; observed keys absent until
   observed (the `reset_write_width` convention). `check-sel4-pins.py` H1V1 block asserts
   `pwm_clock_hz == 120_000_000 // (pwm_clock_divider + 1)` and the bases.
5. Host gates: `just ruff`, `just typos`, `just sel4_pin_check`, `just sel4_gate_control_check`
   (47 gates, `nt98690_boot` still 25), `just devlog_check`, `just tasks_check`.

## B2. Board session

| Step | Command / action | Observation to record |
|---|---|---|
| 1 | Meter, board off: orient the header from the known console pads P_GPIO[32..33] | header candidates |
| 2 | `--survey`, then `--gpio-probe 0` (then 1..5 as needed) with the meter on candidates | **which P_GPIO reaches which header pin**; the pinmux/clock reset state |
| 3 | Servo tester on that pin + GND; `--pwm-probe --pwm-channel <ch> --transcript devlog/…/pwm-probe.log` | servo follows 1000 → 1500 → 2000 us (operator observation); `check … = ok`; `gpio_data … high=<k>` |
| 4 | ESC + motor (props off, own battery, common ground); `--pwm-probe --pwm-pulse-us 1000,1600,1200,1000 --pwm-hold-seconds 5` | ESC arms at 1000 us; runs fast at 1600, slower at 1200, stops (operator observation) |
| 5 | Vendor banner after `reset`; full power cycle; banner again | the board is unharmed |

Budget: two power cycles (steps 2–3, then 4–5).

## B3. Records after the session

- `sel4/pins.toml`: `pwm_channel`, `pwm_pad`, `pwm_pad_header`, `pwm_pinmux_at_prompt`,
  `gpio_data_reflects_function_pad`, `pwm_probe_observed = "2026-09-XX"`.
- A dated `h1v1-pwm-probe` devlog entry, Kind `Audit`, with `pwm-probe.log`,
  `gpio-probe.log`, an Investigation-log row per B2 step, operator observations labelled as such,
  and the decision on pad sampling. Append a `## Corrections` row to the lane Decision entry
  with the pinned channel.
- Close the S1 work item with the observed exit condition; update the memory file.

## B4. Stop conditions (Defect entries, not improvisation)

- A core-rail invariant fails at survey → stop, write nothing, record the values.
- No P_GPIO[0..5] reaches a header → record; evaluate D_GPIO[2..3] (PWM4/5 `_2`, 3.3 V,
  TOP+0xB4) or a soldered lead; DSI_GPIO alternates are 1.8 V and sensor-bound (avoid).
- Readbacks correct but no servo motion → check TOP+0xA8, `--gpio-probe` the same pad, try the
  invert bit; do not proceed to the ESC.
- Banner missing after `reset`, or vendor Linux misbehaves after the power cycle → compare the
  before/after survey; no further writes until explained.

---

# Part C — Sessions 2 and 3

## C0. Design decisions (with the rejected alternative)

| Decision | Choice | Rejected |
|---|---|---|
| Declared device in the root (**S2, general**) | `AuthorityInventory::declared(region: DeviceRegion) -> AuthorityInventory` in `slime-root/src/graph_runtime/platform.rs`: installs one pre-carved region as `regions[0]`, `devices[0] = AuthorityDevice { region: 0, offset: 0 }`, `len = 1`; the caller prints `SLIME_ROOT io authority inventory devices=1 mode=declared`. cfg-free, covered by a host unit test (so it is not dead code in the general PR). `MAX_IO_DEVICES` stays 2. | Generalising `AuthorityDevice` with paddr/size/irq: nothing consumes them. |
| Feeding it on the H1V1 (**S3, board**) | `main.rs`: a static `NS02201_PWM_REGION: Option<DeviceRegion>` filled in the existing carve block (`:791-825`), grown to **TOP → CG → WDT → PWM** in ascending paddr, before the UART carve; `take()`n at `:1038` under `slime_ns02201_pwm` in place of `probe_authority_devices`. Admission decides whether an inventory is needed only after the UART carve, so a static hand-off is the only ordering that satisfies both. | Hoisting admission above the carves. |
| Board facts (**S3**) | pins → `build-sel4.py` exports `SLIME_NS02201_PWM_PADDR`, `SLIME_NS02201_PINMUX_PADDR`, `SLIME_NS02201_PWM_CHANNEL`, `SLIME_NS02201_PWM_DIVIDER` only for `--pwm-graph` on the H1V1 → `build.rs` emits `slime_ns02201_pwm` when the paddr is set → `const_parse_hex_usize(env!(…))`. The `sel4` H1V1 image keeps its exact carve order; P6.C untouched. | A const table in the root. |
| Clock gate and pinmux (**S3**) | Root-side, once, at carve time: RMW CG+0x30 field = divider, CG+0x84 bit ch, TOP+0x18 field ch = 1, TOP+0xA8 bit ch = 0; print `SLIME_NT98690 pwm bringup ch=0 pad=P_GPIO0 clk_hz=1000000`; never read the PWM page afterwards (its frame goes to the driver by `map_child`). | Mediated driver writes to CG/TOP: SoC-wide pages for four one-time words. |
| Known limit (devlog Decisions, S2 driver spec) | PWM page 0 also holds channels 8–12 and the shared ENABLE/DISABLE/LOAD words; driver policy (bits 0–7, CTRL/PERIOD/EXT of ch 0–7 only) is the only guard. | — |
| Composition (**S2**) | `contracts/system-spec/v1/systems/sel4-pwm.zti` = `sel4` product graph + `nvt-pwm-driver` + `slisp-pwm-rpc`, selected by `build-sel4.py --pwm-graph` (`VARIANT_MANIFESTS["pwm"] = "sel4-pwm"`, own target dir). Boots on QEMU: the budget makes the root scan virtio, it finds nothing, `io_device_bind` fails, the driver prints `[nvt-pwm-driver] device absent, refusing requests` and stays resident answering `STATUS_NO_DEVICE`; health green; `sel4_boot_layout_check` boots the plane against a frozen fixture. | Extending `sel4`: every QEMU product gate would depend on a device QEMU lacks; P6.C's frozen counts would change. A QEMU marker gate for `sel4-pwm`: one checker per composition is forbidden. |
| Wrong-device guard (**S2**) | Before any write the driver reads offset 0 of its page; the virtio magic `0x74726976` → refuse as `device absent`. | Trusting the ordinal alone. |
| Contract (**S2**) | `contracts/pwm-servo/v1/{schema.zt,gen_rust.zt}` rendering `components/proto/src/pwm_servo.rs` **and** `components/runtime/include/slime/pwm_servo.h` (the `contracts/spawn/v1` pattern). Request 32 B `{magic, version, channel u32, period_us u32, pulse_us u32, flags u32, reserved 8}`; reply 16 B `{magic, version, status i32, detail u32}` (detail = PERIOD word read back). Constants `MAX_CHANNEL = 5`, `MIN/MAX_PERIOD_US = 2000/20000`, `MIN/MAX_PULSE_US = 1000/2000`, `PULSE_DISABLE = 0`, `CLOCK_HZ = 1000000`, `FAILSAFE_NS`; statuses `OK, BAD_CHANNEL, BAD_PERIOD, BAD_PULSE, NO_DEVICE, DEVICE_ERROR`. | A generic actuator/GPIO protocol (`roadmap/11-io-substrate.md` wants device-specific ones). |
| Driver (**S2**) | `components/services/nvt-pwm-driver`, slots PEER 0, DEVICE 1, MMIO 2 (`componentAbi`); start = bind, `io_mmio_map(1, 2, epoch, 0x20_0000_0000, 0, 0x1000)`, guard, `[nvt-pwm-driver] ready clk_hz=1000000`; per request validate → CTRL 0, PERIOD, EXT → ENABLE (first) or LOAD (update) → read back and print `[nvt-pwm-driver] request ch=0 period_us=20000 pulse_us=1600`, `[nvt-pwm-driver] check enable ch=0 = ok`, `[nvt-pwm-driver] check period = 0x00204000 ok`, `[nvt-pwm-driver] check ext = 0x004e0600 ok`; disable prints `check enable ch=0 = off`. **Failsafe**: `clockAuthority` row (`monotonicRead`); no command within `FAILSAFE_NS` while a pulse above idle is live → program 1000 us, print `[nvt-pwm-driver] failsafe ch=0 pulse_us=1000`. | A yield-counted timeout (not a time); no failsafe. |
| Slisp (**S2**) | Builtin `(pwm ch us)` / `(pwm ch us period_us)` over slot 3 (`PWM_SLOT 3U`): `slisp.h` `SLISP_EFFECT_PWM`; `slisp.c` `pwm_effect()` beside `spawn_effect()`; `main.c` `pwm_command()` beside `spawn_command()` and an `evaluate_line` arm printing `=> pwm 0 1600` or `! pwm <status>`. One pinned 32-byte vector asserted on both sides: `components/proto/tests/pwm_servo.rs` decodes it, `host_main.c` encodes it. | A spawnable applet: 16-byte binding names, no arguments. |
| Board gate (**S3**) | Own checker `scripts/check/check-nt98690-pwm.py` (own image, own marker contract, own session) in `GATES`; recipe `nt98690_pwm_check serial="": slisp_core_check sel4_boot_layout_check`. | A mode of the Slisp checker: one pinned contract per module. |
| Records | IO-track milestone (closes at S2 on QEMU evidence + tests); **P6.D** (closes at S3), P6's "nothing else" boundary amended to name one PWM channel (`roadmap/07-architecture-portability.md` P6 status; `roadmap/README.md` L35/L52). | — |

## C1. Session 2 — general: mechanism, contract, driver, Slisp, composition (QEMU green)

Nothing in this PR names the NT98690, `ns02201`, or a pin. Branch `feat/pwm-servo`.

**Root** (`slime-root/src/graph_runtime/platform.rs`): `AuthorityInventory::declared(region)`
with a host test in the module (bump the B23 count in `just/quality.just` from 211); no
`main.rs` change. Check `install_driver`'s unconditional `grant_irq_source` against
`irqSources = 0`; if refused, budget `irqSources = 1` (inert without an `interruptSource` grant).

**Contract**: `contracts/pwm-servo/v1/{schema.zt,gen_rust.zt}`;
`scripts/generate/generate-pwm-servo-bindings.py`; `just pwm_servo_gen` in `just/generate.just`;
the `--check` line in `contracts_check`; `components/proto/src/lib.rs` (`pub mod pwm_servo`,
`valid_pwm_servo_request`); `components/proto/tests/pwm_servo.rs`;
`components/runtime/include/slime/pwm_servo.h` (generated).

**Driver**: `components/services/nvt-pwm-driver/{Cargo.toml,build.rs,src/main.rs}` (register
layout module-documented against A2; policy: channels 0–5, bits 0–7 only); root `Cargo.toml`
stanza; `contracts/component-spec/v1/components/nvt-pwm-driver.zti` (service; requires `device`,
`endpoint`, `mmioRegion`; `runtime.devices = ["device"]`; health required).

**Slisp**: `components/slisp/{slisp.h,slisp.c,main.c,host_main.c}` per C0.

**Composition**: `contracts/system-spec/v1/systems/sel4-pwm.zti` (component `nvt-pwm-driver`,
owner init, health required, slisp depends on it; grants `init-nvt-pwm-driver` executable,
`pwm-block-device` device/mapMmio, `pwm-block-mmio` mmioRegion/mapMmio, `slisp-pwm-rpc` endpoint
send/recv; slot pins init `bootLayout`, slisp:3 and driver 0/1/2 `componentAbi`;
`ioResourceBudget = [{ holder = "nvt-pwm-driver"; mmioBytes = 4096; mmioMappings = 1;
irqSources = 0; dmaPages = 0; dmaMappings = 0; outstandingRequests = 0; bufferLoans = 0;
device = 0 }]`, `ioResourceBudgetObject = true`; `clockAuthority` row + `clockAuthorityObject =
true`). Regenerate `compositions/sel4-pwm.zti`; bless `baselines/sel4-pwm.zti`;
`scripts/lib/system_spec.py` `DERIVED_GENERATION_FIXTURES`; inventory row with
`owningGate = "sel4_boot_layout_check"` (S3 re-points it); `scripts/check/check-sel4-boot-layout.py`
`PLANES` row + fixture via `just sel4_boot_layout_bless`. `build-sel4.py`: `PWM_VARIANT` /
`--pwm-graph`, admitted wherever `variant == GRAPH_VARIANT` gates product behaviour (`:905-916`,
`:930-942`, `:1326-1334`); verify the boot-layout QEMU invocation attaches no virtio device.

**Records**: `myque new` the IO-track item (parent: the lane item from S1) and P6.D (`depends`
on it); `roadmap/11-io-substrate.md` short item; a dated `pwm-servo-mechanism` devlog entry,
Kind `Change` with the QEMU `sel4-pwm` transcript (`device absent`, healthy) as evidence.

**Verification order**: `just pwm_servo_gen` → `contracts_check` → `component_spec_check`,
`component_crate_split_check` → `generate-generation-from-spec.py --check` →
`composition_inventory_check` → `fmt_check_all`, `lint_all`, `ruff`, `typos` → `test_host` →
`test_sel4_root` (new count) → `slisp_core_check` → `sel4_component_graph_check` (markers
unchanged) → `sel4_boot_layout_check` (new row) → `generation_check` →
`io_driver_authority_check`, `sel4_root_boot_check` → `devlog_check`, `tasks_check`.

## C2. Session 3 — H1V1: bring-up, build inputs, board gate, observed exit

Branch `feat/nt98690-pwm`. Depends on PR-S1 (pins) and PR-S2 (mechanism).

**Build inputs**: `build-sel4.py` exports the four `SLIME_NS02201_PWM_*` env vars for H1V1 +
`--pwm-graph`; `slime-root/build.rs` `rustc-check-cfg` + `rustc-cfg=slime_ns02201_pwm` +
`rustc-env` pass-through; `check-sel4-pins.py` consumes S1's keys.

**Root** (`slime-root/src/main.rs`, all under `all(slime_ns02201_h1v1, slime_ns02201_pwm)`):
statics `NS02201_PINMUX_PAGE`, `NS02201_PWM_PAGE`, `NS02201_PWM_REGION`; the carve loop at `:791`
grows to TOP/CG/WDT/PWM; `ns02201_pwm_bringup(cg, top)` + marker; at `:1038` the arm calling
`AuthorityInventory::declared(region)`. Pure helpers `pwm_group_divider_word(current, ch,
divider)` and `pinmux_words(top18, topa8, ch)` in `platform_timer.rs` (beside the reset helper)
with two host tests.

**Gate** `scripts/check/check-nt98690-pwm.py`: the shape of `check-nt98690-slisp.py`
(`build_artifacts` with `--pwm-graph`, stem `slime-sel4-pwm-ns02201-h1v1-test-terminator`,
`check_identity`, `stage_and_launch`, `read_until`, `type_command`, `contract_view`).
`SESSION_COMMANDS = ("(pwm 0 1000)\n", "(pwm 0 1600)\n", "(pwm 0 1200)\n", "(pwm 0 0)\n")` with
`HOLD_SECONDS = 3.0` after each `=>` line so the operator sees arm → fast → slow → stop (inside
the 10 s failsafe). Markers, within `literal_for`'s grammar and **frozen against a fresh QEMU
`sel4-pwm` transcript, never assumed**: P6.A handoff; `Booting all finished, dropped to user
space`; `SLIME_NT98690 pwm bringup ch=0 pad=P_GPIO0 clk_hz=1000000`; `SLIME_ROOT io authority
inventory devices=1 mode=declared`; `SLIME_ROOT product input ready uart=0x2f0130000`;
`SLIME_ROOT generation admitted number=\d+ executables=<n> instances=<n> grants=<n> `;
`SLIME_IO quota task=\d+ instance=nvt-pwm-driver devices=1 shared_granule=0`;
`\[nvt-pwm-driver\] ready clk_hz=1000000`; `SLIME_GRAPH healthy … failed=0`;
`SLIME_ROOT READY target_profile=aarch64-sel4-nt98690-h1v1`; `slisp> `; `\[slisp\] resident
input wait`; per command: `\(pwm 0 <us>\)\n\[nvt-pwm-driver\] request ch=0 period_us=20000
pulse_us=<us>`, the three `check` lines (or `check enable ch=0 = off` for 0), `=> pwm 0 <us>`;
`SLIME_NT98690 test terminator accepted`; `SLIME_NT98690 reset request kind=wdt`;
`U-Boot 2021\.10` (~34). Failure markers: P6.C's set plus `\[nvt-pwm-driver\] fail .*`,
`\[nvt-pwm-driver\] failsafe .*`, `\[nvt-pwm-driver\] device absent.*`, `! pwm .*`. Evidence
`build/nt98690-pwm-evidence/{pwm-session.log,pwm-identities.json}`. `GATES` row with its exact
count; `just/hardware.just` `nt98690_pwm_check` with the operator's counted steps in its comment
(power on when told; ESC signal + GND on the pinned pad; props off); `sel4/pins.toml boot_files`
+ `check-sel4-pins.py` list gain the stem; inventory `owningGate` → `nt98690_pwm_check`.

**Exit condition (serial-observable)**: after each of `(pwm 0 1000)`, `(pwm 0 1600)`,
`(pwm 0 1200)` the driver reads back ENABLE bit 0 set and the PERIOD/EXT words A2 lists for that
pulse; after `(pwm 0 0)` ENABLE bit 0 clear; terminator → watchdog reset → banner; 0 framing
errors; one power-on. The motor arming, running fast, then slow is a **Verification-table row of
class `Operator observation [INFERENCE]`**, never a marker.

**Records**: `roadmap/07-architecture-portability.md` P6.D section (Deliverables, Required
checks, Verification target, Exit condition, then `**Exit condition (observed):**`) and the
amended boundary sentences; a dated `p6d-nt98690-pwm` devlog entry, Kind `Change`, Status
`Verified`, with transcript + identities; `myque close` P6.D with the observed exit; `devlog/README.md`
rows; memory file.

**Verification order**: `sel4_gate_control_check` (the new row rejects mutations) →
`sel4_nt98690_image_check`, `build-sel4.py --platform ns02201-h1v1 --pwm-graph --test-terminator`
→ `nt98690_slisp_check <serial>` (P6.C still green with the new carve order) →
`nt98690_pwm_check <serial>` → `devlog_check`, `tasks_check`.

## C3. Risks and stop conditions (S2/S3)

- `map_child` evicts the root's mapping of the PWM page: all root bring-up is on CG/TOP only,
  before launch.
- A carve-order mistake loses the reset path exactly as P6.C observed
  (`Allocate(DeviceFramePassed)`): `nt98690_slisp_check` must stay green; if it regresses, stop.
- S1 findings override pins: an unreachable `P_GPIO0` changes only `pwm_channel`/`pwm_pad`/marker
  text; a pinmux write with no effect from the root stops the lane before the driver session.
- Failsafe window vs gate timing: each next command must follow inside `FAILSAFE_NS`; a fired
  failsafe is a failure marker.
- Stop conditions: `SLIME_ROOT FATAL` on the board image; an ENABLE readback not matching the
  written bit; a QEMU `sel4-pwm` boot where the driver binds *anything* (the guard prints
  `device absent` on QEMU by design).
