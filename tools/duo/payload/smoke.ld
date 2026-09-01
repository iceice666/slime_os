/*
 * Link the smoke payload well clear of both the firmware reservation and the
 * addresses stock U-Boot uses for its own images.
 *
 * CV1800B DRAM window (from the running board's device tree):
 *   base 0x80000000, size 0x03f40000  -> 0x80000000 .. 0x83f40000
 * mmode_resv0 (OpenSBI) occupies 0x80000000 + 0x00040000.
 * Stock U-Boot loads boot.sd at ${uImage_addr}; we stay above it.
 */
ENTRY(_start)

PAYLOAD_BASE = 0x82000000;

SECTIONS
{
    . = PAYLOAD_BASE;

    .text : {
        KEEP(*(.text.start))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(8) {
        *(.data .data.*)
    }

    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.* COMMON)
        __bss_end = .;
    }

    . = ALIGN(16);
    /* small boot stack; the payload only nests a few calls deep */
    . = . + 0x1000;
    __stack_top = .;

    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame) }
}
