/* Slime OS component link script (contracts/component/v1).
 *
 * Every component links at the fixed component load base declared by its target
 * profile (`componentBase` in contracts/target-profile/v1); each component runs
 * in its own address space, so one fixed base serves all of them and no
 * relocation or PIC is ever needed. The image converter in
 * scripts/build/build-generation.py re-bases segments to offsets relative to this
 * base and asserts the base matches. The seL4 product profile uses no linker
 * script at all: it carries a whole ELF loaded at its own link addresses.
 *
 * --build-id=none (passed by the builder) keeps the link deterministic:
 * identical input objects produce byte-identical ELFs.
 */
OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)
ENTRY(_start)

SECTIONS
{
    . = 0x400000;
    .text : { *(.text) *(.text.*) }
    . = ALIGN(4096);
    .rodata : { *(.rodata) *(.rodata.*) }
    . = ALIGN(4096);
    .data : { *(.data) *(.data.*) }
    . = ALIGN(4096);
    .bss : { *(.bss) *(.bss.*) *(COMMON) }
    /DISCARD/ : { *(.note*) *(.comment) *(.eh_frame) *(.eh_frame_hdr) }
}
