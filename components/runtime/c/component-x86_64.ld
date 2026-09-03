OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    . = 0x400000;
    .text : { *(.text._start) *(.text) *(.text.*) } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata) *(.rodata.*) } :rodata
    . = ALIGN(4096);
    .data : { *(.data) *(.data.*) *(.got) *(.got.*) *(.got.plt) } :data
    . = ALIGN(4096);
    .bss : { *(.bss) *(.bss.*) *(COMMON) } :data
    . = ALIGN(4096);
    _end = .;
    /DISCARD/ : { *(.note*) *(.comment) *(.eh_frame) *(.eh_frame_hdr) }
}
