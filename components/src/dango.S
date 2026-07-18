.intel_syntax noprefix
.section .text
.global _start
_start:
    sub rsp, 160
    lea rdi, [rip + sysinfo_request]
    mov rsi, sysinfo_request_end - sysinfo_request
    call send_console
    lea rdi, [rip + echo_request]
    mov rsi, echo_request_end - echo_request
    call send_console

    xor rdi, rdi
    call receive_result
    mov rdi, 1
    call receive_result
    xor rdi, rdi
    mov rax, 3
    int 0x80
receive_result:
    mov r9, rdi
    sub rsp, 96
receive_retry:
    mov rdi, r9
    mov rsi, rsp
    lea rdx, [rsp + 64]
    mov rax, 2
    int 0x80
    cmp rax, -3
    je yield_retry
    cmp rax, 0
    jl fail
    mov rsi, rax
    mov rdi, rsp
    call send_console
    add rsp, 96
    ret
yield_retry:
    xor rax, rax
    int 0x80
    jmp receive_retry
fail:
    mov rdi, 1
    mov rax, 3
    int 0x80
send_console:
    mov rdx, rsi
    mov rsi, rdi
    mov rdi, 2
    xor r10, r10
    xor r8, r8
    mov rax, 1
    int 0x80
    ret
sysinfo_request:
    .ascii "[dango] resolve sysinfo via capability\n"
sysinfo_request_end:
echo_request:
    .ascii "[dango] echo-agent tool call\n"
echo_request_end:
