.section .text._start,"ax",@progbits
.global _start
.type _start, @function
_start:
    /* The System V AMD64 ABI requires `%rsp` to be 16-byte aligned
       *immediately before* a `call`; the pushed return address then leaves the
       callee seeing `%rsp % 16 == 8`, which is the state its prologue assumes
       when it aligns spill slots and outgoing argument areas.

       `__slime_component_stack_top` is 16-byte aligned by the `.balign` below,
       so the `call` is already correctly aligned and nothing must be
       subtracted first. Subtracting one word here would invert the invariant
       and hand the callee a misaligned frame. */
    leaq __slime_component_stack_top(%rip), %rsp
    call slime_component_start
1:
    hlt
    jmp 1b
.size _start, . - _start

.section .bss.slime_component_stack,"aw",@nobits
.balign 16
__slime_component_stack:
    .skip 65536
__slime_component_stack_top:
