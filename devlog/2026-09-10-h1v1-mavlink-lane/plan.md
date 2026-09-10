# H1V1 MAVLink heartbeat lane — meta plan (Part A), Session 1 (Part B), Sessions 2–3 (Part C)

Drafted 2026-09-10. Companion to the ESC lane's plan of record
(`devlog/2026-09-07-h1v1-esc-lane/plan.md`). Same shape, same conventions: Part A is the durable
half a later session re-verifies one line at a time instead of re-exploring; Part B is the
executable next session; Part C outlines the two after it. Every fact cites a source, and a fact
no source on disk establishes says so.

## Context

The H1V1 answers typed Slisp input over UART0 (P6.C) and its bench probe has driven an ESC from
the vendor prompt (P6.PWM.A, reopened pending one readback run). The next physical ask is
outward telemetry: Slime OS emitting a MAVLink v2 HEARTBEAT once per second through an RFD900x
telemetry radio, so a ground station sees the board as a live MAVLink system. Transmit only, at
the radio's default 57600 baud. Receiving commands back is a separate later lane and nothing here
plans it.

"Radio on GPIO" means the SoC's UART7, whose transmit pad is P_GPIO[8] at pin 13 of the 40-pin
header, on the same 3.3 V bank as the ESC lane's pad. Nothing is bit-banged. UART8 was the first
choice and was withdrawn on 2026-09-10 when the board's pinout diagram showed its data pads,
P_GPIO[4..5], reach no connector; only that port's flow-control pins do. Every UART7 fact below
was re-derived from the same sources the UART7 facts came from.

The lane is cut the way the ESC lane is, and for the same reason (upstream wants the general
half reviewable without the board riding along):

| Session | Branch | Scope | Closes |
|---|---|---|---|
| S1 | `feat/nt98690-uart-probe` | Board-only bench probe from the vendor U-Boot prompt: find the pad, survey the clock/pinmux/UART state, transmit pinned heartbeats, decode them on the host through the paired radio, restore, reset | `P6.MAV.A` (bench) |
| S2 | `feat/serial-tx` | Board-neutral: `serial-device/v1` and `mavlink-heartbeat/v1` contracts, a 16550 TX driver, a heartbeat producer paced by the clock authority, the `sel4-mavlink` composition, QEMU evidence only. Names no SoC. | `IO9` (IO track) |
| S3 | `feat/nt98690-mavlink` | Board: UART7 carve and clock/pinmux bring-up from pins, build inputs, the board gate whose exit is decoded on the ground radio | `P6.E` (board) |

S1 is independent of the ESC lane and can share the next board session with the `--pwm-probe`
readback rerun. S2 depends on the ESC lane's S2 (`IO8`, the declared-device mechanism) and is
its second consumer. S3 depends on S1 (pins) and S2 (mechanism) and shares the root carve block
with the ESC lane's S3, so whichever lands second rebases.

---

# Part A — Meta plan (durable)

## A1. Goal and exit conditions

**Goal.** On the named H1V1, from one power-on, with the RFD900x's transmit-side radio wired to
UART7 TX and a second RFD900x on the development host: the resident Slime graph emits one
MAVLink v2 HEARTBEAT per second and the host decodes them with valid CRCs and consecutive
sequence numbers, with no operator in the loop.

**Exit, S1 (bench, serial-observable on both ports).** From the unmodified vendor U-Boot: the
UART7 clock, reset, divider, pinmux and pad words read their surveyed values; the port takes the
divisor and line settings and reads them back; N pinned heartbeat frames written to the transmit
holding register are each decoded on the host radio's serial port within a bounded time, CRC
valid, sequence consecutive; every shared word the probe changed is written back to its surveyed
value **and read back**; `reset` returns the vendor banner. The pad, header pin, and every
surveyed word are pinned in `sel4/pins.toml`.

**Exit, S2 (QEMU).** The `sel4-mavlink` composition admits; the driver binds no device and stays
resident answering `NO_DEVICE`; the heartbeat component keeps its 1 Hz cadence and reports each
refused send; graph health stays green; the plane's boot layout matches its frozen fixture. On
the host: the Rust encoder reproduces the pinned frame vector byte for byte, the Python decoder
accepts it, and the declared-device inventory is covered by a root unit test (shared with IO8).

**Exit, S3 (board, serial-observable).** After `SLIME_ROOT READY`, the driver's ready marker
names the clock and divisor it read back; the heartbeat component's per-send marker shows
`status=ok` with a consecutive sequence; the host radio port decodes ≥ 10 consecutive heartbeats
with valid CRC and an inter-arrival time within 1 s ± 250 ms; the gate-only terminator returns
the board to its vendor firmware; zero framing errors on UART0. No operator observation is in the
exit condition at all — this lane's actuator is a receiver the checker owns.

## A2. Hardware facts — sources verified 2026-09-10

The ESC plan's A2 table already establishes the TOP, CG, GPIO, and PAD block bases, the PWM12
core-rail words that must never be written, the P_GPIO GPIO/FUNCTION word `TOP+0xA8`
(bit = pad, 0 = FUNCTION), and the carve-order rule (device untyped retypes monotonically upward).
This table adds only what UART7 needs.

| Fact | Value | Source |
|---|---|---|
| UART7 is a 16550 | `uart@2,f0136000`, `compatible = "ns16550a"`, `reg = <0x2 0xf0136000 0x0 0x1000>`, `reg-shift = 2`, `reg-io-width = 4`, `clock-frequency = 48000000` | decompiled board DTB `nvt-evb.bin` (`/srv/novatek/sdk/worktrees/lamb-h1v1/output/`), the `uart@2,f0136000` node |
| Interrupt | `interrupts = <0 0x5a 4>` → SPI 90 → GIC INTID 122 (UART0 is SPI 43, INTID 75). Unused by a TX-only lane | same DTB |
| UART7 clock | DT `clk_uart6`: `div_reg_offset CG_UART_CLK_DIV_REG1_OFFSET` (0x60), `div_bit_idx 8`, `div_bit_width 8`, `gate_reg_offset CG_CLK_EN_REG3_OFFSET` (0x7C), `gate_bit_idx 22`, `reset_reg_offset CG_SYS_RESET_REG3_OFFSET` (0x9C), `reset_bit_idx 22`, `current_rate 48000000`, same 480 MHz parent | `configs/.../nvt-clock.dtsi` `clk_uart6` node; offsets `include/dt-bindings/clock/nvt-ns02201.h:80,89,107` |
| Naming trap | DT `uart6` / `clk_uart6` at `0x2f0136000` **is** the SoC's "UART7" (the pinmux and U-Boot name); DT `uart7` at `0x2f0137000` is SoC UART8. In general SoC UARTn is DT uart(n−1), there being no SoC UART1. This plan uses the SoC name UART7 throughout | `configs/.../nvt-peri.dtsi:23-24,145-173`; U-Boot `soc.c:286-308` (below) |
| CG base | `0x2_F002_0000` (already `reset_cg_base` in pins) | `u-boot/arch/arm/include/asm/arch-nvt-ns02201_a64/IOAddress.h:20,40` |
| UART7 clock source | parent phandle `0x106` = `fix480m`, **480 MHz** fixed | `h1v1.dts:2730-2735`; `configs/.../nvt-clock.dtsi:92-97` |
| UART7 clock divider | `CG+0x60` bits **[15:8]** (`CG_UART_CLK_DIV_REG1_OFFSET`); the field holds **(divisor − 1)**: rate = 480 MHz / (field + 1). For 48 MHz the field is **9**. Its value at the U-Boot prompt is **not established on disk** (no U-Boot or TF-A code writes `CG+0x60`; the loader is a blob) — the probe writes 9 rather than trusting what it finds, and restores the surveyed value afterwards | `linux-kernel/include/dt-bindings/clock/nvt-ns02201.h:80`; `nvt-clock.dtsi:1070-1078`; `drivers/clk/novatek/nvt-clk-provider.c:912-915` (set), `:857-862` (recalc) |
| UART7 clock gate | `CG+0x7C` bit **22** (`CG_CLK_EN_REG3_OFFSET`); **1 = clock on** | `nvt-ns02201.h:89`; `nvt-clock.dtsi:1076-1077`; `nvt-clk-provider.c:726-733` |
| UART7 reset | `CG+0x9C` bit **22** (`CG_SYS_RESET_REG3_OFFSET`); **active-low RSTN: 1 = released, 0 = held in reset**. The DT says `do_reset = NOT_RESET`, so Linux only ever sets the bit; the probe does the same (set if clear, never pulse) | `nvt-ns02201.h:107`; `nvt-clock.dtsi:1078-1080`; `nvt-clk-provider.c:651-666, 687-694` |
| Pinmux for UART7 → P_GPIO[8..9] | `TOP_REG13_OFS = 0x34` ("TOP Control Register 13 (UART[1/2])"), field `UART7` = bits **[27:24]** (fields are 4 bits each from bit 0: `UART`, `UART2` … `UART7`); `MUX_1 = 1` routes UART7_1 = P_GPIO[8] TX, P_GPIO[9] RX; 0 = unrouted. `TOP_REG14_OFS = 0x38`, field `UART7_RTSCTS` = bits **[17:16]** — **never written** (no flow control) | `linux-kernel/drivers/soc/nvt/plat-novatek/include/plat-ns02201_a64/top_reg.h:20-22, 294-328`; `include/dt-bindings/pinctrl/nvt-ns02201-pinctrl.h:650-651, 662-663`; `ns02201_pinmux_host.c:7167-7212` |
| What the Linux pinmux driver writes for a `_1` UART route | **Exactly three fields**: the port's `TOP+0x34` field = 1 and the two pads' `TOP+0xA8` bits = 0 (for UART7: `[27:24]`, bits 8 and 9). No pad, pull, drive, or other register | `ns02201_pinmux_host.c:7167-7212, 11552-11565` |
| The vendor's own U-Boot recipe for UART7_1 (never compiled in, but authoritative) | `TOP+0xA8 &= ~0x300` (bits 8,9 → FUNCTION); `TOP+0x34 = (v & ~0xF000000) \| 0x1000000` (UART7 = MUX_1); `CG+0x7C \|= 0x400000` (bit 22, gate on). Then the generic ns16550 init | `u-boot/arch/arm/mach-novatek/nvt_ns02201_a64/soc.c:262-275` (`serial_preinit`'s `COM7` block, called from nowhere in the tree) |
| Package pins | P_GPIO8 = `UART7_1_TX` (O), P_GPIO9 = `UART7_1_RX` (I), P_GPIO10/11 = RTS/CTS (optional); all `3318 IO` (3.3 V), default pull-down; INT[66..69]. Their other alternates (PWM8/9, I2C19, SIF5, a sensor port) are claimed by nothing on this board | pinmux CSV rows 115–118; header positions 13, 15, 3, 5 from `header_pins` |
| Pad word for P_GPIO[8] | `TOP+0xA8` (`TOP_REGPGPIO0_OFS`, one bit per pad, `PGPIO_8` = bit 8, `PGPIO_9` = bit 9; `0 = FUNCTION`, `1 = GPIO`). **Only bit 8 is cleared**: the TX pad is routed, P_GPIO[9] stays GPIO (no RX). With the RX function input unrouted the 16550 may report `LSR.BI`/`DR`; the probe and driver mask LSR to THRE (bit 5) and TEMT (bit 6) | `top_reg.h:35-41, 509-544` |
| Pad electrical for P_GPIO[8] | Pull: `PAD+0x08` bits [17:16] (`PAD_PIN_PGPIO8` = 64 + 16; 0 none, 1 down, 2 up, 3 keeper; board default down). Drive: `PAD+0x110` bits [3:0] of the next word (`PAD_DS_PGPIO8`). Rail: `PAD+0x210` bit 4 = P1 rail, 0 = 3.3 V, board declares `pgpio_0_19` on P1 at 3.3 V; observed `0x00000024` at the prompt (3.3 V). **None of these is written** — a push-pull UART TX needs no pull, and the ESC probe already asserts the rail bit | `plat-ns02201_a64/pad.h:22-57, 111-118, 302-309, 479`; `ns02201_pad_host.c:66-75, 141-161, 405-410`; `configs/.../nvt-peri-dev.dtsi:8`; ESC audit entry |
| GPIO bank words for P_GPIO[8] | Same bank-1 words the PWM probe uses: DATA `GPIO+0x04`, DIR `+0x34`, SET `+0x64`, CLR `+0x94`, bit 8. The second word starts at P_GPIO[32], so `--gpio-probe 8` needs no change | `plat-ns02201_a64/nvt-gpio.h:20-23, 38, 47`; `drivers/gpio/gpio-nvt.c:433-447, 539-559` |
| Header pins | P_GPIO[8] = **pin 13**, P_GPIO[9] = pin 15, P_GPIO[0] = pin 26 (the ESC lane's pad); ground at 6, 9, 14, 20, 25, 30, 34, 39. P_GPIO[4] and [5] are **not on the header**, which is why UART8 was withdrawn. The whole map is pinned as `header_pins` in `sel4/pins.toml`. **S1 confirms pin 13 with `--gpio-probe 8`** rather than searching for it | the board's pinout diagram, operator-supplied, kept as `devlog/2026-09-08-h1v1-pwm-probe/header-pinout.png`; its labels checked against the pinmux CSV (SPI2_2, SPI_1, PWM0 on P_GPIO0) |
| U-Boot and UART7 | U-Boot's serial is the generic `ns16550.c` with `MEM32` (4-byte stride), `COM1..COM5` = `0x2f0130000..0x2f0134000` only, `CONS_INDEX = 1`, `SYS_NS16550_CLK = 48000000`. No `COM6..9`, so no code path touches UART7 (`COM7` = `0x2f0136000` is not defined for this SoC variant); `serial_preinit` is never called; U-Boot's pinmux host is compiled only with `CONFIG_VIDEO_LOGO` (unset). Linux's `uartII{pinmux = <0x0>}` routes no UART6–9 pad. **Expect the gate off, the reset either state, the mux 0, and bits 4/5 of `TOP+0xA8` set at the prompt — unobserved; S1 reads them** | `include/configs/novatek/ns02201_a64.h:19-29`; defconfig `:253, 1180-1193, 209`; `nvt_ns02201_a64/Makefile:14`; `configs/.../nvt-top.dtsi:21-22` |
| U-Boot console init sequence (the 16550 convention to copy) | `ns16550_init`: wait `LSR.TEMT`; `IER = 0x00`; `MCR = 0x03` (DTR\|RTS); `FCR = 0x07` (`FIFO_EN\|RXSR\|TXSR`); `LCR = 0x03` (8N1); then `LCR = 0x83`, `DLL = div & 0xFF`, `DLM = div >> 8`, `LCR = 0x03`. `div = DIV_ROUND_CLOSEST(clock, 16 × baud)`. Registers at index × 4: RBR/THR/DLL `+0x00`, IER/DLM `+0x04`, IIR/FCR `+0x08`, LCR `+0x0C`, MCR `+0x10`, LSR `+0x14`, MSR `+0x18`, SCR `+0x1C`. No vendor-specific register | `u-boot/drivers/serial/ns16550.c:27-40, 212-228, 259-269`; `include/ns16550.h:41-43, 88-119, 146-148` |
| Divisor for 57600 | 48 MHz / (16 × 57600) = 52.08 → `DLL = 52`, `DLM = 0` → 57 692 baud, +0.16 % (16550 tolerance ~2 %). U-Boot's own console divisor at 115200 is 26, the same formula | `ns16550.c:212-217`; arithmetic, computed 2026-09-10 |
| Line settings | 8N1, no flow control: `LCR = 0x83` (DLAB) → `DLL = 52`, `DLM = 0` → `LCR = 0x03`; `FCR = 0x07`; `MCR = 0x00` (U-Boot writes `0x03`; DTR/RTS reach no routed pad, so either is inert — `0x00` is pinned so the value is not mistaken for flow control); `IER = 0x00`. Readback: `LCR`, `DLL`/`DLM` (with DLAB), `MCR`, `IER`; `FCR` is write-only and `IIR` reports bit 7 set once FIFOs are enabled (this UART answers `0x81`, not the datasheet's `0xC1`; observed 2026-09-10) | 16550 register map; U-Boot sequence above; RFD900x default serial format |
| Time on the wire | 21 bytes × 10 bits / 57600 = 3.65 ms per frame; one `mw.l` per byte from the prompt is ~ms apart so the 16-byte TX FIFO never fills in S1; in S3 the driver polls `LSR.THRE` (bit 5) before each byte and `LSR.TEMT` (bit 6) after the frame | arithmetic |
| Core rail | UART7's words (`CG+0x60[15:8]`, `CG+0x7C` bit 22, `CG+0x9C` bit 22, `TOP+0x34[27:24]`, `TOP+0xA8` bit 8) are disjoint from the PWM12 set (`CG+0x8C`, `CG+0xA4`, `CG+0x324`, `TOP+0x1C`, `PWM+0x60/0x64`, `PWM+0x104` bit 12) and from the PWM0 set (`CG+0x30`, `CG+0x84`, `TOP+0x18`, `TOP+0xA8` bit 0). The probe still asserts `CORE_RAIL_INVARIANTS` before its first write and after its last, because CG and TOP are shared pages, and every write is a read-modify-write of one field | ESC plan A2; the offsets above |
| Write list, in order (all RMW with readback) | 1. `CG+0x9C \|= 1<<22` (release reset if held) 2. `CG+0x60[15:8] = 9` 3. `CG+0x7C \|= 1<<22` 4. `TOP+0x34[27:24] = 1` 5. `TOP+0xA8 &= ~(1<<8)` 6. 16550 init on `0x2_F013_6000` (A2 line settings). Restore is 5 → 1 in reverse, each read back | derived from the rows above; vendor `serial_preinit` is the same sequence minus the reset and divider it never had to touch |

## A3. MAVLink and radio facts — verified 2026-09-10

| Fact | Value | Source |
|---|---|---|
| Frame, MAVLink v2, unsigned | `FD len 00 00 seq sysid compid msgid[3, LE] payload[len] ck_lo ck_hi` = 12 + len bytes | MAVLink v2 packet format (mavlink.io/en/guide/serialization.html) |
| HEARTBEAT | msgid 0, payload 9 bytes in wire order (fields sorted by size): `custom_mode u32, type u8, autopilot u8, base_mode u8, system_status u8, mavlink_version u8`; `CRC_EXTRA = 50` | common.xml message 0 |
| Values pinned for this lane | `sysid 1`, `compid 191` (MAV_COMP_ID_ONBOARD_COMPUTER), `type 18` (MAV_TYPE_ONBOARD_CONTROLLER), `autopilot 8` (MAV_AUTOPILOT_INVALID), `base_mode 0`, `custom_mode 0`, `system_status 4` (MAV_STATE_ACTIVE), `mavlink_version 3` | common.xml enums; lane decision (A4) |
| Zero-truncation | v2 may strip trailing zero payload bytes; the last byte here is `mavlink_version = 3`, so `len` is always 9 and nothing is truncated. The decoder still accepts `1 ≤ len ≤ 9` | MAVLink v2 serialization |
| Checksum | CRC-16/MCRF4XX (X.25, init `0xFFFF`, reflected poly `0x8408`, no final XOR) over `len..payload` then over the one byte `CRC_EXTRA`; sent low byte first. Routine validated: `crc(b"123456789") == 0x6F91` | MAVLink `crc_calculate`; check value computed 2026-09-10 |
| **Pinned vector, seq 0** | `fd 09 00 00 00 01 bf 00 00 00 00 00 00 00 12 08 00 04 03 ae c6` | computed 2026-09-10 with the validated routine |
| Pinned vector, seq 1 | `fd 09 00 00 01 01 bf 00 00 00 00 00 00 00 12 08 00 04 03 be 48` | same |
| Pinned vector, seq 255 | `fd 09 00 00 ff 01 bf 00 00 00 00 00 00 00 12 08 00 04 03 b7 7c` | same |
| Sequence | `seq` is a free-running u8 per sender; wraps 255 → 0. The decoder's "consecutive" check is `(seq_n − seq_{n−1}) mod 256 == 1` | MAVLink v2 |
| RFD900x serial side | Default `SERIAL_SPEED = 57` (57600), 8N1, no flow control unless `CTSRTS` is set; TX/RX are 3.3 V CMOS. Power is separate (5 V supply pins) — the board only supplies TX and GND | RFD900x datasheet / SiK parameter set (**vendor document, not on disk**; confirm on the host radio with `ATI5`) |
| RFD900x MAVLink mode | Default `MAVLINK = 1`: the radio packetises on MAVLink frame boundaries and injects `RADIO_STATUS` (msgid 109) into the **ground-side** stream. The host decoder therefore skips non-zero msgids and never asserts that only heartbeats arrive | SiK/RFD firmware behaviour (**vendor document**); decoder decision (A4) |
| Radio pairing | Both radios must share `NETID`, `AIR_SPEED`, `MIN/MAX_FREQ`, `NUM_CHANNELS`, `ECC`, `MAVLINK`, `LBT_RSSI`. Assumed already paired (they were sold/used as a pair); S1 confirms link by the green LED going solid, then by decoded frames | vendor document |
| Host receiver | `pyserial 3.5` is on this host; `pymavlink` is not and is not added. The lane's decoder is ~40 lines of Python in the repo (`scripts/lib/mavlink.py`), reused by the bench probe, its host model, and the S3 gate | checked 2026-09-10 |
| Wiring | Board header pin 13 (P_GPIO[8], UART7 TX) → radio RX; a board GND pin (14 is adjacent) ↔ radio GND; radio VCC from its own 5 V supply; pin 15 (P_GPIO[9]) not connected (no RX). Radio RX is a 3.3 V input, so no level shifting | pinmux CSV (3.3 V bank); radio datasheet |

## A4. Fixed decisions and names

| Decision | Choice | Rejected |
|---|---|---|
| The port | UART7 at `0x2f0136000`, TX on P_GPIO[8] (header pin 13), **TX pad only routed** (P_GPIO[9] stays GPIO, RTS/CTS untouched) | UART8 (first choice): its data pads are not on the header. UART9: its TX pad is P_GPIO[0], the ESC lane's PWM pad, so the two lanes would collide on pin 26. UART0: the kernel debug console and the Slisp input path. Bit-banging a GPIO at 57600 from seL4 userspace: no timing guarantee |
| Line config ownership (S2/S3) | The **driver** programs LCR/DLL/DLM/FCR/MCR on its own mapped page; the **root** programs the SoC-wide words once at carve time (`CG+0x9C` release, `CG+0x60` field = 9, `CG+0x7C` gate, `TOP+0x34` mux, `TOP+0xA8` bit 8) and prints `SLIME_NT98690 uart7 bringup pad=P_GPIO8 clk_hz=48000000` — the PWM lane's split, for the same reason (SoC-wide pages never reach a component) | Mediated driver writes to CG/TOP |
| Divider policy | Write the divider field to 9 (48 MHz) unconditionally, in the probe and in the root, and read it back; never derive the baud from whatever the field held. The prompt-time value is pinned as an observation, not consumed | Trusting the DT's 48 MHz as the live state (nothing on disk shows what the loader leaves in `CG+0x60`) |
| Framing owner | The **heartbeat component** builds the frame (contract `mavlink-heartbeat/v1`, Rust encoder generated from the schema, `seq` state in the component); the **driver** is a byte-transparent bounded TX (`serial-device/v1`). Two contracts, two components | One "radio" component that both frames and drives the UART: couples a protocol to a 16550 |
| Pinned vector as the seam | S1 hardcodes the seq-0/1/255 vectors above as bench data (verification code, not a persisted format); S2 makes `contracts/mavlink-heartbeat/v1/schema.zt` the source of truth and asserts on the host that the generated Rust encoder and `scripts/lib/mavlink.py` both reproduce the same vectors byte for byte | Landing the contract in the board-only S1 (breaks the general-vs-board cut); a hand-packed frame in S2 (violates the Zutai rule) |
| Decoder on the host | `scripts/lib/mavlink.py`: `x25_crc`, `encode_heartbeat(seq, …)`, `FrameDecoder.feed(bytes) -> [Frame]` resynchronising on `0xFD`, verifying CRC with `CRC_EXTRA` **for msgid 0 only** and passing other msgids through unverified (radio `RADIO_STATUS`). No pymavlink | Adding pymavlink to the flake for one message |
| Cadence | 1 Hz from the clock authority's monotonic read, deadline-based (`next = last + 1 s`, not `sleep(1 s)` after each send) so drift does not accumulate; a send that returns `NO_DEVICE`/`DEVICE_ERROR` is reported and the cadence continues. **One second in ticks needs the timer rate, which no syscall exposes today** (`monotonic_read` returns raw `CNTPCT` ticks; the root knows `frequency_hz()` but keeps it): S2 adds `CLOCK_RATE_READ` under the caller's `monotonicRead` authority (C0) | Stopping on first failure (a heartbeat that stops after one refusal never shows up on a QEMU plane); a per-profile tick table in the component (names a SoC in the general session) |
| Register access (S2) | **Mediated only**: `MediatedMmio::read32/write32` on a root-mapped page, as both shipped drivers do. `io_mmio_map` is never called: the root's `map_mmio` re-homes the frame into the child and the root's own view of a declared region would then read the wrong address. Budget `mmioMappings = 0` | Mapping the page into the driver |
| TX blocking | Driver polls `LSR.THRE` per byte with a bounded spin (≥ 16 × byte time = 2.8 ms at 57600, pinned as `TX_POLL_LIMIT`) and returns `TIMEOUT` rather than hang; 21 bytes is 3.65 ms on the wire | IRQ-driven TX: adds an interrupt source for nothing a 1 Hz sender needs |
| QEMU shape (S2) | `sel4-mavlink` = `sel4` product graph + `uart16550-driver` + `mavlink-heartbeat`, selected by `build-sel4.py --mavlink-graph`; boots on QEMU with the budget making the root scan virtio, the driver finding no device (or the wrong-device guard refusing a virtio magic), printing `[uart16550-driver] device absent, refusing requests`, and the heartbeat printing `[mavlink-heartbeat] send seq=0 status=no-device` each second; health green; frozen boot layout | Extending `sel4` (every product gate would carry an absent device); a QEMU marker gate for the plane (one checker per composition is forbidden) |
| Wrong-device guard | Before any write the driver reads offset 0 of its page and refuses if it sees the virtio magic `0x74726976`; a 16550's offset 0 is RBR/THR and reads the RX FIFO | Trusting the ordinal alone |
| Names | Contracts `contracts/serial-device/v1`, `contracts/mavlink-heartbeat/v1`; components `components/services/uart16550-driver`, `components/applications/mavlink-heartbeat`; composition `sel4-mavlink`; markers `SLIME_NT98690 uart7 bringup …`, `[uart16550-driver] ready clk_hz=48000000 divisor=52`, `[mavlink-heartbeat] send seq=<n> status=<ok\|no-device\|device-error\|timeout>`; gate `nt98690_mavlink_check`; pins keys `uart7_*` in `[ns02201_h1v1]`; work items `P6.MAV` (epic), `P6.MAV.A` (bench), `IO9` (mechanism), `P6.E` (board) | `nvt-uart-driver` (the driver is a plain 16550, not Novatek-specific — the SoC facts stay in the root) |
| Roadmap home | `P6.E` under P6 in `roadmap/07-architecture-portability.md` with the "nothing else" boundary amended to name one TX-only UART; `IO9` in `roadmap/11-io-substrate.md` beside IO8 | A new track |
| RX | Out of scope; a later lane (interrupt source, bounded input path separate from the console, validating parser) | — |

## A5. Reuse map (verified paths)

Verified 2026-09-10. The bench-probe paths are at upstream commit `3429307f` (the review-round
head), not the local checkout, which is four commits behind — S1 starts from that commit.

**Bench probe and its host model** (`scripts/check/check-nt98690-boot.py` @3429307f):
- CLI in `main()` :1293 — modes dispatch dry-run → serial-required → monitor → survey →
  reset-probe → gpio-probe → pwm-probe → scored boot. New `--uart-probe` goes between
  gpio-probe and pwm-probe; `--dry-run` learns a `--uart-probe` rendering.
- Serial is **raw termios, not pyserial**: `scripts/lib/uboot_console.py` — `open_serial(device,
  baud, fail)` :53 (`O_NONBLOCK`, `PARMRK|INPCK` so framing errors count), `Console(endpoint, baud,
  fail)` :102 (also `tcp:HOST:PORT`), `Console.write(data, timeout=10.0)` :173,
  `Console.read_for(seconds) -> str` :199, `flush_input()` :228, `received_text()` :137;
  free functions `reach_uboot` :233, `send_command(console, command, prompt, timeout, fail)` :317,
  `report_transcript` :342. The policy is stated in `scripts/check/check-rpi5-boot.py:22-24`
  (no pyserial: reproducibility). **Gap:** `read_for` returns decoded text; the radio receiver
  needs bytes. S1 adds a byte-preserving read to the same module (see B1) rather than a second
  serial stack.
- Register helpers: `read_words(output, count)` :593, `read_register(output, address)` :731,
  `read_one(console, prompt, address)` :860 (`md.l addr 1`), `verify_register(console, prompt,
  address, expected, mask)` :868, `write_and_verify(...)` :880, `read_modify_write(console,
  prompt, address, clear_mask, set_mask) -> (before, after)` :887, `check_core_rail(console,
  prompt, when)` :900 over `CORE_RAIL_INVARIANTS` :305, `drive_gpio_level` :926, `gpio_probe`
  :944 (snapshot → toggle → `finally` cleanup chain through `run_cleanup_step` :474 → rail
  recheck), `pwm_probe_plan` :773 (the dry-run renderer), `pwm_probe` :1044 (survey → rail →
  writes → holds → `finally` cleanup with verified restoration → `reset` → `wait_for_banner`;
  result line :1244; pass only if `channel_disabled`, `settings_restored`, `firmware_banner` all
  true :1263). Constants block :258-296 (`CG_BASE = 0x2_F002_0000`, `TOP_BASE = 0x2_F001_0000`,
  `GPIO_BASE = 0x2_F004_0000`, `PAD_BASE = 0x2_F003_0000`, `TOP_PGPIO_FUNC = 0xA8`, GPIO
  `P_DATA 0x04 / P_DIR 0x34 / P_SET 0x64 / P_CLR 0x94`).
- Only `md.l`, `mw.l`, `reset` are spoken to the board; holds are host-side `time.sleep`.
- Host model `scripts/check/check-nt98690-bench-probe.py` @3429307f: `ModelConsole` :41 with
  `_execute` :105 interpreting `md.l`/`mw.l`/`sleep` and W1S/W1C words :124-133,
  `initial_registers()` :142, `run_pwm` :158 (monkeypatches `PROBE.time.sleep`),
  `assert_restored` :181, `assert_core_rail_untouched` :195, `expect_failure` :216,
  `BoardOpened` :21; 15 scenarios :539-556; recipe `nt98690_bench_probe_check`
  (`just/hardware.just:136`). S1 adds a 16550 model (THR/LSR) and `uart_*` scenarios here.
- Pins checker `scripts/check/check-sel4-pins.py` @3429307f: H1V1 block inside `check_profile`
  :376 (`h1v1 = table(pins, "ns02201_h1v1")` :481); UART0 identity pinned :510-530
  (`serial = "uart0-ns16550a-0x2f0130000"`, `serial_reg_shift = 2`, `serial_reg_io_width = 4`,
  `serial_clock_hz = 48000000`); PWM address/scale block :554-582; `observed_pwm_routes`
  :597 gating `pwm_channel/pwm_pad/pwm_pad_header/pwm_probe_observed` :600-623. `uart7_*`
  keys and an `observed_uart_routes` table follow that pattern. **No key for UART7 or
  `0x2f0136000` exists anywhere in the tree today.**
- `sel4/pins.toml [ns02201_h1v1]` starts :244; PWM keys :373-389; `boot_files` :390.

**Root** (`slime-root/src/`):
- H1V1 carve block `main.rs:787-826` under `#[cfg(slime_ns02201_h1v1)]`, invariant comment
  :787-790 (device untyped retypes monotonically upward, so CG `0x2_f002_0000` and WDT
  `0x2_f006_0000` are carved before UART0 `0x2_f013_0000`); each granule is
  `ScratchPage::claim` :800 + `device::DeviceRegion::map(allocator, VSPACE, page, paddr)` :805;
  product input :827-871 maps `PRODUCT_UART_PADDR` :536 (`env!("SLIME_PRODUCT_UART_PADDR")`)
  and prints `SLIME_ROOT product input ready uart=…` :864. UART7 at `0x2_f013_7000` is above
  UART0 and so is carved **after** it — the one place this lane differs from the PWM lane's
  "before UART" rule.
- `device.rs`: `DeviceRegion::{map :144, map_child :119, read32 :209, write32 :220,
  physical_address :167}`, `MAX_IO_DEVICES = 2` :683. `DwApbInput` :325-346 only reads RBR/LSR;
  **the root never programs a divisor or LCR** — firmware owns UART0's line config. UART7 has no
  firmware owner, so its line config is the driver's (A4).
- `graph_runtime/platform.rs`: `AuthorityDevice` :3, `AuthorityInventory` :9-58,
  `probe_authority_devices` :66-122 scans **virtio-mmio only** (`VIRTIO_MMIO_BASE + i *
  GRANULE_SIZE` :78, `VirtioMmio::probe` :88). **`AuthorityInventory::declared` does not exist
  yet — IO8 (ESC lane S2) adds it; this lane's S2 depends on that landing.**
- `graph_runtime/services/io_resource.rs:340-345` `install_driver(service, driver, instance,
  quota, shared_granule)`: `device/region/source = quota.device + 1` :349-352,
  `grant_mmio_region` :366, `grant_irq_source` :383 (unconditional — the ESC plan's note about
  `irqSources = 0` applies here too). Mediated path `io_resource.rs`: `map_mmio` :663,
  `read_mmio32` :750, `write_mmio32` :771, bounds `mediated_mmio_offset` :30 (`offset + 4 <=
  granted_bytes`, 4-aligned). Syscall labels `MMIO_READ32 = 67`, `MMIO_WRITE32 = 68`
  (`components/proto/src/syscall_abi.rs:88-99`), dispatch `ipc.rs:138-139`.
- Build inputs: `scripts/build/build-sel4.py` — `NS02201_H1V1` platform :158-173
  (`pins_section = "ns02201_h1v1"`), `PRODUCT_UART_KINDS` :180, UART0 export :906-916
  (regex `uart0-<kind>-<hex>` → `SLIME_PRODUCT_UART_PADDR`, only for `GRAPH_VARIANT`),
  `VARIANT_MANIFESTS` :230-237, `--platform` :1298. `slime-root/build.rs`: rerun-if-env :5-15,
  check-cfg :16-31, `slime_ns02201_h1v1` :46, `SLIME_PRODUCT_UART_PADDR` → cfg
  `slime_product_uart` + `rustc-env` :58-69.

**Userspace driver to copy** (`components/services/virtio-net-driver/src/main.rs`, and
`components/lib/src/virtio_mmio.rs`):
- `MediatedMmio { device_slot, region_slot, epoch }` :58, `new` :66, `read32(offset) ->
  Option<u32>` :74, `write32(offset, value) -> bool` :84. The 16550 driver uses only these two
  plus `io_device_bind(DEVICE_SLOT)` and `io_mmio_map(device_slot, region_slot, epoch, base,
  offset, length)` (`components/runtime/src/syscall.rs:482, :486`); never `begin` (that is the
  virtio handshake). Also available: `io_mmio_read32` :503 / `io_mmio_write32` :512 directly.
- Slots: `PEER_SLOT = 0`, `DEVICE_SLOT = 1`, `MMIO_SLOT = 2` (:35-37); readiness `send_ready()`
  :968 (`send(PEER_SLOT, b"ready", &[])` retried on `ERR_WOULDBLOCK`).
- Endpoint-served request loop template: `components/system/console/src/main.rs:32-80`
  (`recv_blocking(0, &mut buf, &mut caps)`; `MAX_MSG = 64`, `MAX_CAPS_PER_MSG = 1` in
  `syscall_abi.rs:108-109`; `reply(payload)` `syscall.rs:142`; `call(slot, payload, reply)`
  :136 for the client side). **A request is at most 64 bytes** — this bounds the serial-device
  request (A4/C1).
- Crate shape: `Cargo.toml` (`slime-component-<name>`, edition 2024, `[[bin]] test = false`,
  deps `slime-components`/`slime-proto`/`slime-rt`, build-dep `slime-build-support`), two-line
  `build.rs`, workspace `Cargo.toml` stanza `[profile.release.package.slime-component-<name>]`
  :44-47 (enforced by `just component_crate_split_check`), record
  `contracts/component-spec/v1/components/<name>.zti` (shape of `virtio-net-driver.zti`).

**Time**: `monotonic_read()` `syscall.rs:843`, `timer_arm(delay) -> timer id` :856 (expiry
signals the instance's declared notification + badge), `timer_cancel` :866,
`notification_wait(slot)` :185. No sleep syscall. Pattern to copy: `robot-supervisor`
`wait_until(ready_at)` (`components/applications/robot-supervisor/src/main.rs:225-237`) —
re-reads `monotonic_read` after every wake — and `robot-sensor` `await_tick()` :162-181 (checks
the badge bit). Grant row shape: `clockAuthority = [{ holder; monotonicRead = true; timerUse =
true; simulatedRead = false; simulatedAdvance = false; timerQuota; timerNotification;
timerBadgeBit }]` (`contracts/system-spec/v1/baselines/sel4-robot-runtime.zti:4-14`) plus
`clockAuthorityObject = true`. No component paces at 1 Hz today.

**Contract template** (`contracts/link-device/v1/{schema.zt,gen_rust.zt}`): scalars :21-48,
`WireField` :50, request record :57-65 + `requestLayout` :85-93 (56 B, every byte explicit,
little-endian), reply :70-80 + `replyLayout` :96-106 (24 B), `main` :137-144 reads
`SLIME_LINK_DEVICE_BINDINGS_ROOT`; `gen_rust.zt` reuses `wire.rust`/`wire.codec`
(`layoutNames/allValid/wireBytes/offsetConsts/wireStruct` :47-52). Generator
`scripts/generate/generate-link-device-bindings.py` (`render` :24, `--check` :74); recipe
`just link_device_gen` (`just/generate.just:5`); `--check` line in `contracts_check`
(`just/contracts.just:33`); generated `components/proto/src/link_device.rs`; validators
`valid_link_request` / `valid_link_reply` (`components/proto/src/lib.rs:1227, :1253`); tests
`components/proto/tests/link_device.rs`. **Rust only** — a C header (the spawn pattern,
`generate-spawn-bindings.py:21`) is needed only when a C component speaks the protocol, and
none does here.

**Compositions and gates**: the H1V1 product graph is `contracts/system-spec/v1/systems/sel4.zti`
(built with `--component-graph`; `--platform ns02201-h1v1` and `--test-terminator` are build
flags, not spec fields). Driver composition with a budget: `systems/sel4-io-link.zti` — grants
:33-92 (`device` rights `["mapMmio"]`, `mmioRegion` `["mapMmio"]`, `endpoint` `send`/`recv`),
`slotPins` :93-141 (`reason = "componentAbi"`), `ioResourceBudget` :243-254 with
`ioResourceBudgetObject = true`. Derivation: `scripts/lib/system_spec.py`
`DERIVED_GENERATION_FIXTURES` :71-114, `_SPEC_FIELDS` :141-180; gates `just system_spec_check`,
`just system_composition_closure_check` (`just/contracts.just:55-62`), `just
sel4_boot_layout_check` with `PLANES` at `scripts/check/check-sel4-boot-layout.py:70` and
`--bless`. Board gate template `scripts/check/check-nt98690-slisp.py`: constants :47-58,
`REQUIRED_MARKERS` :86 (34), `FAILURE_MARKERS` :152, `SESSION_COMMANDS` :175,
`build_artifacts()` :198, `check_identity()` :231, `stage_and_launch` :262, `read_until` :305,
`type_command` :332, `drive_session` :346, `main` :405 (imports the boot checker via
`load_script("nt98690_boot_gate", …)` :415 for `load_profile`, `wait_for_banner`,
`BANNER_PATTERN`); evidence `build/nt98690-slisp-evidence/`; recipe `just/hardware.just:186`.
Gate table `scripts/check/check-sel4-gate-controls.py:30` `GATES` (`nt98690_boot` 25,
`nt98690_sel4` 19, `nt98690_slisp` 34 at :74-77). Marker matching `scripts/lib/sel4_gate_markers.py`
(`chains_from_gate` :13, `match_marker_contract` :43).

**Python**: `flake.nix:88-96` python has jinja2/pyyaml/lxml/ply/pyfdt/jsonschema/setuptools —
no pyserial, no pymavlink, no pyproject/requirements. `ruff.toml` targets py311.

## A6. Go / no-go observations S1 must produce

1. **That header pin 13 follows P_GPIO[8]** (`--gpio-probe 8`, with the pad-level readback the review round added). The diagram says so; the probe is what turns that into an observation. No-go if it does not: fall back to UART6_1 on P_GPIO[12..13] (header pins 40 and 36, the next complete pair with no claim on it), never a soldered lead to a pad the header lacks.
2. **The surveyed state at the prompt**: `CG+0x60`, `CG+0x7C`, `CG+0x9C`, `TOP+0x34`, `TOP+0x38`, `TOP+0xA8`, `PAD+0x08`, `PAD+0x210`, and UART7 `LCR/LSR/IER/MCR/IIR` — pinned as `uart7_*_at_prompt` keys so S3's root asserts what it found rather than assuming.
3. **That the port transmits at the programmed rate**: N frames written, N decoded on the host radio port with valid CRC and consecutive `seq`. If frames arrive garbled, the 480 MHz source assumption is wrong (A2: `fix480m` is a DT declaration; the probe writes the divider field itself, so the field is not the suspect); the probe reports the LSR and the decoder's raw bytes so the rate can be diagnosed from the transcript.
4. **That restoration is observed**: every shared word read back after restore (the P6.PWM.A lesson, applied from the first run).

## A7. Risks

- **Source clock**: the 48 MHz in the DT is what Linux programs (`fix480m` / 10); at the U-Boot prompt the divider field may hold anything and the gate is probably off. The probe reads the words first (pinned as observations), then writes the field to 9 itself and restores it, so the only remaining assumption is the 480 MHz fixed source — which the decoded frames confirm or refute on the first run.
- **Radio configuration**: if the host radio is not at 57600 or the pair is not on one `NETID`, nothing decodes and the failure is indistinguishable from a dead pad. The bench session opens with `ATI5` on the host radio (a documented operator step) and a loopback of the host port (write a vector, read it back locally) before the board is touched.
- **Shared carve block with the ESC lane's S3**: both grow the same carve loop; the second to land rebases. Ordering stays ascending: TOP → CG → WDT → PWM → UART0 → UART7.
- **Core rail**: UART7's words are disjoint from PWM12's, but they share pages; the probe keeps the rail invariants and read-modify-writes one field per word.
- **QEMU plane has no 16550 the driver may touch**: on `aarch64-sel4-qemu-virt` the console is a PL011; on the RV64 profile the ns16550a at `0x10000000` is the kernel debug console. The S2 composition declares no device on any QEMU profile, so the driver never binds there.

## A8. Later sessions in one paragraph each

**S2 (general).** IO8's `AuthorityInventory::declared` is reused unchanged. New: two Zutai contracts with Rust bindings (and a generated Python constants module so S1's decoder and the Rust encoder are held to one vector); one root syscall, `CLOCK_RATE_READ`; `uart16550-driver` (slots PEER 0, DEVICE 1, MMIO 2; bind → wrong-device guard through mediated reads → line config → ready marker; per request: bounded length, THRE-polled byte writes, TEMT wait, reply with bytes written); `mavlink-heartbeat` (clock authority, endpoint to the driver, deadline loop, per-send marker, runs forever); `sel4-mavlink` composition with budget `mmioBytes 4096, mmioMappings 0, irqSources 0`, frozen boot layout, inventory row. QEMU evidence: `device absent`, `send seq=N status=no-device` ticking, `SLIME_GRAPH healthy`.

**S3 (board).** Pins → `build-sel4.py` exports `SLIME_NS02201_UART7_PADDR`, `SLIME_NS02201_UART7_CLK_HZ`, `SLIME_NS02201_UART7_DIVISOR`, `SLIME_NS02201_UART7_PAD` under `--mavlink-graph` on the H1V1 → `build.rs` cfg `slime_ns02201_uart7` → root carve appends UART7 after UART0, one-time CG/TOP bring-up with readback, static region handed to the declared inventory. Gate `scripts/check/check-nt98690-mavlink.py` in the Slisp checker's shape plus a second serial port for the ground radio: boots, waits for the ready and first-send markers, decodes ≥ 10 heartbeats on the radio port, checks cadence, sends the terminator, expects the banner. Evidence: UART0 transcript, radio-port capture, decoded-frame table, identities.

---

# Part B — Session 1: bench probe and facts (no Slime code)

Goal: from the unmodified vendor `nvt:` prompt, confirm header pin 13 follows P_GPIO[8], survey UART7's
clock/mux/line state, transmit pinned heartbeats through UART7 into an RFD900x, decode them on
the host through the paired radio, restore every shared word with readback, `reset`. One PR on
`feat/nt98690-uart-probe` branched from upstream `3429307f` (the review-round head — fetch it
first; the local checkout is four commits behind), one lane Decision entry plus one Audit entry,
two power cycles. Nothing here writes a block device.

## B0. Bench prerequisites

- Board on the bench, UART0 at `/dev/ttyUSB0`, SW18 at `0x1001`, power switch reachable; a
  meter for the pad-find step.
- Two RFD900x radios, assumed paired (A3). The ground radio on the host's second USB-serial port
  (`/dev/ttyUSB1` below); the air radio on its own 5 V supply. Only header pin 13 (P_GPIO[8]) →
  air-radio RX and pin 14 (GND) ↔ GND reach the board (3.3 V bank, no shifter). Pin 15
  (P_GPIO[9]), RTS, CTS unconnected.
- A terminal program for `ATI5` (operator step, B2 step 0). No logic analyzer: the actuator is
  the host decoder, so the UART0 transcript plus the radio capture carry the claim.

## B1. Host work before the board (one commit each, in this order)

1. **Work items.** `myque new` four items (UUIDs allocated by the tool; display keys per A4):
   epic `P6.MAV` "MAVLink heartbeat over an RFD900x on the H1V1" (`--kind epic`, tags
   `architecture`, `hardware`); milestone `P6.MAV.A` "H1V1 UART7 bench probe: heartbeats
   decoded on the ground radio" (parent P6.MAV, same tags; exit conditions = A1 "Exit, S1"
   verbatim); milestone `IO9` "serial-device and mavlink-heartbeat contracts, a 16550 TX driver,
   the sel4-mavlink composition" (parent P6.MAV, tag `io`, `depends` P6.MAV.A and IO8);
   milestone `P6.E` "MAVLink heartbeat on the H1V1" (parent P6.MAV, tags `architecture`,
   `hardware`, `depends` IO9). `just tasks_check`.
2. **Lane record.** a dated `h1v1-mavlink-lane` devlog folder holding `index.md` and `plan.md`: Kind `Decision`,
   Status `Proposed`, Scope the five files below, `Work items` = the four UUIDs, Gates
   `just nt98690_bench_probe_check`, `just sel4_pin_check`, `just sel4_gate_control_check`;
   `plan.md` = Part A verbatim. Register in `devlog/README.md`. `just devlog_check`.
3. **Bytes from the console** — `scripts/lib/uboot_console.py`: `Console.read_bytes_for(self,
   seconds: float) -> bytes` is the existing `read_for` loop returning `collected`
   (marker-stripped, `FF FF` collapsed, framing errors counted) before decode; `read_for`
   becomes `return self.read_bytes_for(seconds).decode("utf-8", "replace")`. `_strip_markers`
   :141-171 already collapses the doubled `0xFF` and defers a lone trailing `0xFF` in
   `_parmrk_pending`; of all 256 sequence numbers only seq 70 and 255 carry a `0xFF` after STX
   and none ends in one, so the deferral can never hold back a heartbeat's last byte. Docstring
   states that invariant. *Choice:* the radio port is a second `Console(receiver, UART7_BAUD,
   fail)` — same termios setup, framing-error count, append-only capture, `tcp:` bridging.
   *Rejected:* a separate class or pyserial: a second serial stack for one extra method.
4. **`scripts/lib/mavlink.py`** (new, stdlib only, ~90 lines). `STX = 0xFD`,
   `HEARTBEAT_MSGID = 0`, `HEARTBEAT_CRC_EXTRA = 50`, `HEARTBEAT_LEN = 9`,
   `RADIO_STATUS_MSGID = 109`. `x25_crc(data: bytes, crc: int = 0xFFFF) -> int` (check value
   `0x6F91`). `encode_heartbeat(seq: int, *, sysid=1, compid=191, mav_type=18, autopilot=8,
   base_mode=0, custom_mode=0, system_status=4, mavlink_version=3) -> bytes` (21 bytes;
   `ValueError` outside `0..255`). `@dataclass(frozen=True) Frame(msgid, seq, sysid, compid,
   payload: bytes, crc_ok: bool | None, raw: bytes)`. `class FrameDecoder` with `feed(self,
   data: bytes) -> list[Frame]`: internal buffer, scans for `STX`, needs 10 header bytes for
   `len`, frame length `12 + len` (+13 if `incompat_flags & 1`), returns when short; verifies
   the CRC for msgid 0 only, `crc_ok=None` otherwise; a bad-CRC heartbeat is returned with
   `crc_ok=False` and scanning resumes one byte after its STX. `radio_status_rssi(frame) ->
   tuple[int, int] | None` (`payload[4], payload[5]` = rssi, remrssi; `None` unless msgid 109
   with ≥ 6 payload bytes). `PINNED_HEARTBEATS: dict[int, bytes] = {0, 1, 255: …}` from A3.
   *Choice:* the vectors are asserted by the host-model scenario `pinned_vectors_reproduced`
   under the existing `just nt98690_bench_probe_check`. *Rejected:* a `__main__` self-test — a
   second entry point with no recipe.
5. **Constants, pure helpers, pins** — `scripts/check/check-nt98690-boot.py` beside the PWM
   block, each cited to A2: `UART7_BASE = 0x2_F013_6000`, `UART7_REG_SHIFT = 2`,
   `uart7_register(index: int) -> int` (`UART7_BASE + (index << UART7_REG_SHIFT)`, the one
   place the stride lives); indices `UART_THR = UART_DLL = 0`, `UART_IER = UART_DLM = 1`,
   `UART_IIR = UART_FCR = 2`, `UART_LCR = 3`, `UART_MCR = 4`, `UART_LSR = 5`;
   `UART_LCR_DLAB_8N1 = 0x83`, `UART_LCR_8N1 = 0x03`, `UART_FCR_ENABLE_RESET = 0x07`,
   `UART_IIR_FIFO_ENABLED = 0xC1`, `UART_LSR_THRE = 1 << 5`, `UART_LSR_TEMT = 1 << 6`,
   `UART_LSR_TX_MASK = 0x60` (BI/DR may float with the RX pad unrouted, so LSR is never
   compared outside these two bits), `UART_TEMT_POLLS = 5`; `UART7_CLOCK_SOURCE_HZ =
   480_000_000`, `CG_UART7_CLK_DIV = 0x60`, `CG_UART7_CLK_DIV_SHIFT = 8`,
   `CG_UART7_CLK_DIV_MASK = 0xFF << 8`, `CG_UART7_CLK_DIVIDER = 9` (field = divisor − 1),
   `UART7_CLOCK_HZ = UART7_CLOCK_SOURCE_HZ // (CG_UART7_CLK_DIVIDER + 1)`,
   `CG_UART7_CLK_EN = 0x7C`, `CG_UART7_CLK_EN_BIT = 22`, `CG_UART7_RESET = 0x9C`,
   `CG_UART7_RESET_BIT = 22` (active-low RSTN: 1 = released; set if clear, never pulsed);
   `TOP_UART7_MUX = 0x34`, `TOP_UART7_MUX_MASK = 0xF << 24`, `TOP_UART7_MUX_1 = 1 << 24`,
   `TOP_UART7_RTSCTS_MUX = 0x38` (bits [17:16], survey only, never written),
   `UART7_PAD_BIT = 8` (`TOP_PGPIO_FUNC`), `PAD_PGPIO8_PULL = 0x08` (bits [17:16], survey only);
   `UART7_BAUD = 57_600`, `UART7_DIVISOR = 52`; `UART_PROBE_FRAMES = 10`,
   `UART_PROBE_INTERVAL_SECONDS = 1.0`, `UART_PROBE_RECEIVE_WINDOW_SECONDS = 2.0`. Pure
   helpers with the documented example asserted: `uart7_clock_hz_from_field(field: int) -> int`
   (`480 MHz // (field + 1)`; `9 → 48_000_000`; used only to *print* what the prompt-time field
   means), `uart7_divisor(clock_hz: int, baud: int) -> int` (`round(clock / (16 × baud))`;
   `ValueError` beyond 2 % rate error; `(48_000_000, 57_600) → 52`),
   `cg_uart7_divider_word(current: int) -> int` (field replaced by 9).
   `check_uart_pins(profile) -> None` mirrors `check_pwm_pins`: the bases plus
   `uart7_reg_shift`, `uart7_reg_io_width`, `uart7_clock_source_hz`, `uart7_clock_divider`,
   `uart7_clock_hz`, `uart7_baud`, `uart7_divisor` must equal the constants.
   `sel4/pins.toml [ns02201_h1v1]` after the PWM keys: `uart7_base = "0x2f0136000"`,
   `uart7_reg_shift = 2`, `uart7_reg_io_width = 4`, `uart7_clock_source_hz = 480000000`,
   `uart7_clock_divider = 9`, `uart7_clock_hz = 48000000`, `uart7_baud = 57600`,
   `uart7_divisor = 52`, with a comment that the observed keys are absent until the probe
   reports them. `check-sel4-pins.py` after the PWM block: `uart7_base == 0x2_F013_6000`,
   shift 2, width 4, source exactly 480 MHz, `uart7_clock_hz == source // (divider + 1)` with
   `divider == 9`, `uart7_divisor == round(uart7_clock_hz / (16 × uart7_baud)) == 52` (a
   self-consistent 24 MHz/26 pair fails), and `observed_uart_routes: dict[int, tuple[str, str,
   str]] = {}` gating `uart7_pad`, `uart7_pad_header`, `uart7_clock_at_prompt`,
   `uart7_mux_at_prompt`, `uart7_probe_observed` exactly as `observed_pwm_routes` does — all
   absent while the table is empty, all present and matching once it holds `{7: ("P_GPIO8",
   "40-pin GPIO header pin 13", "2026-09-XX")}`. `uart7_clock_at_prompt` and `uart7_mux_at_prompt` are
   inline tables `{ divider = "0x…", gate = "0x…", reset = "0x…" }` / `{ top13 = "0x…",
   top14 = "0x…", pgpio_function = "0x…" }`, each value parsed as hex. *Rejected:* six flat
   keys — these six are one survey.
6. **The probe mode and its host model.** CLI in `main()` :1293: `--uart-probe`
   (store_true), `--uart-frames` (int, 10), `--uart-interval-seconds` (float, 1.0),
   `--uart-receiver` (str, default `None`; tty or `tcp:HOST:PORT`, opened at `UART7_BAUD`),
   `--uart-receive-window-seconds` (float, 2.0), `--uart-receiver-capture` (Path; raw
   `receiver.received_bytes`, written in the same `finally` as `--transcript`),
   `--uart-divisor` (int, 52; any other value turns the verdict line into `DIAGNOSTIC`, never
   `PASS` — B4's rate experiment is visible in the transcript, not a code edit). Dispatch
   between gpio-probe and pwm-probe; `--dry-run --uart-probe` prints `uart_probe_plan(frames,
   interval, receiver, window, divisor) -> list[str]` after `check_uart_pins`, with
   `[RMW+verify]` marks and the receiver as a comment line (`# receiver: /dev/ttyUSB1 at
   57600, 2.0s window per frame` or `# receiver: none — frames_decoded will be unobserved`);
   dry-run opens nothing. `validate_uart_probe_inputs(frames, interval, window, divisor)`
   (`1 ≤ frames ≤ 256`, non-negative times, `1 ≤ divisor ≤ 0xFFFF`) runs before any `Console`
   is constructed. Functions:
   - `uart_survey(console, prompt) -> dict[str, int]` over `UART_SURVEY_REGISTERS`
     (`cg_uart7_divider`, `cg_uart7_clock_enable`, `cg_uart7_reset`, `top_uart7_mux`,
     `top_uart7_rtscts_mux`, `top_pgpio_func`, `pad_pgpio4_pull`, `pad_p1_status`), printing
     `[uart]   <name> (<addr>) = 0x…` plus `[uart]   divider field 0x.. means <rate> Hz at the
     prompt`; `fail` without writing if `PAD+0x210` bit 4 reads 1 (the wiring claim is 3.3 V).
     Never reads the UART7 page.
   - `uart_survey_port(console, prompt) -> dict[str, int]` reads `lcr`, `lsr`, `ier`, `mcr`,
     `iir` — never index 0 (an `md.l` of RBR pops the RX FIFO) — and only after the reset is
     released and the gate is on: a read of a gated page may abort U-Boot (B4).
   - `program_uart7_line(console, prompt, divisor: int) -> None`, U-Boot's `ns16550_init`
     order: poll LSR up to `UART_TEMT_POLLS` for TEMT under `UART_LSR_TX_MASK`;
     `write_and_verify` IER=0x00; MCR=0x00 (mask 0x1F); `send_command` FCR=0x07 then
     `verify_register(IIR, 0x80, mask 0x80)`; LCR=0x03; LCR=0x83; DLL=`divisor & 0xFF`;
     DLM=`divisor >> 8`; LCR=0x03.
   - `transmit_frame(console, prompt, frame: bytes) -> None`: one `mw.l <THR> 0x<byte>` per
     byte, then up to `UART_TEMT_POLLS` reads of LSR until bit 6 (masked to bits 5–6; BI/DR
     from the unrouted RX pad are ignored); `fail` naming the last LSR value otherwise.
   - `receive_frame(receiver, decoder, seq, window) -> tuple[Frame | None, list[Frame]]`: loops
     `read_bytes_for(0.2)` into `decoder.feed` until a frame with `msgid == 0 and crc_ok and
     seq == seq` or the window elapses; returns it and everything decoded meanwhile
     (RADIO_STATUS for the rssi column, bad-CRC frames for the counters).
   - `uart_probe(console, prompt, frames, interval, receiver, window, divisor, timeout) -> str`:
     `reach_uboot` → `uart_survey` → `check_core_rail(… "before the UART probe")` → under the
     `pwm_probe` try/finally shape (`modified` before the first write, `primary_error`
     preserved): RMW `CG_UART7_RESET` set bit 22 if clear; RMW divider field to
     `CG_UART7_CLK_DIVIDER` (unconditionally — A4); RMW `CG_UART7_CLK_EN` set bit 22;
     `uart_survey_port`; RMW `TOP_UART7_MUX` field = 1; RMW `TOP+0xA8` clear bit 8 (bit 9
     untouched, TOP14 never written); `program_uart7_line`; drain the receiver 0.5 s; per frame
     k: `transmit_frame(encode_heartbeat(k & 0xFF))`, `receive_frame` when a receiver is
     given, print `[uart]   frame seq=<k> sent lsr=0x60 decoded=<yes|no|unobserved>
     rssi=<r>/<remr>|-`, `time.sleep(max(0, interval − elapsed))`. `finally`: `cleanup_steps` =
     restore TOP pad function, TOP13 mux, CG divider, CG gate, CG reset — each a
     `read_modify_write` back to the surveyed field so every restoration is verified by
     readback (the P6.PWM.A lesson) — then the core-rail recheck, then `reset` +
     `wait_for_banner` only when `cleanup_safe`. Result line `[uart]   result:
     frames_sent=<n> frames_decoded=<m|unobserved> crc_failures=<c> other_msgids=<o>
     settings_restored=<bool> firmware_banner=<bool>`; passes only if every sent frame decoded
     (when a receiver is given), `settings_restored`, `firmware_banner`.
   - Host model in `check-nt98690-bench-probe.py`: `class Uart16550Model` (`dll, dlm, lcr,
     ier, mcr, fifo_enabled, tx: bytearray, temt_stuck: bool`), dispatched from
     `ModelConsole._execute` for any address in the UART7 page: THR writes append to `tx` (DLL
     when DLAB), LSR reads `0x71` (THRE|TEMT plus BI|DR, so an unmasked compare fails) or
     `0x11` when `temt_stuck`, DLL/DLM/LCR/IER/MCR read back, IIR reads `0x81` after FCR bit 0, as the board did,
     FCR write-only, index-0 reads raise `AssertionError("RBR popped")`; reads of the UART7 page
     while the model's gate bit is clear raise `AssertionError("gated page read")`.
     `initial_registers()` gains `CG+0x60 = 0x0013_0000` (field 0x13, a 24 MHz prompt value the
     probe must overwrite and put back), `CG+0x7C = 0` (gated), `CG+0x9C = 0x10` (WDT bit only;
     UART7 held), `TOP+0x34 = 0`, `TOP+0x38 = 0`, `PAD+0x08 = 0x100`. `class ModelReceiver`
     (`read_bytes_for(seconds) -> bytes`, `received_bytes`, `framing_errors = 0`, `close()`)
     drains the model's `tx`, with mutations `radio_status_before_each: bool` (prefixes junk
     plus a RADIO_STATUS frame with rssi 200/190), `corrupt_seq: int | None`, `drop_seq: int |
     None`. `run_uart(console, receiver, frames=10, divisor=52)` monkeypatches
     `PROBE.time.sleep` as `run_pwm` does; `assert_uart_restored(console)` covers the five
     words. Scenarios, each confirmed to fail against the named mutation:
     `pinned_vectors_reproduced`; `parmrk_markers_stripped_for_bytes` (`_strip_markers` on a
     detached `Console`: the seq-255 vector with its `0xFF` doubled comes back byte-identical
     with `framing_errors == 0`, and `FF 00 41` counts one error); `normal_uart` (all 10
     decoded, five words restored, core rail untouched, `reset` sent, the eleven 16550
     commands in U-Boot order, no `md.l` at index 0, no `mw.l` at TOP+0x38, `TOP+0xA8` bit 9
     still set, LSR compared under the mask); `reset_set_once_never_pulsed` (`mw.l` to
     `CG+0x9C` occur exactly twice, release then restore, never 1→0→1 mid-run);
     `reset_already_released_untouched` (bit 22 set at survey: no `mw.l` to `CG+0x9C` at all);
     `divider_restored_to_surveyed_value` (field 0x13 → 9 → 0x13, with the survey line printing
     24000000 Hz); `no_receiver_reports_unverified`; `gated_port_not_read_before_ungate`;
     `reset_release_dropped_refuses` (a dropped RMW on `CG_UART7_RESET` fails naming the
     register, no divider/gate/UART7 write follows, cleanup still runs);
     `dropped_restore_detected` (parametrised over the five restored words: failure names
     `restore UART7 <word>`, later steps still verified, `reset` still attempted);
     `decoder_resyncs_after_radio_status` (all 10 decoded, `other_msgids=10`, `rssi=200/190`);
     `corrupted_crc_not_counted` (seq 3 corrupted: fails naming seq 3, `frames_decoded=3`,
     `crc_failures=1`, restored, `reset` sent); `temt_timeout_detected` (`temt_stuck`: fails
     after `UART_TEMT_POLLS` LSR reads naming `lsr=0x11`, no further THR writes, restored);
     `cleanup_on_interrupt` (KeyboardInterrupt in the interval sleep after frame 2, preserved,
     restored); `uart_pins_bind_the_divisor` (`check_uart_pins` refuses `uart7_divisor = 26`,
     `uart7_clock_hz = 24000000`, `uart7_clock_divider = 19`, `uart7_baud = 115200` one at a
     time); `diagnostic_divisor_never_passes` (`--uart-divisor 104` through `main()` with
     `PROBE.Console` stubbed: verdict says `DIAGNOSTIC`); `uart_cli_rejects_before_opening`
     (`--uart-frames 0`, `--uart-divisor 0`: `BoardOpened` never raised). 17 new → 32, plus
     item 7's → 33.
7. **Receiver-only mode.** `--uart-listen-seconds N` (float, default `None`; requires
   `--uart-receiver`, forbids `--serial`); `uart_listen(receiver, seconds: float) -> None`
   prints every decoded frame as `[listen] msgid=<m> seq=<s> crc_ok=<…> rssi=<…>`, then
   `[listen] bytes=<n> frames=<f> heartbeats=<h> framing_errors=<e>`; asserts nothing, like
   `--monitor`. Scenario `listen_reports_without_board`. *Choice:* listen — it exercises the
   real receive path against the actual radio pair, and a linked ground radio in `MAVLINK=1`
   mode emits `RADIO_STATUS` on its own, so pairing is checked with the board untouched.
   *Rejected:* a TX–RX loopback on the host adapter: needs a jumper and proves the one link
   (host ↔ radio) that is not in doubt.
8. **Docs and gates.** Module docstring names the new mode and the never-touch set (TOP14,
   UART7 index 0, the PWM12 words); the `just/hardware.just` comment mentions the 16550 model.
   Run, in order: `python3 scripts/check/check-nt98690-boot.py --dry-run --uart-probe` (paste
   into the PR), `just nt98690_bench_probe_check` (33 scenarios), `just sel4_pin_check`,
   `just sel4_gate_control_check` (count unchanged, `nt98690_boot` still 25 — no marker
   contract for a bench mode), `just ruff`, `just typos`, `just devlog_check`,
   `just tasks_check`.

## B2. Board session

| Step | Command / action | Observation to record |
|---|---|---|
| 0 | Host radio only: terminal at 57600 8N1, `+++` after 1 s silence, `ATI5`, `ATO`. **Operator step** — scripting command mode risks leaving the radio in it on a failure path. Then air radio powered; `--uart-listen-seconds 15 --uart-receiver /dev/ttyUSB1` | `SERIAL_SPEED=57`, `AIR_SPEED`, `NETID`, `MAVLINK`, `ECC`, `MIN/MAX_FREQ`, firmware version; green LED solid; the listen line (`frames`, `rssi`, `framing_errors`) |
| 1 | Power cycle 1. `--survey`, then `--gpio-probe 8 --gpio-cycles 20 --gpio-hold-seconds 1 --transcript …/gpio-probe-8.log`, meter on header pin 13 (and, if time allows, pin 15 for stillness) | **pin 13 follows P_GPIO[8]**, as the diagram says; level readbacks in the transcript |
| 2 | Board off. Wire pin 13 (P_GPIO[8]) → air-radio RX, pin 14 (GND) ↔ GND; radio on its own 5 V. Power cycle 2 | — |
| 3 | `--uart-probe --uart-receiver /dev/ttyUSB1 --transcript …/uart-probe.log --uart-receiver-capture …/radio-capture.bin 2>&1 \| tee …/uart-probe-stdout.log` | the eight survey words and the five port words; every `[RMW+verify]` and 16550 readback; per-frame `decoded=yes`, `rssi`; the five `[cleanup] restore … : verified` lines; core-rail recheck; result line; banner |
| 4 | Vendor banner after `reset`; if the ESC lane shares the session, its readback rerun now; full power cycle; banner again | the board is unharmed |

## B3. Records after the session

- `sel4/pins.toml`: `uart7_pad = "P_GPIO8"`, `uart7_pad_header = "40-pin GPIO header pin 13"`,
  `uart7_clock_at_prompt = { divider, gate, reset }`, `uart7_mux_at_prompt = { top13, top14,
  pgpio_function }`, `uart7_probe_observed = "2026-09-XX"`; `observed_uart_routes` gains
  `{7: ("P_GPIO8", "40-pin GPIO header pin 13", "2026-09-XX")}`; `just sel4_pin_check`.
- a dated `h1v1-uart-probe` devlog entry, Kind `Audit`: `uart-probe.log` (UART0 raw),
  `uart-probe-stdout.log` (the `[uart]` lines — the PWM audit's lesson, whose `check` lines were
  lost), `radio-capture.bin` (raw ground-radio bytes) and `radio-capture.log` (the decoder's
  table regenerated from the `.bin` with `FrameDecoder`, so the claim is reproducible from the
  immutable capture), `gpio-probe-8.log`; an Investigation-log row per B2 step; the `ATI5`
  readings and the header pin labelled operator observations. Register in `devlog/README.md`;
  append a `## Corrections` row to the lane Decision entry with the pinned route and any A2
  value the board contradicted.
- `myque close` P6.MAV.A with the observed exit condition quoted from the result line and the
  restore readbacks; leave it open if any of B4 fired. Update the memory file.

## B4. Stop conditions (Defect entries, not improvisation)

- A core-rail invariant fails at survey, or `PAD+0x210` bit 4 reads 1 → stop, write nothing,
  record the values.
- U-Boot prints `"Synchronous Abort" handler` on the first UART7 read → the gate or reset
  polarity is not what A2 says; power-cycle, record the CG words, no retry with guessed bits.
- Pin 13 does not follow P_GPIO[8] → record; the diagram is then wrong for this board and
  every position in `header_pins` is suspect. Fall back to UART6_1 on P_GPIO[12..13] (pins 40
  and 36) only after `--gpio-probe 12` confirms it — a new route entry with its own run, never
  a widened range.
- LSR TEMT never sets → clock or reset wrong; the probe fails naming the LSR value; record the
  CG words and the port survey; do not retry blindly.
- Frames sent, nothing decoded → check the air radio's LEDs, repeat step 0's listen, `ATI5` on
  both radios if reachable. Only if the pair is proven linked, run once with `--uart-divisor
  104` (host radio unchanged at 57600): a decode there means the true source is 960 MHz and
  A2's parent rate is a Defect; `--uart-divisor 26` likewise for 240 MHz. Either verdict is
  `DIAGNOSTIC` and closes nothing.
- Banner missing after `reset`, or the vendor Linux misbehaves after the power cycle → compare
  the before/after survey; no further writes until explained.

---

# Part C — Sessions 2 and 3

Paths and line numbers verified at the local checkout (`e34347aa`) unless marked unverified;
`check-sel4-pins.py` anchors are at `3429307f`. Three facts from the tree that shape the design:
(1) both shipped drivers use only `MediatedMmio` and never `io_mmio_map`; the root's `map_mmio`
(`graph_runtime/services/io_resource.rs:60-92`) requires a whole granule and `map_child`s the
frame, moving the root-side base (`device.rs:119-135`), after which the mediated path would read
the wrong address — so a declared, root-mapped page is driven mediated only. (2) `grant_irq_source`
(`io_resource.rs:612-651`) never consults the quota, so `irqSources = 0` is fine. (3)
`monotonic_read` returns raw `CNTPCT` ticks (`platform_timer.rs:279-282`); the root's
`frequency_hz()` (`:198`) reaches no component, and `build-generation.py:1163-1170` strips
product selectors from component environments — a 1 Hz cadence needs one new clock syscall.
Also: `sel4_boot_layout_check` stops each plane at `[layout] end`, before activation
(`check-sel4-boot-layout.py:195-200`), so the heartbeat never runs under that gate;
`test_sel4_root` expects 214 at this checkout (`just/quality.just:275`).

## C0. Design decisions (with the rejected alternative)

Only what A4 leaves open.

| Decision | Choice | Rejected |
|---|---|---|
| `serial-device/v1` wire | Request 64 B (`MAX_MSG`): `magic u32, version u16, op u8, flags u8, length u16, reserved [2], payload [52]`. Reply 16 B: `magic u32, version u16, op u8, status u8, bytes_written u16, reserved [2], detail u32` (LSR at completion, masked to `THRE\|TEMT`). Constants `FORMAT_VERSION 1, REQUEST_LEN 64, REPLY_LEN 16, MAX_PAYLOAD 52, SERIAL_MAGIC 0x4453_4C53, OP_WRITE 1, OP_STATUS 2, KNOWN_REQUEST_FLAGS 0, STATUS_OK 0, STATUS_BAD_LENGTH 1, STATUS_BAD_OP 2, STATUS_NO_DEVICE 3, STATUS_DEVICE_ERROR 4, STATUS_TIMEOUT 5`. `1 ≤ length ≤ MAX_PAYLOAD` for `OP_WRITE`, `== 0` for `OP_STATUS`, unused payload bytes zero (canonical, as `link-device`). `TX_POLL_LIMIT`, clock, divisor, baud: not in the contract | A 24 B reply with counters: nothing reads them |
| Where the driver's divisor comes from | **Build-time knobs**: `option_env!("SLIME_UART16550_CLOCK_HZ")` / `option_env!("SLIME_UART16550_DIVISOR")` → `Option<(u32, u16)>`, added to `COMPILE_TIME_KNOBS` (`components/build-support/src/lib.rs:56-63`) and forwarded in `build-generation.py`'s `sel4_component_environment` (`:1160-1170`, beside the fabric knobs, commented as board facts keyed on `--platform`, the class of `SLIME_TARGET_PROFILE` at `:1162`, not behaviour selectors). Absent on every QEMU/closure build (`build-system-image.py:110-131` scrubs `SLIME_*`): the QEMU driver has no line config and, if a device ever bound, prints `fail no line configuration` and answers `STATUS_DEVICE_ERROR`. S3's `build-sel4.py` sets them from pins only for H1V1 + `--mavlink-graph` | A `configure` op (board facts in the general client); component-spec `configuration` (`contracts/component-spec/v1/schema.zt:82-94`: integer defaults the runtime never delivers, and the default would be 48 MHz in a board-neutral spec) |
| `reg-shift` | Fixed `REG_SHIFT = 2` in the driver (`offset = index << 2`), the layout the root's `DwApbInput` fixes (`device.rs:325-335`) and the pins checker asserts for UART0 | A contract constant (not a protocol fact); a build input (nothing in the tree varies it) |
| Tick rate for 1 Hz (**root mechanism, S2**) | New clock syscall `CLOCK_RATE_READ = 70` in `contracts/syscall-abi/v1/schema.zt` (group `clock`, after `:257`; 69 is the current max), served in `serve_clock_request` (`graph_runtime/services/policy.rs:70-95`) from `timer_adapter.frequency_hz()` under the caller's `monotonicRead` authority; rows in the `ipc.rs:1552` table (`SERVICE_CLOCK`) and `clock_request_len` (`:177`, `Some(0)`); wrapper `slime_rt::monotonic_frequency() -> Result<u64, i64>` in `components/runtime/src/syscall.rs` + `sel4_transport.rs:1333`; `just syscall_abi_gen`; host tests beside `:1621`. Heartbeat period = frequency ticks. The ESC lane's `FAILSAFE_NS` needs the same fact | A per-profile tick table in the component (names a SoC; QEMU virt's `CNTFRQ` is not pinned); a knob (scrubbed by closure builds) |
| Readiness | No `send_ready`; the heartbeat's first `call` is the rendezvous (the driver blocks in `recv_blocking(PEER_SLOT)`) and health is liveness (`services.rs:1425-1446`). `[uart16550-driver] ready clk_hz=<n> divisor=<d>` is a debug marker only | Copying `send_ready` (`virtio-net-driver:968`): nothing here waits for it |
| Heartbeat lifetime | Runs forever on every plane (product component). The boot-layout gate kills QEMU at `[layout] end`; the QEMU evidence transcript is a bounded manual boot (C1), not a gate | Stopping after N under QEMU: needs a plane selector in the component, and a heartbeat that stops is the bug A4 rejects |
| One-vector seam (host) | `contracts/mavlink-heartbeat/v1/gen_rust.zt` renders **both** `components/proto/src/mavlink_heartbeat.rs` and `scripts/lib/mavlink_heartbeat.py` (the `io-resource/v1` shape: `render :: Format -> { python; rust }`, `schema.zt:51-58`), each carrying the constants and the three vectors. `scripts/lib/mavlink.py` (S1) imports its constants from the generated module; the Rust test and S1's host model both assert against the generated vectors | A Rust test shelling out to Python; two hand-copied vectors |
| Future combined image | `declared(region)` fills `regions[0]`/`devices[0]`, `len = 1`. A `sel4-pwm-mavlink` would add `AuthorityInventory::declare_next(&mut self, region) -> Result<usize, DeviceRegion>` filling `regions[len]`/`devices[len]` up to `MAX_IO_DEVICES = 2` (exactly PWM + UART7), ordinals in ascending paddr (PWM `device 0`, UART7 `device 1`), two budget rows. Not built by either lane | Widening `MAX_IO_DEVICES` |

## C1. Session 2 — general: contracts, syscall, driver, producer, composition (QEMU green)

Nothing in this PR names the NT98690, `ns02201`, a pin, 48 MHz, or 52. Branch `feat/serial-tx`.
Depends on the ESC lane's S2 (`AuthorityInventory::declared`).

**Contracts** (template `contracts/link-device/v1/{schema.zt,gen_rust.zt}`, generator
`scripts/generate/generate-link-device-bindings.py`):
- `contracts/serial-device/v1/{schema.zt,gen_rust.zt}` → `components/proto/src/serial_device.rs`
  (`WireSerialRequest`/`WireSerialReply` via `wire.codec`'s `offsetConsts`/`wireStruct`, `valid`
  pins `requestLen == 64 && replyLen == 16`); `scripts/generate/generate-serial-device-bindings.py`;
  `just serial_device_gen` (`just/generate.just`, beside `:5`); `--check` line in
  `contracts_check` (`just/contracts.just:33`); `pub mod serial_device` +
  `valid_serial_request`/`valid_serial_reply` in `components/proto/src/lib.rs` (beside `:1227`);
  `components/proto/tests/serial_device.rs` (shape of `tests/link_device.rs`).
- `contracts/mavlink-heartbeat/v1/{schema.zt,gen_rust.zt}`: constants `stx 253, payloadLen 9,
  frameLen 21, msgId 0, crcExtra 50, sysId 1, compId 191, mavType 18, autopilot 8, baseMode 0,
  customMode 0, systemStatus 4, mavlinkVersion 3, crcInit 65535, crcPoly 0x8408`; `frameLayout`
  (wire order, 21 B): `stx 1, len 1, incompat 1, compat 1, seq 1, sysid 1, compid 1, msgid 3
  byteArray, custom_mode 4, type 1, autopilot 1, base_mode 1, system_status 1, mavlink_version 1,
  checksum 2`; vectors `vectorSeq0/1/255` as hex `Text` from A3. Renders
  `components/proto/src/mavlink_heartbeat.rs` (consts, `WireHeartbeat` codec, `VECTOR_SEQ_0/1/255:
  [u8; 21]`) and `scripts/lib/mavlink_heartbeat.py` (`# @generated`). Encoder logic is not
  generated: `pub fn x25_crc(bytes: &[u8]) -> u16` and `pub fn encode_heartbeat(seq: u8) -> [u8;
  FRAME_LEN]` in `components/proto/src/lib.rs` (the CRC is arithmetic, not a format).
  `scripts/generate/generate-mavlink-heartbeat-bindings.py`; `just mavlink_heartbeat_gen`;
  `--check` in `contracts_check`. Tests: `components/proto/tests/mavlink_heartbeat.rs` asserts
  `encode_heartbeat(0/1/255) == VECTOR_SEQ_*`, `x25_crc(b"123456789") == 0x6F91`, and
  `WireHeartbeat::decode(encode(seq))` round-trips; `scripts/lib/mavlink.py` replaces S1's
  literals with `from mavlink_heartbeat import …`, and S1's host model gains
  `encode_heartbeat(seq) == VECTOR_SEQ_n` and `FrameDecoder.feed(VECTOR_SEQ_n)` yields one
  CRC-valid msgid-0 frame.

**Root** (mechanism only): `CLOCK_RATE_READ` per C0 (`syscall-abi` schema, `ipc.rs` two tables,
`policy.rs` arm, runtime wrapper), host tests in `ipc.rs` (bump `expected=` in
`just/quality.just:275`). No `main.rs` change; `AuthorityInventory::declared` reused unchanged.

**Driver** `components/services/uart16550-driver/{Cargo.toml,build.rs,src/main.rs}`
(`slime-component-uart16550-driver`, deps `slime-components`/`slime-proto`/`slime-rt`, workspace
`Cargo.toml` `[profile.release.package.…]` stanza, record
`contracts/component-spec/v1/components/uart16550-driver.zti`: service, `provides = ["device";
"mmioRegion"]`, `requires = ["device"; "endpoint"; "mmioRegion"]`, `runtime.devices = ["device"]`
as `virtio-blk-driver.zti:47-49`, `health = "required"`, `stackBytes = 16384`,
`test.requiredTestEnvironment = "sel4_boot_layout_check"` then S3 → `nt98690_mavlink_check`).
Slots `PEER_SLOT 0, DEVICE_SLOT 1, MMIO_SLOT 2`; constants `REG_SHIFT 2`, `THR 0x00, IER 0x04,
FCR 0x08, LCR 0x0C, MCR 0x10, LSR 0x14, DLL 0x00, DLM 0x04, IIR 0x08`, `LSR_THRE 1<<5, LSR_TEMT
1<<6`, `VIRTIO_MAGIC 0x74726976`, `TX_POLL_LIMIT: u32 = 4096` mediated LSR reads per byte
(documented as syscalls, not time; sized from the S2 transcript, C3). Start:
`io_device_bind(DEVICE_SLOT)` — `Err` → `[uart16550-driver] device absent, refusing requests`,
`device = None`; `Ok` → `MediatedMmio::new(1, 2, epoch)`, read offset 0: `== VIRTIO_MAGIC` →
same marker, `device = None`; else `line_config()` in A2's U-Boot order: wait `LSR & TEMT`
(bounded), `IER = 0`, `MCR = 0`, `FCR = 0x07`, `LCR = 0x03`, `LCR = 0x83`, `DLL = divisor &
0xFF`, `DLM = divisor >> 8`, readback `DLL/DLM`, `LCR = 0x03`, readback `LCR == 0x03`, `MCR ==
0`, `IER == 0`, `IIR == 0xC1`; any mismatch → `[uart16550-driver] fail line config reg=<name>
got=<hex>` and `device = None` (resident, `STATUS_DEVICE_ERROR`); knobs absent → `fail no line
configuration`; success → `[uart16550-driver] ready clk_hz=<n> divisor=<d>`. Loop:
`recv_blocking(PEER_SLOT, &mut buf, &mut caps)` → `WireSerialRequest::decode` +
`valid_serial_request` (bad → `STATUS_BAD_LENGTH`/`BAD_OP`) → `device.is_none()` → `NO_DEVICE` →
`OP_WRITE`: per byte spin `read32(LSR) & THRE` ≤ `TX_POLL_LIMIT` else `TIMEOUT` with
`bytes_written = i`; `write32(THR, b)`; after the last byte spin on `TEMT`; reply `OK
bytes_written = length, detail = LSR & (THRE|TEMT)`. `OP_STATUS` → `OK detail = LSR`.
`reply(&reply.encode())`.

**Producer** `components/applications/mavlink-heartbeat/…` (`slime-component-mavlink-heartbeat`,
record with `componentType = "application"`, `requires = ["endpoint"]`, `health = "required"`).
`DRIVER_SLOT = 0` (endpoint, `componentAbi`); tick via
`resolve_binding(b"notification:mavlink-heartbeat-tick+wait")` (`virtio-net-driver:201`
pattern), `TIMER_BADGE = 1 << 9`. Start: `period = monotonic_frequency()`; `next =
monotonic_read() + period`; `seq: u8 = 0`. Loop: `frame = encode_heartbeat(seq)`; request
`{SERIAL_MAGIC, 1, OP_WRITE, 0, 21, [0;2], payload[..21] = frame}`; `call(DRIVER_SLOT,
&request.encode(), &mut reply)`; map `status` → `ok|no-device|device-error|timeout`, decode
failure or `bytes_written != 21` → `bad-reply`; `debug_write("[mavlink-heartbeat] send seq=<n>
status=<s>\n")`; `wait_until(next)` = `robot-supervisor:225-237` re-reading the clock after each
wake with `robot-sensor:162-181`'s badge check; `next += period` (if `next < now`, `next = now +
period`); `seq = seq.wrapping_add(1)`. A failed `timer_arm`/`monotonic_read` is fatal (the
supervisor's reasoning, `:208-214`).

**Composition** `contracts/system-spec/v1/systems/sel4-mavlink.zti` = `sel4.zti` verbatim plus:
`name = "sel4-mavlink"`, `generation = <next unused number; unverified>`, components
`mavlink-heartbeat`, `uart16550-driver`; placements `{component = "uart16550-driver"; owner =
"init"; health = "required"}`, `{component = "mavlink-heartbeat"; owner = "init"; health =
"required"; dependencies = ["uart16550-driver"]}`; grants `init-uart16550-driver` and
`init-mavlink-heartbeat` (executable, `["exec"; "spawn"]`), `uart7-device` (device,
`["mapMmio"]`, source/target driver), `uart7-mmio` (mmioRegion, `["mapMmio"]`),
`heartbeat-serial-rpc` (endpoint, source `mavlink-heartbeat`, target `uart16550-driver`,
`["send"; "recv"]`); slot pins: `init` two new `bootLayout` slots (numbers from `--bless`), driver
`heartbeat-serial-rpc 0 / uart7-device 1 / uart7-mmio 2` `componentAbi`, heartbeat
`heartbeat-serial-rpc 0` `componentAbi`; `notifications = [{name = "mavlink-heartbeat-tick";
source = "init"; target = "mavlink-heartbeat"}]` with `notificationBindings` `init signal` /
`mavlink-heartbeat wait slot 0` (`sel4-robot-runtime.zti:427-458`); `clockAuthority = [{holder =
"mavlink-heartbeat"; monotonicRead = true; timerUse = true; simulatedRead = false;
simulatedAdvance = false; timerQuota = 1; timerNotification = "mavlink-heartbeat-tick";
timerBadgeBit = 9}]`, `clockAuthorityObject = true`; `ioResourceBudget = [{holder =
"uart16550-driver"; mmioBytes = 4096; mmioMappings = 0; irqSources = 0; dmaPages = 0;
dmaMappings = 0; outstandingRequests = 0; bufferLoans = 0; device = 0}]`,
`ioResourceBudgetObject = true`. Then: `DERIVED_GENERATION_FIXTURES["sel4-mavlink"] =
"sel4-mavlink.zti"` (`scripts/lib/system_spec.py:71`); `generate-generation-from-spec.py` →
`compositions/sel4-mavlink.zti`; bless `baselines/sel4-mavlink.zti`; inventory row in
`contracts/composition-inventory/v1/inventory.zti` with `owningGate = "sel4_boot_layout_check"`;
`ROOT_PARAMETERS["sel4-mavlink"] = ("qemuKeyboard",)` in
`scripts/generate/generate-system-image-closures.py:181` (the graph carries `slisp-input`) and
regenerate `closures/sel4-mavlink.zti`; `PLANES += ("sel4-mavlink", "slime-sel4-mavlink.elf")`
(`check-sel4-boot-layout.py:70`) and `just sel4_boot_layout_bless` →
`contracts/boot-layout/v1/fixtures/sel4-mavlink.layout`. `build-sel4.py`: `MAVLINK_VARIANT =
"mavlink"`, `VARIANT_MANIFESTS["mavlink"] = "sel4-mavlink"`, `VARIANT_TARGET_DIRS`
`root-mavlink`, `VARIANT_IMAGES` `slime-sel4-mavlink.elf`, `--mavlink-graph` in the `selected`
list (`:1310`), and admitted beside `GRAPH_VARIANT` at `:899` (QEMU keyboard), `:905` (product
UART + terminator), `:933` (external slisp), `:1172` (`component_graph`), `:1331` (terminator
prerequisite).

**QEMU evidence** (no gate, by discipline): boot the `build/closure/sel4-mavlink/` image with the
QEMU line `check-sel4-boot-layout.py:166-181` uses, 60 s, and commit the transcript showing
`SLIME_ROOT io authority inventory devices=<n> mode=userspace`, `SLIME_IO quota …
instance=uart16550-driver`, `[uart16550-driver] device absent, refusing requests`,
`[mavlink-heartbeat] send seq=0 status=no-device` … `seq=5`, `SLIME_GRAPH healthy … failed=0`
(counts unverified until seen), `SLIME_ROOT READY`. QEMU virt instantiates empty virtio-mmio
transports; whether `probe_authority_devices` inventories one (bind succeeds, guard trips on the
magic) or none (bind fails) ends in the same marker — record which.

**Records**: `myque new` `IO9` (parent `P6.MAV`) and `P6.E` if S1 has not; `roadmap/11-io-substrate.md`
`## IO9 — Bounded serial transmit and a clock-paced producer` beside IO8; a dated
`serial-tx-mechanism` devlog entry, Kind `Change`, Status `Verified`, Gates
`just sel4_boot_layout_check`, `just test_host`, `just test_sel4_root`, transcript as sibling;
`devlog/README.md` row; `myque close IO9` on the QEMU transcript + host tests.

**Verification order** (every name checked against `just/*.just`): `serial_device_gen`,
`mavlink_heartbeat_gen`, `syscall_abi_gen` → `contracts_check` → `component_spec_check`,
`component_crate_split_check` → `system_spec_check` (runs `generate-generation-from-spec.py
--check`) → `system_composition_closure_check` → `python3
scripts/generate/generate-system-image-closures.py` then `system_image_builder_check`,
`system_image_closure_check` → `fmt_check_all`, `lint_all`, `ruff`, `typos` → `test_host` →
`test_sel4_root` (new count) → `sel4_component_graph_check` (markers unchanged) →
`sel4_boot_layout_bless`, `sel4_boot_layout_check` → `system_image_closure_aggregate_check` →
`generation_check` → `io_driver_authority_check`, `clock_authority_check`,
`sel4_root_boot_check` → `devlog_check`, `tasks_check`. (The ESC plan's
`composition_inventory_check` does not exist; the recipe is `system_composition_closure_check`.)

## C2. Session 3 — H1V1: bring-up, build inputs, board gate, observed exit

Branch `feat/nt98690-mavlink`. Depends on S1 (pins `uart7_*`) and S2. Uses A2's concrete words.

**Pins and build inputs**: S1's `[ns02201_h1v1]` keys (Part B). `check-sel4-pins.py`:
`boot_files` gains `"slime-sel4-mavlink-ns02201-h1v1-test-terminator.bin"` (`sel4/pins.toml:390`,
checker `:625-630`). `build-sel4.py`, only `platform is NS02201_H1V1 and variant ==
MAVLINK_VARIANT`: root env `SLIME_NS02201_UART7_PADDR`, `SLIME_NS02201_UART7_CLOCK_HZ`,
`SLIME_NS02201_UART7_DIVISOR`; generation env (the `environment=` argument of
`build_sel4_generation`, `:760-778`) `SLIME_UART16550_CLOCK_HZ`, `SLIME_UART16550_DIVISOR`.
`slime-root/build.rs`: `rerun-if-env-changed` ×3, `rustc-check-cfg=cfg(slime_ns02201_uart7)`,
`rustc-cfg=slime_ns02201_uart7` when the paddr is set (refuse unless `target_profile ==
"aarch64-sel4-nt98690-h1v1"`), `rustc-env` pass-through of the three.

**Root** (`slime-root/src/main.rs`, under `all(slime_ns02201_h1v1, slime_ns02201_uart7)`):
statics `NS02201_UART7_PAGE: FreePage`, `NS02201_UART7_REGION: Option<device::DeviceRegion>`,
`NS02201_UART7_PADDR = const_parse_hex_usize(env!(…))` (`:422`); the TOP granule
`NS02201_PINMUX_PAGE` + `NS02201_PINMUX_REGISTERS: Option<MappedGranule>` shared with the ESC lane
under `any(slime_ns02201_pwm, slime_ns02201_uart7)`, carved first in the `:791-825` loop (TOP
`0x2_f001_0000` < CG). After the product-input block (`:826-871`, UART0 at `0x2_f013_0000`) and
before the timer phase: `ScratchPage::claim` + `DeviceRegion::map(allocator, VSPACE, page,
NS02201_UART7_PADDR)` → static. `ns02201_uart7_bringup(cg: &MappedGranule, top:
&MappedGranule)`, called right after the carve, all RMW with readback, in A2's order: (1)
`CG+0x9C |= 1<<22` (active-low reset; set if clear, never pulse), (2) `CG+0x60[15:8] = 9`, (3)
`CG+0x7C |= 1<<22`, (4) `TOP+0x34[27:24] = 1`, (5) `TOP+0xA8 &= ~(1<<8)` (bit 9 untouched); any
readback mismatch → `fatal!("uart7 bringup readback reg=… want=… got=…")`; then
`SLIME_NT98690 uart7 bringup pad=P_GPIO8 clk_hz=48000000`. Pure helpers in `platform_timer.rs`
beside the ESC lane's: `uart7_divider_word(current, field) -> u32`, `uart7_mux_word(current) ->
u32`, `set_bit23(current) -> u32`, and the shared `pgpio_function_word(current, pad) -> u32`
(`current & !(1 << pad)`; ESC calls it with 0, this lane with 4) — host tests, bump
`just/quality.just:275`. Admission arm at `:1038`, written once as
`#[cfg(any(slime_ns02201_pwm, slime_ns02201_uart7))]`: `match (PWM.take(), UART7.take()) {
(Some(r), None) | (None, Some(r)) => AuthorityInventory::declared(r), (Some(_), Some(_)) =>
fatal!("two declared devices need a multi-device inventory"), _ => fatal!(…) }`; whichever lane
lands second rebases onto this shape. The `sel4` H1V1 image (no `slime_ns02201_uart7`) keeps its
exact carve order.

**Gate** `scripts/check/check-nt98690-mavlink.py`, the Slisp checker's shape (`build_artifacts`
with `--mavlink-graph --test-terminator --skip-pin-check`; `STEM =
"slime-sel4-mavlink-ns02201-h1v1-test-terminator"`; `check_identity` requires `variant ==
"mavlink"`, `component_graph`, `test_terminator`; `stage_and_launch`, `read_until` reused
verbatim; `main` imports the boot checker via `load_script`) plus `--receiver` opened as
`Console(receiver, 57600, fail)`. Session: `read_until` `HEARTBEAT_FIRST =
r"\[mavlink-heartbeat\] send seq=0 status=ok"` (`BOOT_TIMEOUT_SECONDS`); then
`HEARTBEAT_WINDOW_SECONDS = 12.0` of `receiver.read_bytes_for(0.5)` slices appended to a
`capture: bytearray` while UART0 keeps draining into the transcript; each slice through
`mavlink.FrameDecoder.feed`, recording host `time.monotonic()` per frame; require ≥
`MIN_HEARTBEATS = 10` msgid-0 frames, CRC valid, `(seq_n − seq_{n−1}) % 256 == 1`, every
inter-arrival in `[0.75, 1.25]` s, other msgids passed through and counted; then
`console.write(TEST_TERMINATOR)` → `RESET_MARKER` → `wait_for_banner`. `REQUIRED_MARKERS`,
frozen against a fresh QEMU `sel4-mavlink` transcript first and only then the board: P6.A's
five, `Starting loader`, `Entering kernel`, `Booting all finished, dropped to user space`,
allocator, `SLIME_NT98690 uart7 bringup pad=P_GPIO8 clk_hz=48000000`, `SLIME_ROOT product input
ready uart=0x2f0130000`, `SLIME_TIMER acquired irq=30 freq_hz=12000000`, `SLIME_TIMER delivered
…`, `SLIME_TIMER OK`, `SLIME_ROOT io authority inventory devices=1 mode=declared`, `SLIME_ROOT
generation admitted number=\d+ executables=8 instances=8 grants=16 ` (counts unverified),
`SLIME_GRAPH activated instances=1`, `\[init\] launching component graph`, `SLIME_IO quota
task=\d+ instance=uart16550-driver devices=1 shared_granule=0`, `SLIME_GRAPH healthy
generation=\d+ instances=[0-9a-f]{16} required=6 live=6 idle=6 failed=0` (unverified),
`SLIME_ROOT READY target_profile=aarch64-sel4-nt98690-h1v1`, `\[init\] product services
resident`, `SLIME_NT98690 test terminator accepted`, `SLIME_NT98690 reset request kind=wdt`,
`U-Boot 2021\.10`; `EXPECTED_UNORDERED = (r"\[uart16550-driver\] ready clk_hz=48000000
divisor=52", r"\[mavlink-heartbeat\] send seq=0 status=ok", r"\[mavlink-heartbeat\] send seq=1
status=ok")` because the driver, producer, and root print from different tasks.
`FAILURE_MARKERS` = P6.C's set (`check-nt98690-slisp.py:152-171`, minus the Slisp-session ones)
plus `\[uart16550-driver\] fail .*`, `\[uart16550-driver\] device absent.*`,
`\[mavlink-heartbeat\] send .* status=(no-device|device-error|timeout|bad-reply)`. Evidence
`build/nt98690-mavlink-evidence/{mavlink-session.log, radio-capture.bin, decoded-frames.json,
mavlink-identities.json}` (identities add `receiver`, `receiver_baud`, `heartbeats_decoded`,
`other_msgids`, `interarrival_min/max_s`, `capture_sha256`). `GATES` row `("nt98690_mavlink",
"check/check-nt98690-mavlink.py", <exact count>)` (`check-sel4-gate-controls.py:77`);
`just/hardware.just` `nt98690_mavlink_check serial="" receiver="": sel4_boot_layout_check` with
the operator's counted steps in the comment (1 card carries the `.bin`; 2 header pin 13 → radio RX,
GND ↔ GND, radio on its own 5 V; 3 `ATI5` on the host radio shows 57600 and the shared `NETID`;
4 power on when told); inventory `owningGate` → `nt98690_mavlink_check`.

**Exit condition (serial-observable, both ports)**: after `SLIME_ROOT READY
target_profile=aarch64-sel4-nt98690-h1v1`, UART0 shows `[uart16550-driver] ready
clk_hz=48000000 divisor=52` and `send seq=0 status=ok`, `seq=1 status=ok`; the receiver port
decodes ≥ 10 consecutive msgid-0 frames, CRC valid, `seq` consecutive mod 256, inter-arrival
1 s ± 0.25 s; no failure marker; the terminator → watchdog reset → vendor banner; 0 framing
errors on UART0; one power-on. No operator observation is in the condition.

**Records**: `roadmap/07-architecture-portability.md` `### P6.E — MAVLink heartbeat over UART7
to an RFD900x` (Deliverables, Required checks, Verification target `just nt98690_mavlink_check
<serial> <receiver>`, Exit condition, `**Exit condition (observed):**`), P6 status `:910`
amended to "…the resident interactive Slisp product over UART0, and one transmit-only UART
carrying a MAVLink heartbeat to a telemetry radio — and nothing else: storage, network, display,
generation management, and serial receive remain unclaimed"; `roadmap/README.md:35` boundary
column and `:54` sentence gain the same clause; a dated
`p6e-nt98690-mavlink` devlog entry, Kind `Change`, Status `Verified`, Gates `just nt98690_mavlink_check`, `just
sel4_gate_control_check`, siblings: transcript, `radio-capture.bin`, `decoded-frames.json`,
identities; `devlog/README.md` row; `myque close P6.E` with the observed exit; memory file.

**Verification order**: `sel4_pin_check` → `sel4_gate_control_check` (new row) →
`test_sel4_root` (helpers) → `sel4_nt98690_image_check` (the `sel4` H1V1 image unchanged) and
`python3 scripts/build/build-sel4.py --platform ns02201-h1v1 --mavlink-graph --test-terminator`
→ `nt98690_slisp_check <serial>` (P6.C still green) → `nt98690_mavlink_check <serial>
<receiver>` → `devlog_check`, `tasks_check`.

## C3. Risks and stop conditions (S2/S3)

- **Mediated path on a declared region**: `read_mmio32` indexes `inventory.region(…)` and reads
  at the root VA; if any later code `map_child`s or `unmap`s the UART7 region, every driver access
  fails `MapFailed`. The driver must never call `io_mmio_map`; `reclaim_driver` on a driver
  restart (`services/io_resource.rs:388`) must leave a declared region mapped — verify on the ESC
  lane's S2 before relying on it.
- **QEMU virt's empty transports**: if `probe_authority_devices` inventories one, the H1V1-shaped
  budget binds it and the guard is the only thing between the driver and a virtio register; the
  guard runs before any write, including the `TEMT` wait.
- **`TX_POLL_LIMIT` is syscalls, not time**: if the mediated round trip is far cheaper than 1 µs,
  4096 spins may be under one byte-time at 57600 (174 µs) — a `TIMEOUT` on a healthy port. Size
  it from the observed syscall cost in the S2 transcript and note the derivation; a spurious
  `timeout` marker is a stop condition on the board.
- **Unrouted RX pad**: `LSR.DR/BI/FE` may be set; anything comparing the whole LSR to a value is
  wrong — mask to `THRE|TEMT` everywhere, including `detail`.
- **Cadence tolerance vs the radio**: the RFD900x packetises on frame boundaries and interleaves
  `RADIO_STATUS`; ±250 ms is generous but a saturated air link can hold a frame. Non-zero msgids
  never count and never fail; a window with < 10 heartbeats fails.
- **Shared carve block**: ascending order TOP → CG → WDT → (PWM) → UART0 → UART7; a misordered
  UART7 carve reproduces P6.C's `Allocate(DeviceFramePassed)`. `nt98690_slisp_check` must stay
  green under the combined `build.rs`; if it regresses, stop.
- **Marker counts and health**: `required=`/`executables=`/`grants=` are frozen from the QEMU
  transcript, never computed; `EXPECTED_UNORDERED` covers the cross-task race.
- **Syscall label 70**: a collision with anything the ESC lane adds is a `syscall_abi_gen
  --check` failure, not a runtime one; rebase whichever lands second.
- **Stop conditions**: `SLIME_ROOT FATAL` (bring-up readback) on the board; `[uart16550-driver]
  fail line config …` (the port did not take the divisor — recheck A2's `CG+0x60` semantics from
  S1's survey); `status=timeout` with a healthy `ready` marker; a QEMU `sel4-mavlink` boot where
  the driver prints `ready` (something bound); the receiver decoding frames with wrong `seq`
  spacing (the deadline loop is broken, not the wire).
