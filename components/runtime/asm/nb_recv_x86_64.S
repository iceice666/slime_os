# seL4_NBRecv without MCS on x86-64, with the message-info register defined.
#
#   void slime_nb_recv_x86_64(word_t endpoint, word_t out[6], word_t syscall_id);
#
# On return out[0] holds the message info, out[1] the badge, and out[2..6] the
# four message registers. The message info is passed to the kernel as zero: an
# empty NBRecv writes only the badge, so the value read back is then defined.
#
# System V arguments arrive as endpoint in rdi, out in rsi, syscall_id in rdx.
# seL4 takes the same endpoint and syscall number in rdi and rdx, the message
# info in rsi, and an unused reply argument in r12; it returns the message
# registers in r10, r8, r9 and r15. `syscall` overwrites rcx and r11, which are
# caller-saved. The kernel entry does not preserve the stack pointer, so it is
# parked in r14 across the call, as `sel4-sys` does.

.pushsection .text.slime_nb_recv_x86_64,"ax",@progbits
.global slime_nb_recv_x86_64
.type slime_nb_recv_x86_64, @function
slime_nb_recv_x86_64:
    push r12
    push r13
    push r14
    push r15
    mov r13, rsi
    xor esi, esi
    xor r12d, r12d
    mov r14, rsp
    syscall
    mov rsp, r14
    mov [r13], rsi
    mov [r13 + 8], rdi
    mov [r13 + 16], r10
    mov [r13 + 24], r8
    mov [r13 + 32], r9
    mov [r13 + 40], r15
    pop r15
    pop r14
    pop r13
    pop r12
    ret
.size slime_nb_recv_x86_64, . - slime_nb_recv_x86_64
.popsection
