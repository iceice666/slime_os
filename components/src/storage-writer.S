.intel_syntax noprefix
.include "block_proto.inc"
.section .text
.global _start
_start:
    sub rsp, 1024
    lea r12, [rsp]
    lea r13, [rsp + 64]
    lea r14, [rsp + 512]

    # Read sector 2 first. Existing content means this is boot #2: verify only.
    mov al, BLOCK_OP_READ
    mov rbx, 2
    mov ecx, 1
    mov rdx, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail
    movabs rax, 0x4d20332e354d534c
    cmp QWORD PTR [r14], rax
    je verify_existing
    mov QWORD PTR [r14], rax
    mov QWORD PTR [r14 + 8], 0x00000000454c4241
    mov rcx, 16
fill_payload:
    cmp rcx, 512
    je write_payload
    mov rax, rcx
    imul rax, 29
    add rax, 7
    mov BYTE PTR [r14 + rcx], al
    inc rcx
    jmp fill_payload
write_payload:
    mov al, BLOCK_OP_WRITE
    mov rbx, 2
    mov ecx, 1
    mov rdx, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail

    mov al, BLOCK_OP_FLUSH
    xor ebx, ebx
    xor ecx, ecx
    xor edx, edx
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail

    # Clear and read back immediately.
    xor rax, rax
    mov rdi, r14
    mov rcx, 64
    rep stosq
verify_existing:
    mov al, BLOCK_OP_READ
    mov rbx, 2
    mov ecx, 1
    mov rdx, r14
    call block_request
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail
    movabs rax, 0x4d20332e354d534c
    cmp QWORD PTR [r14], rax
    jne fail
    cmp QWORD PTR [r14 + 8], 0x00000000454c4241
    jne fail

    lea rdi, [rip + ok]
    mov rsi, ok_end - ok
    mov rax, 5
    int 0x80
    xor rdi, rdi
    mov rax, 3
    int 0x80

block_request:
    mov DWORD PTR [r12 + OFF_REQUEST_MAGIC], BLOCK_MAGIC
    mov DWORD PTR [r12 + OFF_REQUEST_VERSION], BLOCK_FORMAT_VERSION
    mov BYTE PTR [r12 + OFF_REQUEST_OP], al
    mov BYTE PTR [r12 + OFF_REQUEST_FLAGS], 0
    mov WORD PTR [r12 + OFF_REQUEST_RESERVED], 0
    mov QWORD PTR [r12 + OFF_REQUEST_LBA], rbx
    mov DWORD PTR [r12 + OFF_REQUEST_SECTOR_COUNT], ecx
    test ecx, ecx
    jz no_payload
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 1
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_PHYS], rdx
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
    .ascii "[storage-writer] durable sector verified\n"
ok_end:
failed:
    .ascii "[storage-writer] failed\n"
failed_end:
