OUTPUT_FORMAT(elf64-littleaarch64)
OUTPUT_ARCH(aarch64)
ENTRY(_start)

SECTIONS
{
    . = 0x400000;
    .text : { *(.text._start) *(.text) *(.text.*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata) *(.rodata.*) }
    . = ALIGN(4096);
    .data : { *(.data) *(.data.*) }
    . = ALIGN(4096);
    .bss : { *(.bss) *(.bss.*) *(COMMON) }
    . = ALIGN(4096);
    _end = .;
    /DISCARD/ : { *(.note*) *(.comment) *(.eh_frame) *(.eh_frame_hdr) }
}
