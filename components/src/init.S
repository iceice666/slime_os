.intel_syntax noprefix
.section .text
.global _start
_start:
    lea rdi, [rip + ready]
    mov rsi, ready_end - ready
    mov rax, 5
    int 0x80

    lea rsi, [rip + console_caps]
    mov rdi, 0
    mov rdx, 1
    mov r10, 1
    mov rax, 4
    int 0x80
    test rax, rax
    js fail

    lea rsi, [rip + dango_caps]
    mov rdi, 2
    mov rdx, 3
    mov r10, 2
    mov rax, 4
    int 0x80
    test rax, rax
    js fail

    lea rsi, [rip + sysinfo_caps]
    mov rdi, 6
    mov rdx, 1
    mov r10, 3
    mov rax, 4
    int 0x80
    test rax, rax
    js fail

    lea rsi, [rip + echo_caps]
    mov rdi, 8
    mov rdx, 1
    mov r10, 4
    mov rax, 4
    int 0x80
    test rax, rax
    js fail

    xor rdi, rdi
    mov rax, 3
    int 0x80
fail:
    mov rdi, 1
    mov rax, 3
    int 0x80
ready:
    .ascii "[init] launching component graph\n"
ready_end:
console_caps:
    .long 1
dango_caps:
    .long 3, 4, 5
sysinfo_caps:
    .long 7
echo_caps:
    .long 9
