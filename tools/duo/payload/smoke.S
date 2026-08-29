/*
 * Minimal S-mode smoke payload for the Milk-V Duo (CV1800B, T-Head C906).
 *
 * Proves the whole deployment loop is real: U-Boot hands control to our own
 * code in S-mode, and that code talks to the world through SBI. If this prints,
 * the same handoff can carry an seL4 elfloader.
 *
 * Entry contract mirrors what an seL4 elfloader receives from a booti/bootm
 * style handoff:
 *   a0 = hart id
 *   a1 = DTB physical address
 * Both are preserved and reported, since a real port must trust them.
 *
 * Output goes through SBI legacy console_putchar (EID 0x01) -- the exact call
 * seL4 16.0.0 itself uses, so a successful print also proves seL4's console
 * path works on this firmware.
 */

    .section .text.start, "ax"
    .globl _start
_start:
    /*
     * U-Boot's arch/riscv/lib/bootm.c enters us as a plain C call:
     *     kernel(hart_id, fdt_addr)
     * so `ra` still points back into U-Boot. Preserve it (and U-Boot's sp) and
     * we can simply `ret` at the end to land back at the U-Boot prompt --
     * no SBI reset required, and nothing can strand the board.
     */
    mv      s2, ra
    mv      s3, sp

    /* Establish our own stack before any call: U-Boot's sp is not ours. */
    la      sp, __stack_top

    /* zero .bss so the payload starts from a defined state */
    la      t0, __bss_start
    la      t1, __bss_end
1:
    bgeu    t0, t1, 2f
    sd      zero, 0(t0)
    addi    t0, t0, 8
    j       1b
2:

    /* keep the firmware handoff registers */
    mv      s0, a0                  /* hart id  */
    mv      s1, a1                  /* dtb addr */

    la      a0, msg_hello
    call    puts

    /* report hart id */
    la      a0, msg_hart
    call    puts
    mv      a0, s0
    call    put_hex
    call    put_nl

    /* report the DTB pointer the firmware gave us */
    la      a0, msg_dtb
    call    puts
    mv      a0, s1
    call    put_hex
    call    put_nl

    /* prove we are in S-mode by reading a S-mode-only CSR.
     * sstatus is inaccessible from U-mode; reaching here already implies
     * S-mode or M-mode, and satp distinguishes a real supervisor context. */
    la      a0, msg_satp
    call    puts
    csrr    a0, satp
    call    put_hex
    call    put_nl

    /* read the S-mode timer via rdtime: this is the path seL4 uses for
     * timestamps, so a sane changing value matters. */
    la      a0, msg_time
    call    puts
    rdtime  a0
    call    put_hex
    call    put_nl

    la      a0, msg_marker
    call    puts

    /*
     * Exit by RETURNING INTO U-BOOT.
     *
     * Two exits were tried and both strand the board:
     *   - SBI legacy SHUTDOWN (EID 8) halts the hart: console dies, USB gadget
     *     dies, only a physical power-cycle recovers it.
     *   - SBI SRST (0x53525354) is NOT implemented by this board's OpenSBI 0.9:
     *     the ecall simply returns, falling through to a hang with the same
     *     result.
     *
     * bootm entered us as `kernel(hart_id, fdt_addr)`, so `ra` still points
     * into U-Boot. Restoring U-Boot's ra/sp and returning drops us back at the
     * `cv180x_c906#` prompt, where the harness can `run sdboot` to reach Linux.
     * Nothing can strand the board this way.
     */
    la      a0, msg_return
    call    puts

    mv      sp, s3                  /* U-Boot's stack pointer */
    mv      ra, s2                  /* U-Boot's return address */
    li      a0, 0
    ret

    /* Unreachable: only if U-Boot's ra was not a valid return site. */
hang:
    wfi
    j       hang

/* ---- SBI legacy console putchar: a0 = char ---- */
putc:
    mv      a1, a0
    li      a7, 1                   /* SBI_CONSOLE_PUTCHAR */
    li      a6, 0
    mv      a0, a1
    ecall
    ret

/* ---- puts: a0 = NUL-terminated string ---- */
puts:
    mv      t0, a0
    addi    sp, sp, -16
    sd      ra, 0(sp)
    sd      t0, 8(sp)
1:
    ld      t0, 8(sp)
    lbu     a0, 0(t0)
    beqz    a0, 2f
    addi    t0, t0, 1
    sd      t0, 8(sp)
    call    putc
    j       1b
2:
    ld      ra, 0(sp)
    addi    sp, sp, 16
    ret

put_nl:
    addi    sp, sp, -8
    sd      ra, 0(sp)
    li      a0, '\r'
    call    putc
    li      a0, '\n'
    call    putc
    ld      ra, 0(sp)
    addi    sp, sp, 8
    ret

/* ---- put_hex: a0 = 64-bit value, prints 0x%016lx ---- */
put_hex:
    addi    sp, sp, -32
    sd      ra, 0(sp)
    sd      s2, 8(sp)
    sd      s3, 16(sp)
    mv      s2, a0
    li      a0, '0'
    call    putc
    li      a0, 'x'
    call    putc
    li      s3, 60                  /* shift for the top nibble */
1:
    srl     a0, s2, s3
    andi    a0, a0, 0xf
    li      t1, 10
    blt     a0, t1, 2f
    addi    a0, a0, 'a' - 10
    j       3f
2:
    addi    a0, a0, '0'
3:
    call    putc
    addi    s3, s3, -4
    bgez    s3, 1b
    ld      s3, 16(sp)
    ld      s2, 8(sp)
    ld      ra, 0(sp)
    addi    sp, sp, 32
    ret

    .section .rodata
msg_hello:
    .asciz "\r\n=== SLIME_DUO smoke payload: S-mode entry reached ===\r\n"
msg_hart:
    .asciz "SLIME_DUO hart      = "
msg_dtb:
    .asciz "SLIME_DUO dtb       = "
msg_satp:
    .asciz "SLIME_DUO satp      = "
msg_time:
    .asciz "SLIME_DUO rdtime    = "
msg_marker:
    .asciz "SLIME_DUO PAYLOAD_OK\r\n"
msg_return:
    .asciz "SLIME_DUO returning to U-Boot\r\n"
