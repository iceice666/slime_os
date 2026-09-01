/*
 * P6.A firmware-handoff probe for the Novatek NT98690 (NS02201) H1V1.
 *
 * Proves the deployment loop is real and measures the facts P6.B's seL4
 * platform port must be built from: the exception level the vendor firmware
 * hands over at, the physical address `booti` places the image at, the
 * primary core's counter frequency, the implemented physical-address range,
 * and the interrupt controller's line count. Every one of those is a property
 * of this board's firmware rather than of its documentation -- TF-A programs
 * `CNTFRQ_EL0` only on secondary cores (`plat_helpers.S`), so the primary's
 * value is unknown until it is read here.
 *
 * Entry contract is the arm64 Linux boot protocol, which is what the vendor
 * U-Boot implements and what an seL4 elfloader would receive unchanged:
 *   x0 = physical address of the device tree U-Boot relocated for us
 *   x1 = x2 = x3 = 0
 *   AArch64, MMU off, D-cache off, entered at the exception level BL31 chose
 * All four are preserved and reported, because a real port must trust them.
 *
 * Output goes through the same 16550 UART0 that BL31 and U-Boot already use,
 * by polling only. The probe never programs the baud divisor, the GIC, or the
 * timer: it observes, and its one write to the world outside the UART holding
 * register is the PSCI SYSTEM_RESET that returns the board to its vendor
 * firmware. Nothing here touches eMMC.
 *
 * Building with -DSLIME_PROBE_QEMU_VIRT retargets the three board addresses at
 * QEMU's `virt` machine so this exact instruction stream can be executed
 * before it is ever carried to hardware. That is the only thing the define
 * changes: the register reads, the calibration, the verdicts, the fault
 * vectors, and the reset path are the same code either way. The placement
 * check is expected to report FAIL there, because QEMU places the image at its
 * own RAM base plus `text_offset` rather than at the address this is linked
 * for -- which is itself worth seeing, since it proves the check can fail.
 *
 * Two values are meaningless under that variant and must not be read as
 * evidence about anything: `cnt_hz_est`, because QEMU does not throttle its
 * console to the line rate the estimate assumes and so reports a small
 * fraction of the true frequency, and `base`/`pc`, for the reason above. The
 * variant validates the instruction stream, the register decoding, and the
 * verdict logic; only the board validates the board.
 */

    .section .text.start, "ax"
    .globl _start
_start:
    /*
     * The 64-byte arm64 Image header. U-Boot's `booti` reads this at the load
     * address before branching, and refuses the image without the magic.
     *
     * `text_offset` and `image_size` are taken from the linker script rather
     * than written as literals, because this board's patched `booti` relocates
     * to `ALIGN(gd->ram_base, 2 MiB) + text_offset` unconditionally -- so a
     * `text_offset` that disagreed with the link address would silently move
     * the image out from under the code. Deriving both from the same symbols
     * makes that disagreement unrepresentable.
     *
     * flags bit 3 ("image can be placed anywhere") is set for correctness on a
     * stock U-Boot, which honours it and runs the image where it was loaded.
     * The Novatek patch ignores the bit; both paths land on `text_offset`
     * because that is also the address the gate loads to.
     */
    b       entry                       /* code0: executable branch          */
    .long   0                           /* code1                             */
    .quad   PAYLOAD_BASE                /* text_offset                       */
    .quad   __image_size                /* image_size                        */
    .quad   0xa                         /* flags: LE, 4 KiB pages, anywhere  */
    .quad   0                           /* res2                              */
    .quad   0                           /* res3                              */
    .quad   0                           /* res4                              */
    .ascii  "ARM\x64"                   /* magic: LE 0x644d5241              */
    .long   0                           /* res5                              */

entry:
    /*
     * Mask every exception before touching anything. The vendor firmware left
     * its own vectors installed and we are about to replace them; an interrupt
     * arriving in that window would vector somewhere neither side owns.
     */
    msr     daifset, #0xf

    /* Preserve the firmware handoff registers before any of them is clobbered. */
    mov     x19, x0                     /* device-tree physical address       */
    mov     x20, x1
    mov     x21, x2
    mov     x22, x3

    /* Where we are actually running, as opposed to where we were linked. */
    adr     x23, _start

    mrs     x24, CurrentEL
    lsr     x24, x24, #2                /* 2 == EL2, the expected handoff     */

    /*
     * Our own stack. U-Boot's `sp` belongs to U-Boot, and this must be
     * 16-byte aligned: `SCTLR_ELx.SA` may be set, in which case a misaligned
     * stack pointer faults on the first `stp`.
     */
    adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    /*
     * Install our own vectors before the first memory access that could fault.
     * A fault after this point prints its syndrome and resets the board; a
     * fault before it lands in whatever U-Boot left behind, which on this
     * board is a "Synchronous Abort" handler that hangs at the prompt.
     */
    adrp    x0, vectors
    add     x0, x0, :lo12:vectors
    cmp     x24, #2
    b.ne    1f
    msr     vbar_el2, x0
    b       2f
1:
    msr     vbar_el1, x0
2:
    isb

    /* Console base, kept live for the whole run so `putc` needs no reload. */
#ifdef SLIME_PROBE_QEMU_VIRT
    movz    x25, #0x0000                /* QEMU virt PL011 at 0x09000000 */
    movk    x25, #0x0900, lsl #16
#else
    movz    x25, #0x0000                /* H1V1 UART0 at 0x2_f013_0000   */
    movk    x25, #0xf013, lsl #16
    movk    x25, #0x0002, lsl #32
#endif

    mov     x26, #0                     /* verdict accumulator: 0 == all ok   */

    adr     x0, msg_banner
    bl      puts
    bl      put_nl

    /* ---- handoff state ---- */

    adr     x0, lbl_el
    mov     x1, x24
    bl      print_kv

    adr     x0, lbl_base
    mov     x1, x23
    bl      print_kv

    adr     x0, lbl_pc
    adr     x1, .
    bl      print_kv

    adr     x0, lbl_x0
    mov     x1, x19
    bl      print_kv

    adr     x0, lbl_x1
    mov     x1, x20
    bl      print_kv

    adr     x0, lbl_x2
    mov     x1, x21
    bl      print_kv

    adr     x0, lbl_x3
    mov     x1, x22
    bl      print_kv

    adr     x0, lbl_fdt_magic
    bl      read_fdt_magic
    mov     x1, x0
    adr     x0, lbl_fdt_magic
    bl      print_kv

    adr     x0, lbl_fdt_size
    bl      read_fdt_size
    mov     x1, x0
    adr     x0, lbl_fdt_size
    bl      print_kv

    /* ---- core identity ---- */

    mrs     x1, midr_el1
    adr     x0, lbl_midr
    bl      print_kv

    mrs     x1, midr_el1
    ubfx    x1, x1, #4, #12             /* MIDR_EL1.PartNum: 0xd09 == A73     */
    adr     x0, lbl_midr_part
    bl      print_kv

    mrs     x1, mpidr_el1
    adr     x0, lbl_mpidr
    bl      print_kv

    mrs     x1, id_aa64mmfr0_el1
    adr     x0, lbl_mmfr0
    bl      print_kv

    mrs     x1, id_aa64mmfr0_el1
    and     x1, x1, #0xf                /* PARange: selects the kernel's PA bits */
    adr     x0, lbl_parange
    bl      print_kv

    mrs     x1, id_aa64pfr0_el1         /* bits 27:24 == 0 confirms no GIC sysregs */
    adr     x0, lbl_pfr0
    bl      print_kv

    /* ---- generic timer ---- */

    mrs     x1, cntfrq_el0
    adr     x0, lbl_cntfrq
    bl      print_kv

    isb
    mrs     x27, cntpct_el0
    mov     x1, x27
    adr     x0, lbl_cntpct0
    bl      print_kv

    /*
     * Calibrate the counter against the UART's own clock.
     *
     * `CNTFRQ_EL0` is a value firmware writes, not a measurement, and this
     * board's TF-A writes it on secondary cores only -- so on the primary it
     * may be zero or stale, and P6.B cannot pin a timer frequency from it
     * alone. Transmitting a known number of characters at a known line rate
     * gives an independent estimate: 230 characters at 115200 baud, 8N1, is
     * 230 * 10 / 115200 = 19.965 ms, so the tick delta multiplied by 50 is the
     * frequency in Hz, biased 0.17% low. That is nowhere near enough precision
     * to pin a timer, and far more than enough to tell 12 MHz from 24 MHz,
     * 25 MHz, or a stopped counter.
     *
     * Both reads are taken with the transmitter fully drained (`LSR.TEMT`), so
     * the window is the burst itself rather than the burst plus whatever was
     * still in the FIFO.
     */
    adr     x0, lbl_calib
    bl      puts
    bl      wait_tx_idle
    isb
    mrs     x27, cntpct_el0
    mov     w13, #230
