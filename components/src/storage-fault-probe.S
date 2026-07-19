.intel_syntax noprefix
.include "block_proto.inc"
.section .text
.global _start
_start:
    sub rsp, 1024
    lea r12, [rsp]
    lea r13, [rsp + 64]
    lea r14, [rsp + 512]
    mov rcx, 512
    mov al, 0x5a
    mov rdi, r14
    rep stosb

    # Every injected request must fail with its structured status and must not
    # poison the next request. Replays consume the exact last recorded input.
    mov al, BLOCK_OP_WRITE
    mov dl, BLOCK_FLAG_INJECT_REQUEST_FAILURE
    mov ebx, 3
    mov ecx, 1
    mov r8, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -7
    jne fail
    call replay_last
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -7
    jne fail

    mov al, BLOCK_OP_READ
    mov dl, BLOCK_FLAG_INJECT_TIMEOUT
    mov ebx, 0
    mov ecx, 1
    mov r8, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -8
    jne fail

    mov al, BLOCK_OP_READ
    mov dl, BLOCK_FLAG_INJECT_RESET
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -7
    jne fail

    mov al, BLOCK_OP_FLUSH
    mov dl, BLOCK_FLAG_INJECT_FLUSH_FAILURE
    xor ebx, ebx
    xor ecx, ecx
    xor r8d, r8d
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -7
    jne fail

    mov al, BLOCK_OP_WRITE
    mov dl, BLOCK_FLAG_INJECT_INTERRUPTED
    mov ebx, 3
    mov ecx, 1
    mov r8, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -7
    jne fail

    # Out-of-range write is rejected before any device data transfer.
    mov al, BLOCK_OP_WRITE
    xor edx, edx
    mov ebx, 8
    mov ecx, 1
    mov r8, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -3
    jne fail

    # The service/device must remain usable after every failure and reset.
    mov al, BLOCK_OP_READ
    xor edx, edx
    xor ebx, ebx
    mov ecx, 1
    mov r8, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail

    lea rdi, [rip + ok]
    mov rsi, ok_end - ok
    mov rax, 5
    int 0x80
    xor rdi, rdi
    mov rax, 3
    int 0x80

replay_last:
    mov al, BLOCK_OP_READ
    mov dl, BLOCK_FLAG_REPLAY_LAST
    xor ebx, ebx
    mov ecx, 1
    mov r8, r14

block_request:
    mov DWORD PTR [r12 + OFF_REQUEST_MAGIC], BLOCK_MAGIC
    mov DWORD PTR [r12 + OFF_REQUEST_VERSION], BLOCK_FORMAT_VERSION
    mov BYTE PTR [r12 + OFF_REQUEST_OP], al
    mov BYTE PTR [r12 + OFF_REQUEST_FLAGS], dl
    mov WORD PTR [r12 + OFF_REQUEST_RESERVED], 0
    mov QWORD PTR [r12 + OFF_REQUEST_LBA], rbx
    mov DWORD PTR [r12 + OFF_REQUEST_SECTOR_COUNT], ecx
    test ecx, ecx
    jz no_payload
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 1
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_PHYS], r8
    jmp issue
no_payload:
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 0
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_PHYS], 0
issue:
    xor rdi, rdi
    mov rsi, r12
    mov rdx, r13
    mov rax, 6
    int 0x80
    test rax, rax
    js fail
    ret

fail:
    lea rdi, [rip + failed]
    mov rsi, failed_end - failed
    mov rax, 5
    int 0x80
    mov rdi, 1
    mov rax, 3
    int 0x80
ok:
    .ascii "[storage-fault-probe] recovery and replay verified\n"
ok_end:
failed:
    .ascii "[storage-fault-probe] failed\n"
failed_end:
