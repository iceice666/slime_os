OUTPUT_FORMAT(elf64-x86-64)
OUTPUT_ARCH(i386:x86-64)

ENTRY(_start)

PHDRS
{
    text     PT_LOAD    FLAGS(0x05); /* Execute + Read */
    rodata   PT_LOAD    FLAGS(0x04); /* Read only */
    data     PT_LOAD    FLAGS(0x06); /* Write + Read */
    dynamic  PT_DYNAMIC FLAGS(0x06); /* Dynamic PHDR for relocations */
}

SECTIONS
{
    /* Higher-half kernel: load at the top 2 GiB of the canonical
       high virtual address space. Limine maps this to a physical
       load address it picks itself. */
    . = 0xffffffff80000000;

    . = ALIGN(256);
    __start_text = .;
    .text : {
        *(.text .text.*)
    } :text
    . = ALIGN(4096);
    __stop_text = .;

    __start_rodata = .;
    .rodata : {
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(16);
    } :rodata
    .dynstr : { *(.dynstr) } :rodata
    .dynsym : { *(.dynsym) } :rodata
    .rela.dyn : { *(.rela.dyn) } :rodata
    . = ALIGN(4096);
    __stop_rodata = .;

    __start_data = .;
    .dynamic : { *(.dynamic) } :data :dynamic
    .got : { *(.got) } :data
    .got.plt : { *(.got.plt) } :data
    .data : {
        /* Limine request markers and requests. KEEP so --gc-sections
           cannot strip them before the bootloader scans the ELF. */
        KEEP(*(.requests_start))
        KEEP(*(.requests))
        KEEP(*(.requests_end))
        *(.data .data.* .sdata .sdata.*)
    } :data

    /* Zero-initialized memory last, to avoid padding the file. */
    .bss : {
        *(.bss .bss.* .sbss .sbss.*)
        *(COMMON)
    } :data
    . = ALIGN(4096);
    __stop_data = .;

    /DISCARD/ : {
        *(.eh_frame*)
        *(.note .note.*)
        *(.interp)
        *(.comment)
    }
}