1:
    mov     w0, #'.'
    bl      putc
    subs    w13, w13, #1
    b.ne    1b
    bl      wait_tx_idle
    isb
    mrs     x28, cntpct_el0
    bl      put_nl

    mov     x1, x28
    adr     x0, lbl_cntpct1
    bl      print_kv

    sub     x1, x28, x27
    adr     x0, lbl_cnt_delta
    bl      print_kv

    sub     x1, x28, x27
    mov     x2, #50
    mul     x1, x1, x2
    adr     x0, lbl_cnt_hz_est
    bl      print_kv

    /* ---- entry state the loader inherits ---- */

    bl      read_sctlr
    mov     x1, x0
    adr     x0, lbl_sctlr
    bl      print_kv

    cmp     x24, #2
    b.ne    3f
    mrs     x1, hcr_el2
    b       4f
3:
    mov     x1, #-1                     /* EL2-only register, unreadable here */
4:
    adr     x0, lbl_hcr_el2
    bl      print_kv

    cmp     x24, #2
    b.ne    5f
    mrs     x1, cnthctl_el2
    b       6f
5:
    mov     x1, #-1
6:
    adr     x0, lbl_cnthctl
    bl      print_kv

    cmp     x24, #2
    b.ne    7f
    mrs     x1, cntvoff_el2
    b       8f
7:
    mov     x1, #-1
8:
    adr     x0, lbl_cntvoff
    bl      print_kv

    /* ---- interrupt controller ---- */

    /*
     * GICD is at 0x2_fff01000 -- above 4 GiB, like every device on this SoC.
     * Reading it here proves a 64-bit device access works from the handoff
     * state, which is the property P6.B's kernel device window depends on.
     * Read-only: the distributor is already initialised by BL31 and this probe
     * has no business reprogramming it.
     *
     * The base is re-derived before every read rather than held across the
     * prints between them, because `putc` scratches x9 -- holding it there
     * silently turned the second and third reads into loads from a line-status
     * value, which printed as zeroes while the verdict below (which did
     * re-derive) still passed. Deriving per read costs three instructions and
     * removes the whole class.
     */
    bl      gicd_base
    ldr     w1, [x0]                    /* GICD_CTLR  */
    adr     x0, lbl_gicd_ctlr
    bl      print_kv

    bl      gicd_base
    ldr     w1, [x0, #4]                /* GICD_TYPER */
    adr     x0, lbl_gicd_typer
    bl      print_kv

    bl      gicd_base
    ldr     w1, [x0, #8]                /* GICD_IIDR  */
    adr     x0, lbl_gicd_iidr
    bl      print_kv

    bl      gicd_base
    ldr     w1, [x0, #4]
    and     x1, x1, #0x1f               /* ITLinesNumber */
    add     x1, x1, #1
    lsl     x1, x1, #5                  /* 32 * (N + 1) INTIDs */
    adr     x0, lbl_gic_irqs
    bl      print_kv

    /* ---- verdicts ---- */

    /*
     * Each check is stated as a literal `ok`/`FAIL` line rather than left for
     * the gate to derive from the hex above. The gate's marker chain is
     * regex-matched and its tamper control must be able to instantiate every
     * pattern, so "this value is non-zero" and "this value grew" are decided
     * here, where the values are, and asserted there as literals.
     */

    /*
     * Placement: the link-time address of `_start` against the address we are
     * actually executing from. The absolute relocation below is what makes
     * this a real comparison -- `adr` alone would give the same answer twice.
     */
    movz    x9, #:abs_g3:_start
    movk    x9, #:abs_g2_nc:_start
    movk    x9, #:abs_g1_nc:_start
    movk    x9, #:abs_g0_nc:_start
    cmp     x23, x9
    cset    w1, ne
    adr     x0, chk_placement
    bl      print_check

    cmp     x24, #2
    cset    w1, ne
    adr     x0, chk_el2
    bl      print_check

    bl      read_fdt_magic
    movz    x9, #0xfeed
    movk    x9, #0xd00d, lsl #16
    cmp     x0, x9
    cset    w1, ne
    adr     x0, chk_fdt_magic
    bl      print_check

    bl      read_sctlr
    and     x0, x0, #1                  /* SCTLR_ELx.M */
    cmp     x0, #0
    cset    w1, ne
    adr     x0, chk_mmu_off
    bl      print_check

    cmp     x28, x27
    cset    w1, ls                      /* fail unless cntpct1 > cntpct0 */
    adr     x0, chk_cnt_advance
    bl      print_check

    bl      gicd_base
    ldr     w10, [x0, #4]
    mov     w1, #1
    cbz     w10, 9f                     /* all zeroes: nothing responded */
    cmn     w10, #1
    b.eq    9f                          /* all ones: bus returned no device */
    mov     w1, #0
9:
    adr     x0, chk_gicd
    bl      print_check

    cbnz    x26, 10f
    adr     x0, msg_payload_ok
    bl      puts
    bl      put_nl
    b       11f
10:
    adr     x0, msg_payload_fail
    bl      puts
    bl      put_nl
11:

    adr     x0, msg_reset
    bl      puts
    bl      put_nl
    b       do_psci_reset

/*
 * Return the device tree's magic word, or an all-ones sentinel if x19 does not
 * point somewhere a device tree could plausibly live.
 *
 * The range check is not decoration: an out-of-range x0 from firmware would
 * otherwise be dereferenced, and while the vectors below would catch the fault
 * and reset the board, "the probe crashed" is much weaker evidence than "the
 * pointer firmware passed was not in DRAM".
 */
read_fdt_magic:
    mov     x0, #-1
    movz    x9, #0x1000
    cmp     x19, x9
    b.lo    1f
    movz    x9, #0x0000
    movk    x9, #0x8000, lsl #16        /* 0x80000000: top of this board's DRAM */
    cmp     x19, x9
    b.hs    1f
    ldr     w0, [x19]
    rev     w0, w0                      /* device trees are big-endian */
1:
    ret

read_fdt_size:
    mov     x0, #-1
    movz    x9, #0x1000
    cmp     x19, x9
    b.lo    1f
    movz    x9, #0x0000
    movk    x9, #0x8000, lsl #16
    cmp     x19, x9
    b.hs    1f
    ldr     w0, [x19, #4]               /* totalsize */
    rev     w0, w0
1:
    ret

/* SCTLR of whichever exception level we were handed, so the MMU check is
 * meaningful at either. Reading SCTLR_EL2 at EL1 is undefined, hence the
 * branch rather than an unconditional read. */
read_sctlr:
    cmp     x24, #2
    b.ne    1f
    mrs     x0, sctlr_el2
    ret
1:
    mrs     x0, sctlr_el1
    ret

/* The interrupt controller's distributor base, in x0. One definition, so the
 * reported registers and the verdict below cannot read different devices. */
gicd_base:
#ifdef SLIME_PROBE_QEMU_VIRT
    movz    x0, #0x0000                 /* QEMU virt GICv2 distributor */
    movk    x0, #0x0800, lsl #16
#else
    movz    x0, #0x1000                 /* H1V1 GICD at 0x2_fff0_1000 */
    movk    x0, #0xfff0, lsl #16
    movk    x0, #0x0002, lsl #32
#endif
    ret

/*
 * PSCI SYSTEM_RESET (SMC32 function 0x84000009). TF-A implements it on this
 * platform through the watchdog, and it is the same call the vendor U-Boot's
 * own `reset` command makes -- so the board returns through its loader to its
 * own firmware with no operator action, which is what lets a gate run twice.
 *
 * The transmitter is drained first. Without that the reset truncates whatever
 * is still in the FIFO, and the last line of the transcript is the one the
 * gate needs.
 */
do_psci_reset:
    bl      wait_tx_idle
    movz    x0, #0x0009
    movk    x0, #0x8400, lsl #16
    mov     x1, #0
    mov     x2, #0
    mov     x3, #0
    smc     #0
    /* Unreachable if firmware honoured the call. */
    mov     x19, x0
    adr     x0, msg_reset_failed
    bl      puts
    mov     x1, x19
    bl      put_hex64
    bl      put_nl
1:
    wfe
    b       1b

/*
 * Fault vectors. Sixteen entries of 0x80 bytes, 2 KiB aligned as VBAR
 * requires. Every one lands in the same reporter: this payload has no
 * legitimate exceptions, so the only question a fault raises is which
 * syndrome, and the answer belongs on the wire before the board resets.
 */
    .balign 2048
vectors:
    .rept 16
    b       fault_entry
    .balign 0x80
    .endr

fault_entry:
    /*
     * Re-establish the stack before reporting. This handler prints, and
     * printing calls, so it needs a usable stack -- and one plausible reason to
     * be here is that the stack pointer was the problem. Without this a
     * misaligned or wandered `sp` faults again inside the handler, and the
     * board loops silently at the one moment it has something to say.
     */
    adrp    x0, __stack_top
    add     x0, x0, :lo12:__stack_top
    mov     sp, x0

    adr     x0, msg_fault
    bl      puts

    cmp     x24, #2
    b.ne    1f
    mrs     x1, esr_el2
    b       2f
1:
    mrs     x1, esr_el1
2:
    bl      put_hex64

    adr     x0, msg_fault_elr
    bl      puts
    cmp     x24, #2
    b.ne    3f
    mrs     x1, elr_el2
    b       4f
3:
    mrs     x1, elr_el1
4:
    bl      put_hex64

    adr     x0, msg_fault_far
    bl      puts
    cmp     x24, #2
    b.ne    5f
    mrs     x1, far_el2
    b       6f
5:
    mrs     x1, far_el1
6:
    bl      put_hex64
    bl      put_nl
    b       do_psci_reset

/*
 * 16550 output, polled.
 *
 * The board's device tree gives this UART `reg-shift = <2>` and
 * `reg-io-width = <4>`, so register N is at byte offset N*4 and every access
 * is 32 bits wide: the transmit holding register is at +0x00 and the line
 * status register at +0x14. Nothing here configures the port -- BL31 brought
 * it up and U-Boot has been printing through it, so writing the divisor would
 * only risk losing the console this milestone exists to read.
 */
#ifdef SLIME_PROBE_QEMU_VIRT
/* PL011: data register at +0x00, flag register at +0x18, TXFF bit 5 set while
 * the FIFO is full and BUSY bit 3 set while the transmitter is still shifting. */
putc:
1:
    ldr     w9, [x25, #0x18]
    tbnz    w9, #5, 1b
    str     w0, [x25]
    ret

wait_tx_idle:
1:
    ldr     w9, [x25, #0x18]
    tbnz    w9, #3, 1b
    ret
#else
    .equ    UART_THR, 0x00
    .equ    UART_LSR, 0x14

/* Clobbers x9. Every caller in this file must assume that; a value parked
 * there across a print is a value the next print destroys. */
putc:
1:
    ldr     w9, [x25, #UART_LSR]
    tbz     w9, #5, 1b                  /* LSR.THRE: holding register empty */
    str     w0, [x25, #UART_THR]
    ret

wait_tx_idle:
1:
    ldr     w9, [x25, #UART_LSR]
    tbz     w9, #6, 1b                  /* LSR.TEMT: transmitter fully idle */
    ret
#endif

puts:
    stp     x29, x30, [sp, #-16]!
    mov     x11, x0
1:
    ldrb    w0, [x11], #1
    cbz     w0, 2f
    bl      putc
    b       1b
2:
    ldp     x29, x30, [sp], #16
    ret

put_nl:
    stp     x29, x30, [sp, #-16]!
    mov     w0, #13
    bl      putc
    mov     w0, #10
    bl      putc
    ldp     x29, x30, [sp], #16
    ret

/* x1 = value. Always sixteen digits: a fixed width keeps the gate's marker
 * regexes fixed-width too, which is what its tamper control can instantiate. */
put_hex64:
    stp     x29, x30, [sp, #-16]!
    mov     x12, x1
    mov     w0, #'0'
    bl      putc
    mov     w0, #'x'
    bl      putc
    mov     w14, #16
1:
    lsr     x0, x12, #60
    and     w0, w0, #0xf
    cmp     w0, #9
    b.hi    2f
    add     w0, w0, #'0'
    b       3f
2:
    add     w0, w0, #('a' - 10)
3:
    bl      putc
    lsl     x12, x12, #4
    subs    w14, w14, #1
    b.ne    1b
    ldp     x29, x30, [sp], #16
    ret

/* x0 = label, x1 = value. */
print_kv:
    stp     x29, x30, [sp, #-16]!
    mov     x15, x1
    bl      puts
    mov     x1, x15
    bl      put_hex64
    bl      put_nl
    ldp     x29, x30, [sp], #16
    ret

/* x0 = label, w1 = 0 for ok, non-zero for FAIL. Accumulates failures in x26 so
 * the summary line at the end cannot disagree with the individual verdicts. */
print_check:
    stp     x29, x30, [sp, #-16]!
    mov     w15, w1
    bl      puts
    cbnz    w15, 1f
    adr     x0, msg_ok
    b       2f
1:
    add     x26, x26, #1
    adr     x0, msg_fail
2:
    bl      puts
    bl      put_nl
    ldp     x29, x30, [sp], #16
    ret

    .section .rodata, "a"

msg_banner:         .asciz "=== SLIME_NT98690 probe: entry reached ==="
msg_payload_ok:     .asciz "SLIME_NT98690 PAYLOAD_OK"
msg_payload_fail:   .asciz "SLIME_NT98690 PAYLOAD_FAIL"
msg_reset:          .asciz "SLIME_NT98690 reset request kind=psci"
msg_reset_failed:   .asciz "SLIME_NT98690 reset failed x0 = "
msg_fault:          .asciz "SLIME_NT98690 FAULT esr = "
msg_fault_elr:      .asciz " elr = "
msg_fault_far:      .asciz " far = "
msg_ok:             .asciz "ok"
msg_fail:           .asciz "FAIL"

lbl_el:             .asciz "SLIME_NT98690 el         = "
lbl_base:           .asciz "SLIME_NT98690 base       = "
lbl_pc:             .asciz "SLIME_NT98690 pc         = "
lbl_x0:             .asciz "SLIME_NT98690 x0         = "
lbl_x1:             .asciz "SLIME_NT98690 x1         = "
lbl_x2:             .asciz "SLIME_NT98690 x2         = "
lbl_x3:             .asciz "SLIME_NT98690 x3         = "
lbl_fdt_magic:      .asciz "SLIME_NT98690 fdt_magic  = "
lbl_fdt_size:       .asciz "SLIME_NT98690 fdt_size   = "
lbl_midr:           .asciz "SLIME_NT98690 midr       = "
lbl_midr_part:      .asciz "SLIME_NT98690 midr_part  = "
lbl_mpidr:          .asciz "SLIME_NT98690 mpidr      = "
lbl_mmfr0:          .asciz "SLIME_NT98690 mmfr0      = "
lbl_parange:        .asciz "SLIME_NT98690 parange    = "
lbl_pfr0:           .asciz "SLIME_NT98690 pfr0       = "
lbl_cntfrq:         .asciz "SLIME_NT98690 cntfrq     = "
lbl_cntpct0:        .asciz "SLIME_NT98690 cntpct0    = "
lbl_calib:          .asciz "SLIME_NT98690 calib      = "
lbl_cntpct1:        .asciz "SLIME_NT98690 cntpct1    = "
lbl_cnt_delta:      .asciz "SLIME_NT98690 cnt_delta  = "
lbl_cnt_hz_est:     .asciz "SLIME_NT98690 cnt_hz_est = "
lbl_sctlr:          .asciz "SLIME_NT98690 sctlr      = "
lbl_hcr_el2:        .asciz "SLIME_NT98690 hcr_el2    = "
lbl_cnthctl:        .asciz "SLIME_NT98690 cnthctl    = "
lbl_cntvoff:        .asciz "SLIME_NT98690 cntvoff    = "
lbl_gicd_ctlr:      .asciz "SLIME_NT98690 gicd_ctlr  = "
lbl_gicd_typer:     .asciz "SLIME_NT98690 gicd_typer = "
lbl_gicd_iidr:      .asciz "SLIME_NT98690 gicd_iidr  = "
lbl_gic_irqs:       .asciz "SLIME_NT98690 gic_irqs   = "

chk_placement:      .asciz "SLIME_NT98690 check placement   = "
chk_el2:            .asciz "SLIME_NT98690 check el2         = "
chk_fdt_magic:      .asciz "SLIME_NT98690 check fdt_magic   = "
chk_mmu_off:        .asciz "SLIME_NT98690 check mmu_off     = "
chk_cnt_advance:    .asciz "SLIME_NT98690 check cnt_advance = "
chk_gicd:           .asciz "SLIME_NT98690 check gicd        = "
