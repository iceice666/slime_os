/*
 * Link the H1V1 probe at the address the board's `booti` will place it at.
 *
 * PAYLOAD_BASE is load-bearing three times over, and the three must agree:
 * it is where `fatload` puts the file, it is the `text_offset` written into
 * the arm64 Image header, and it is where this code is linked to run. The
 * vendor U-Boot forces that agreement: `arch/arm/lib/image.c` under
 * `CONFIG_ARCH_NOVATEK` sets `dst = gd->ram_base` unconditionally rather than
 * honouring the header's "place anywhere" flag, so the image is always moved
 * to `ALIGN(ram_base, 2 MiB) + text_offset` -- and `ram_base` is 0 on this
 * board, making the destination exactly `text_offset`. Loading to any other
 * address means a memmove to here anyway; linking for any other address means
 * running at the wrong one. `check_link_address` in the payload builder holds
 * the three together, and the boot gate fails on U-Boot's `Moving Image` line.
 *
 * 0x10000000 is chosen against the vendor memory map, not for convenience.
 * The board's DTB (`nvt_memory_cfg` and `reserved-memory`) claims 0..0x02000000
 * for the loader, its device tree, SHMINFO, and TF-A's BL31, and 0x04800000..
 * 0x0AC00000 for the media CMA pools; U-Boot itself, its heap, and the FDT copy
 * it relocates for us sit just below 0x80000000. This address clears all of
 * them by 84 MiB below and 1.7 GiB above, is 2 MiB-aligned as the relocation
 * arithmetic requires, and lies inside the 1 GiB the firmware's `/memory` node
 * declares. P6.B's seL4 image reuses it, so the headroom above matters.
 */
ENTRY(_start)

PAYLOAD_BASE = 0x10000000;

SECTIONS
{
    . = PAYLOAD_BASE;

    /*
     * `.text.start` first and KEEPt: its first 64 bytes are the arm64 Image
     * header, which U-Boot reads at the load address before branching. A
     * reordered or garbage-collected section here is a `Bad Linux ARM64 Image
     * magic!` at the prompt.
     */
    .text : {
        KEEP(*(.text.start))
        *(.text .text.*)
    }

    .rodata : ALIGN(8) {
        *(.rodata .rodata.*)
    }

    /*
     * No .data or .bss: the probe keeps its whole state in registers and on
     * this stack, so there is nothing to zero before use and nothing whose
     * initialization could be lost by the flat objcopy.
     */
    . = ALIGN(16);
    . = . + 0x1000;
    __stack_top = .;

    /*
     * `image_size` covers the stack as well as the code. U-Boot reserves that
     * span in its lmb before relocating the FDT, so declaring the stack keeps
     * the device-tree copy from landing on it.
     */
    __image_end = .;
    __image_size = __image_end - PAYLOAD_BASE;

    /DISCARD/ : { *(.comment) *(.note.*) *(.eh_frame) }
}
