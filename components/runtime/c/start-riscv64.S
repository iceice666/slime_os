.section .text._start,"ax",@progbits
.global _start
.type _start, @function
_start:
    la sp, __slime_component_stack_top
    call slime_component_start
1:
    wfi
    j 1b
.size _start, . - _start

.section .bss.slime_component_stack,"aw",@nobits
.balign 16
__slime_component_stack:
    .skip 65536
__slime_component_stack_top:
