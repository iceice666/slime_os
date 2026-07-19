.intel_syntax noprefix
.include "store_proto.inc"
.section .text
.global _start
# M5.4 object store probe: exercises STAT/GET/PUT through SYS_STORE_TRANSACT
# with the ObjectStore capability (slot 0). Every step prints a fixed marker
# plus the signed status; the host check matches the full pattern per
# fixture variant. Seeded object digest (SHA-256 of the 512-byte fixture
# sector) is embedded as four little-endian qwords.
_start:
    sub rsp, 1280
    lea r12, [rsp]           # request buffer (64)
    lea r13, [rsp + 64]      # reply buffer (64)
    lea r14, [rsp + 128]     # get buffer (512)
    lea r15, [rsp + 640]     # put buffer (512)

    # Fill the put buffer with the deterministic (i*37+11)&0xFF pattern.
    xor ecx, ecx
fill_pattern:
    mov eax, ecx
    imul eax, eax, 37
    add eax, 11
    mov BYTE PTR [r15 + rcx], al
    inc ecx
    cmp ecx, 512
    jne fill_pattern

    # 1. STAT the seeded object identity.
    mov al, STORE_OP_STAT
    call new_request
    call set_seeded_hash
    call store_call
    lea rdi, [rip + m_stat_seeded]
    mov rsi, m_stat_seeded_end - m_stat_seeded
    call print_str
    mov eax, DWORD PTR [r13 + OFF_REPLY_STATUS]
    call print_status
    call print_newline

    # 2. GET the seeded object and verify type, length, and content hash.
    mov al, STORE_OP_GET
    call new_request
    call set_seeded_hash
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_ADDR], r14
    mov DWORD PTR [r12 + OFF_REQUEST_PAYLOAD_LEN], 512
    call store_call
    lea rdi, [rip + m_get_seeded]
    mov rsi, m_get_seeded_end - m_get_seeded
    call print_str
    mov eax, DWORD PTR [r13 + OFF_REPLY_STATUS]
    call print_status
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne seeded_done
    cmp DWORD PTR [r13 + OFF_REPLY_OBJ_TYPE], 1
    jne seeded_bad
    cmp DWORD PTR [r13 + OFF_REPLY_PAYLOAD_LEN], 512
    jne seeded_bad
    lea rdi, [r14]
    lea rsi, [rsp + 1152]
    call sha256_sector
    mov rax, QWORD PTR [rsp + 1152]
    movabs rbx, 0x66d51817d9904ea1
    cmp rax, rbx
    jne seeded_bad
    mov rax, QWORD PTR [rsp + 1152 + 8]
    movabs rbx, 0xc570fad74052e1be
    cmp rax, rbx
    jne seeded_bad
    mov rax, QWORD PTR [rsp + 1152 + 16]
    movabs rbx, 0xcc2ef58fbf3f8cff
    cmp rax, rbx
    jne seeded_bad
    mov rax, QWORD PTR [rsp + 1152 + 24]
    movabs rbx, 0x17430ffc09fe2801
    cmp rax, rbx
    jne seeded_bad
    lea rdi, [rip + m_hash_ok]
    mov rsi, m_hash_ok_end - m_hash_ok
    call print_str
    jmp seeded_done
seeded_bad:
    lea rdi, [rip + m_hash_bad]
    mov rsi, m_hash_bad_end - m_hash_bad
    call print_str
seeded_done:
    call print_newline

    # 3. PUT a new object (type 7, 512-byte pattern payload).
    mov al, STORE_OP_PUT
    call new_request
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_ADDR], r15
    mov DWORD PTR [r12 + OFF_REQUEST_OBJ_TYPE], 7
    mov DWORD PTR [r12 + OFF_REQUEST_PAYLOAD_LEN], 512
    call store_call
    lea rdi, [rip + m_put_new]
    mov rsi, m_put_new_end - m_put_new
    call print_str
    mov eax, DWORD PTR [r13 + OFF_REPLY_STATUS]
    call print_status
    call print_newline
    # Save the returned identity (zeros when the put failed).
    mov rax, QWORD PTR [r13 + OFF_REPLY_HASH0]
    mov QWORD PTR [rsp + 1184], rax
    mov rax, QWORD PTR [r13 + OFF_REPLY_HASH1]
    mov QWORD PTR [rsp + 1184 + 8], rax
    mov rax, QWORD PTR [r13 + OFF_REPLY_HASH2]
    mov QWORD PTR [rsp + 1184 + 16], rax
    mov rax, QWORD PTR [r13 + OFF_REPLY_HASH3]
    mov QWORD PTR [rsp + 1184 + 24], rax

    # 4. GET the new object back and compare bytes.
    mov al, STORE_OP_GET
    call new_request
    mov rax, QWORD PTR [rsp + 1184]
    mov QWORD PTR [r12 + OFF_REQUEST_HASH0], rax
    mov rax, QWORD PTR [rsp + 1184 + 8]
    mov QWORD PTR [r12 + OFF_REQUEST_HASH1], rax
    mov rax, QWORD PTR [rsp + 1184 + 16]
    mov QWORD PTR [r12 + OFF_REQUEST_HASH2], rax
    mov rax, QWORD PTR [rsp + 1184 + 24]
    mov QWORD PTR [r12 + OFF_REQUEST_HASH3], rax
    mov QWORD PTR [r12 + OFF_REQUEST_BUFFER_ADDR], r14
    mov DWORD PTR [r12 + OFF_REQUEST_PAYLOAD_LEN], 512
    call store_call
    lea rdi, [rip + m_get_new]
    mov rsi, m_get_new_end - m_get_new
    call print_str
    mov eax, DWORD PTR [r13 + OFF_REPLY_STATUS]
    call print_status
    cmp DWORD PTR [r13 + OFF_REPLY_STATUS], 0
    jne new_done
    cmp DWORD PTR [r13 + OFF_REPLY_PAYLOAD_LEN], 512
    jne new_bad
    xor ecx, ecx
compare_new:
    mov al, BYTE PTR [r14 + rcx]
    cmp al, BYTE PTR [r15 + rcx]
    jne new_bad
    inc ecx
    cmp ecx, 512
    jne compare_new
    lea rdi, [rip + m_bytes_ok]
    mov rsi, m_bytes_ok_end - m_bytes_ok
    call print_str
    jmp new_done
new_bad:
    lea rdi, [rip + m_bytes_bad]
    mov rsi, m_bytes_bad_end - m_bytes_bad
    call print_str
new_done:
    call print_newline

    # 5. STAT an unknown identity; must report not-found when the store opens.
    mov al, STORE_OP_STAT
    call new_request
    movabs rax, 0xdeadbeefdeadbeef
    mov QWORD PTR [r12 + OFF_REQUEST_HASH0], rax
    mov QWORD PTR [r12 + OFF_REQUEST_HASH1], rax
    mov QWORD PTR [r12 + OFF_REQUEST_HASH2], rax
    mov QWORD PTR [r12 + OFF_REQUEST_HASH3], rax
    call store_call
    lea rdi, [rip + m_stat_unknown]
    mov rsi, m_stat_unknown_end - m_stat_unknown
    call print_str
    mov eax, DWORD PTR [r13 + OFF_REPLY_STATUS]
    call print_status
    call print_newline

    lea rdi, [rip + m_done]
    mov rsi, m_done_end - m_done
    call print_str
    xor rdi, rdi
    mov rax, 3
    int 0x80

# store_call: issue SYS_STORE_TRANSACT (rax=7) with the request at r12 and
# the reply at r13; hard-fails on syscall or reply-validation errors.
store_call:
    xor rdi, rdi
    mov rsi, r12
    mov rdx, r13
    mov rax, 7
    int 0x80
    test rax, rax
    js hard_fail
    mov rdi, r13
    mov rsi, STORE_REPLY_LEN
    STORE_VALIDATE_REPLY rdi, rsi, hard_fail
    ret

# new_request: zero the request buffer and set magic/version; op in al.
new_request:
    xor ecx, ecx
zero_request:
    mov QWORD PTR [r12 + rcx], 0
    add ecx, 8
    cmp ecx, STORE_REQUEST_LEN
    jne zero_request
    mov DWORD PTR [r12 + OFF_REQUEST_MAGIC], STORE_MAGIC
    mov DWORD PTR [r12 + OFF_REQUEST_VERSION], STORE_FORMAT_VERSION
    mov BYTE PTR [r12 + OFF_REQUEST_OP], al
    ret

# set_seeded_hash: write the seeded object identity into the request.
set_seeded_hash:
    movabs rax, 0x66d51817d9904ea1
    mov QWORD PTR [r12 + OFF_REQUEST_HASH0], rax
    movabs rax, 0xc570fad74052e1be
    mov QWORD PTR [r12 + OFF_REQUEST_HASH1], rax
    movabs rax, 0xcc2ef58fbf3f8cff
    mov QWORD PTR [r12 + OFF_REQUEST_HASH2], rax
    movabs rax, 0x17430ffc09fe2801
    mov QWORD PTR [r12 + OFF_REQUEST_HASH3], rax
    ret

# print_str: debug-write rsi bytes from rdi.
print_str:
    mov rax, 5
    int 0x80
    ret

# print_status: print the signed single-digit status in eax (no newline).
print_status:
    sub rsp, 8
    mov BYTE PTR [rsp], 0
    test eax, eax
    jns status_positive
    mov BYTE PTR [rsp], 45
    neg eax
status_positive:
    cmp eax, 9
    jbe status_digit
    mov al, 63
    jmp status_emit
status_digit:
    add al, 48
status_emit:
    mov BYTE PTR [rsp + 1], al
    lea rdi, [rsp]
    mov rsi, 2
    cmp BYTE PTR [rsp], 0
    jne status_write
    inc rdi
    mov rsi, 1
status_write:
    mov rax, 5
    int 0x80
    add rsp, 8
    ret

print_newline:
    lea rdi, [rip + m_newline]
    mov rsi, m_newline_end - m_newline
    jmp print_str

hard_fail:
    lea rdi, [rip + m_error]
    mov rsi, m_error_end - m_error
    call print_str
    mov rdi, 1
    mov rax, 3
    int 0x80

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

m_stat_seeded:
    .ascii "[store-probe] stat-seeded "
m_stat_seeded_end:
m_get_seeded:
    .ascii "[store-probe] get-seeded "
m_get_seeded_end:
m_put_new:
    .ascii "[store-probe] put-new "
m_put_new_end:
m_get_new:
    .ascii "[store-probe] get-new "
m_get_new_end:
m_stat_unknown:
    .ascii "[store-probe] stat-unknown "
m_stat_unknown_end:
m_hash_ok:
    .ascii " hash-ok"
m_hash_ok_end:
m_hash_bad:
    .ascii " hash-bad"
m_hash_bad_end:
m_bytes_ok:
    .ascii " bytes-ok"
m_bytes_ok_end:
m_bytes_bad:
    .ascii " bytes-bad"
m_bytes_bad_end:
m_newline:
    .ascii "\n"
m_newline_end:
m_done:
    .ascii "[store-probe] done\n"
m_done_end:
m_error:
    .ascii "[store-probe] syscall-error\n"
m_error_end:
