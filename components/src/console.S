.intel_syntax noprefix
.section .text
.global _start
_start:
    sub rsp, 96
recv_loop:
    xor rdi, rdi
    mov rsi, rsp
    lea rdx, [rsp + 64]
    mov rax, 2
    int 0x80
    cmp rax, -3
    je yield_retry
    cmp rax, -2
    je done
    cmp rax, 0
    jl fail
    mov rsi, rax
    mov rdi, rsp
    mov rax, 5
    int 0x80
    jmp recv_loop
yield_retry:
    xor rax, rax
    int 0x80
    jmp recv_loop
done:
    xor rdi, rdi
    mov rax, 3
    int 0x80
fail:
    mov rdi, 1
    mov rax, 3
    int 0x80
