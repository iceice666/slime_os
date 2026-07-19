.intel_syntax noprefix
.include "block_proto.inc"
.section .text
.global _start
_start:
    sub rsp, 1280
    lea r12, [rsp]
    lea r13, [rsp + 64]
    lea r14, [rsp + 128]
    lea r15, [rsp + 640]
    # Build one schema-defined read request for LBA 0 into the 512-byte buffer.
    mov DWORD PTR [r12 + OFF_REQUEST_MAGIC], BLOCK_MAGIC
    mov DWORD PTR [r12 + OFF_REQUEST_VERSION], BLOCK_FORMAT_VERSION
    mov BYTE PTR [r12 + OFF_REQUEST_OP], BLOCK_OP_READ
    mov BYTE PTR [r12 + OFF_REQUEST_FLAGS], 0
    mov WORD PTR [r12 + OFF_REQUEST_RESERVED], 0
    mov QWORD PTR [r12 + OFF_REQUEST_LBA], 0
    mov DWORD PTR [r12 + OFF_REQUEST_SECTOR_COUNT], 1
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 1
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_PHYS], r14

    xor rdi, rdi
    mov rsi, r12
    mov rdx, r13
    mov rax, 6
    int 0x80
    test rax, rax
    js fail

    mov rdi, r13
    mov rsi, BLOCK_REPLY_LEN
    BLOCK_VALIDATE_REPLY rdi, rsi, fail
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne fail
    cmp DWORD PTR [r13 + OFF_REPLY_SECTORS_DONE], 1
    jne fail

    # The fixture begins with this stable little-endian prefix.
    mov rax, QWORD PTR [r14]
    movabs rbx, 0x534f20656d696c53
    cmp rax, rbx
    jne fail

    # SHA-256 the full sector in userspace and compare the fixed digest.
    lea rdi, [r14]
    lea rsi, [r15]
    call sha256_sector
    mov rax, QWORD PTR [r15]
    movabs rbx, 0x9052e5c5e75a43d2
    cmp rax, rbx
    jne fail
    mov rax, QWORD PTR [r15 + 8]
    movabs rbx, 0x7b4979c4668039d0
    cmp rax, rbx
    jne fail
    mov rax, QWORD PTR [r15 + 16]
    movabs rbx, 0xe47d08d23efaa9cb
    cmp rax, rbx
    jne fail
    mov rax, QWORD PTR [r15 + 24]
    movabs rbx, 0x67fae649e7bf6785
    cmp rax, rbx
    jne fail

    # The read-only capability cannot authorize a write request.
    mov BYTE PTR [r12 + OFF_REQUEST_OP], BLOCK_OP_WRITE
    call block_call_expect_bad_cap

    # A buffer declaration smaller than the payload is rejected before I/O.
    mov BYTE PTR [r12 + OFF_REQUEST_OP], BLOCK_OP_READ
    mov DWORD PTR [r12 + OFF_REQUEST_SECTOR_COUNT], 9
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 1
    call block_call
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -5
    jne fail

    # The device capacity is eight sectors; LBA 8 returns out-of-range.
    mov DWORD PTR [r12 + OFF_REQUEST_SECTOR_COUNT], 1
    mov DWORD PTR [r12 + OFF_REQUEST_BUFFER_PAGES], 1
    mov QWORD PTR [r12 + OFF_REQUEST_LBA], 8
    call block_call
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], -3
    jne fail

    lea rdi, [rip + ok]
    mov rsi, ok_end - ok
    mov rax, 5
    int 0x80
    xor rdi, rdi
    mov rax, 3
    int 0x80
fail:
    lea rdi, [rip + failed]
    mov rsi, failed_end - failed
    mov rax, 5
    int 0x80
    mov rdi, 1
    mov rax, 3
    int 0x80
block_call_expect_bad_cap:
    xor rdi, rdi
    mov rsi, r12
    mov rdx, r13
    mov rax, 6
    int 0x80
    cmp rax, -1
    jne fail
    ret
block_call:
    xor rdi, rdi
    mov rsi, r12
    mov rdx, r13
    mov rax, 6
    int 0x80
    test rax, rax
    js fail
    mov rdi, r13
    mov rsi, BLOCK_REPLY_LEN
    BLOCK_VALIDATE_REPLY rdi, rsi, fail
    ret

sha256_sector:
    # rdi=input (512 bytes), rsi=32-byte output. Uses a 64-word schedule.
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15
    sub rsp, 256
    mov r12, rdi
    mov r13, rsi
    mov r14, 0x6a09e667
    mov r15, 0xbb67ae85
    mov ebx, 0x3c6ef372
    mov ebp, 0xa54ff53a
    mov r8d, 0x510e527f
    mov r9d, 0x9b05688c
    mov r10d, 0x1f83d9ab
    mov r11d, 0x5be0cd19
    xor ecx, ecx
.sha_block:
    cmp ecx, 9
    je .sha_done
    xor edi, edi

.sha_load:
    cmp edi, 16
    je .sha_extend
    cmp ecx, 8
    jne .sha_data_word
    xor eax, eax
    cmp edi, 0
    jne .sha_pad_length
    mov eax, 0x80000000
    jmp .sha_store_word
.sha_pad_length:
    cmp edi, 15
    jne .sha_store_word
    mov eax, 0x00001000
    jmp .sha_store_word
.sha_data_word:
    mov eax, ecx
    shl eax, 6
    lea rdx, [r12 + rax]
    mov eax, DWORD PTR [rdx + rdi * 4]
    bswap eax
.sha_store_word:
    mov DWORD PTR [rsp + rdi * 4], eax
    inc edi
    jmp .sha_load
.sha_extend:
    mov edi, 16
.sha_extend_loop:
    cmp edi, 64
    je .sha_rounds
    mov eax, DWORD PTR [rsp + rdi * 4 - 60]
    mov edx, eax
    ror eax, 7
    ror edx, 18
    xor eax, edx
    mov edx, DWORD PTR [rsp + rdi * 4 - 60]
    shr edx, 3
    xor eax, edx
    mov esi, DWORD PTR [rsp + rdi * 4 - 8]
    mov edx, esi
    ror esi, 17
    ror edx, 19
    xor esi, edx
    mov edx, DWORD PTR [rsp + rdi * 4 - 8]
    shr edx, 10
    xor esi, edx
    add eax, DWORD PTR [rsp + rdi * 4 - 64]
    add eax, DWORD PTR [rsp + rdi * 4 - 28]
    add eax, esi
    mov DWORD PTR [rsp + rdi * 4], eax
    inc edi
    jmp .sha_extend_loop
.sha_rounds:
    sub rsp, 32
    mov DWORD PTR [rsp], r14d
    mov DWORD PTR [rsp + 4], r15d
    mov DWORD PTR [rsp + 8], ebx
    mov DWORD PTR [rsp + 12], ebp
    mov DWORD PTR [rsp + 16], r8d
    mov DWORD PTR [rsp + 20], r9d
    mov DWORD PTR [rsp + 24], r10d
    mov DWORD PTR [rsp + 28], r11d
    xor edi, edi
.sha_round_loop:
    cmp edi, 64
    je .sha_accumulate
    mov eax, r8d
    ror eax, 6
    mov edx, r8d
    ror edx, 11
    xor eax, edx
    mov edx, r8d
    ror edx, 25
    xor eax, edx
    add eax, r11d
    mov edx, r8d
    and edx, r9d
    mov esi, r8d
    not esi
    and esi, r10d
    xor edx, esi
    add eax, edx
    lea rdx, [rip + sha_k]
    add eax, DWORD PTR [rdx + rdi * 4]
    add eax, DWORD PTR [rsp + 32 + rdi * 4]
    mov edx, r14d
    ror edx, 2
    mov esi, r14d
    ror esi, 13
    xor edx, esi
    mov esi, r14d
    ror esi, 22
    xor edx, esi
    mov esi, r14d
    and esi, r15d
    mov r11d, r14d
    and r11d, ebx
    xor esi, r11d
    mov r11d, r15d
    and r11d, ebx
    xor esi, r11d
    add edx, esi
    mov r11d, r10d
    mov r10d, r9d
    mov r9d, r8d
    lea r8d, [rbp + rax]
    mov ebp, ebx
    mov ebx, r15d
    mov r15d, r14d
    lea r14d, [rax + rdx]
    inc edi
    jmp .sha_round_loop
.sha_accumulate:
    add r14d, DWORD PTR [rsp]
    add r15d, DWORD PTR [rsp + 4]
    add ebx, DWORD PTR [rsp + 8]
    add ebp, DWORD PTR [rsp + 12]
    add r8d, DWORD PTR [rsp + 16]
    add r9d, DWORD PTR [rsp + 20]
    add r10d, DWORD PTR [rsp + 24]
    add r11d, DWORD PTR [rsp + 28]
    add rsp, 32
    inc ecx
    jmp .sha_block
.sha_done:
    mov eax, r14d
    bswap eax
    mov DWORD PTR [r13], eax
    mov eax, r15d
    bswap eax
    mov DWORD PTR [r13 + 4], eax
    mov eax, ebx
    bswap eax
    mov DWORD PTR [r13 + 8], eax
    mov eax, ebp
    bswap eax
    mov DWORD PTR [r13 + 12], eax
    mov eax, r8d
    bswap eax
    mov DWORD PTR [r13 + 16], eax
    mov eax, r9d
    bswap eax
    mov DWORD PTR [r13 + 20], eax
    mov eax, r10d
    bswap eax
    mov DWORD PTR [r13 + 24], eax
    mov eax, r11d
    bswap eax
    mov DWORD PTR [r13 + 28], eax
    add rsp, 256
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret

.align 4
sha_k:
    .long 0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5
    .long 0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5
    .long 0xd807aa98,0x12835b01,0x243185be,0x550c7dc3
    .long 0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174
    .long 0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc
    .long 0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da
    .long 0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7
    .long 0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967
    .long 0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13
    .long 0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85
    .long 0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3
    .long 0xd192e819,0xd6990624,0xf40e3585,0x106aa070
    .long 0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5
    .long 0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3
    .long 0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208
    .long 0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2

ok:
    .ascii "[storage-probe] sector 0 verified\n"
ok_end:
failed:
    .ascii "[storage-probe] failed\n"
failed_end:
