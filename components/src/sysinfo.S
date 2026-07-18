.intel_syntax noprefix
.section .text
.global _start
_start:
    lea rsi, [rip + info]
    mov rdx, info_end - info
    xor rdi, rdi
    xor r10, r10
    xor r8, r8
    mov rax, 1
    int 0x80
    xor rdi, rdi
    mov rax, 3
    int 0x80
info:
    .ascii "sysinfo{arch=x86_64,target=qemu}\n"
info_end:
