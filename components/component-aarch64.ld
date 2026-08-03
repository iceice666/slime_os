/* Slime OS component link script (contracts/component/v2).
 *
 * Every component links at the kernel's component load base (ENTRY_VA in
 * kernel/src/task/mod.rs); each component runs in its own address space, so
 * one fixed base serves all of them and no relocation or PIC is ever needed.
 * The image converter in scripts/build/build-generation.py re-bases segments to
 * offsets relative to this base and asserts the base matches.
 *
 * --build-id=none (passed by the builder) keeps the link deterministic:
 * identical input objects produce byte-identical ELFs.
 */
OUTPUT_FORMAT(elf64-littleaarch64)
OUTPUT_ARCH(aarch64)
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
    /* rust-lld synthesizes `_end` as a reserved symbol even under `-T`, so
     * defining it here changes no address. It is stated explicitly because the
     * runtime derives real addresses from it — `slime-rt`'s seL4 transport
     * places the IPC buffer on the first granule past `_end` and the transfer
     * window on the next — and an address that load-bearing should be visible
     * in the script that fixes it rather than inherited silently.
     *
     * The page alignment is what makes those two derived addresses land on the
     * granules `slime-root/src/child_vspace.rs` maps, instead of inside the
     * last page of `.bss`.
     */
    . = ALIGN(4096);
    _end = .;
    /DISCARD/ : { *(.note*) *(.comment) *(.eh_frame) *(.eh_frame_hdr) }
}
