.intel_syntax noprefix
.section .text
.global _start
_start:
    lea rsi, [rip + reply]
    mov rdx, reply_end - reply
    xor rdi, rdi
    xor r10, r10
    xor r8, r8
    mov rax, 1
    int 0x80
    xor rdi, rdi
    mov rax, 3
    int 0x80
reply:
    .ascii "echo-agent{tool=echo,result=ok}\n"
reply_end:
