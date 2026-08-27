.section .text._start,"ax",@progbits
.global _start
.type _start, %function
_start:
    adrp x1, __slime_component_stack_top
    add x1, x1, :lo12:__slime_component_stack_top
    mov sp, x1
    bl slime_component_start
1:
    yield
    b 1b
.size _start, . - _start

.section .bss.slime_component_stack,"aw",@nobits
.balign 16
__slime_component_stack:
    .skip 65536
__slime_component_stack_top:
