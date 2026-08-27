#include <slime/component_runtime.h>

extern unsigned char _end[];


static uintptr_t align_up(uintptr_t value, uintptr_t alignment)
{
    return (value + alignment - 1U) & ~(alignment - 1U);
}

static uint64_t descriptor(size_t len, uint64_t form, uint32_t thread)
{
    return ((uint64_t)len << SLIME_DESCRIPTOR_LENGTH_SHIFT)
        | (form << SLIME_DESCRIPTOR_FORM_SHIFT)
        | ((uint64_t)thread << SLIME_DESCRIPTOR_THREAD_SHIFT);
}

static uint64_t pack_word(const uint8_t *bytes, size_t len)
{
    uint64_t word = 0;
    size_t index;
    for (index = 0; index < len; ++index) {
        word |= (uint64_t)bytes[index] << (index * 8U);
    }
    return word;
}

static void console_write_chunk(const uint8_t *bytes, size_t len)
{
    seL4_Word mr0 = 0;
    seL4_Word mr1 = descriptor(len, SLIME_DESCRIPTOR_FORM_INLINE, 0);
    seL4_Word mr2 = pack_word(bytes, len < SLIME_WORD_BYTES ? len : SLIME_WORD_BYTES);
    seL4_Word mr3 = len <= SLIME_WORD_BYTES
        ? 0
        : pack_word(bytes + SLIME_WORD_BYTES, len - SLIME_WORD_BYTES);
    seL4_MessageInfo_t info = seL4_MessageInfo_new(
        SLIME_CONSOLE_LABEL_WRITE, 0, 0, SLIME_FAST_REGISTERS);
    arm_sys_send(
        seL4_SysSend, SLIME_CONSOLE_SERVICE_SLOT, info.words[0], mr0, mr1, mr2, mr3);
}

void slime_debug_write(const uint8_t *bytes, size_t len)
{
    while (len != 0) {
        size_t chunk = len < SLIME_INLINE_BYTES ? len : SLIME_INLINE_BYTES;
        console_write_chunk(bytes, chunk);
        bytes += chunk;
        len -= chunk;
    }
}

void slime_exit(int64_t status)
{
    seL4_Word mr0 = (seL4_Word)status;
    seL4_MessageInfo_t info = seL4_MessageInfo_new(SLIME_LIFECYCLE_EXIT, 0, 0, 1);
    seL4_Word badge = SLIME_ROOT_SERVICE_SLOT;
    seL4_Word reply_info = info.words[0];
    seL4_Word mr1 = 0;
    seL4_Word mr2 = 0;
    seL4_Word mr3 = 0;
    arm_sys_send_recv(
        seL4_SysCall,
        SLIME_ROOT_SERVICE_SLOT,
        &badge,
        info.words[0],
        &reply_info,
        &mr0,
        &mr1,
        &mr2,
        &mr3,
        0);
    for (;;) {
        __asm__ volatile("yield");
    }
}

void slime_component_start(uint32_t startup_arg)
{
    (void)align_up((uintptr_t)_end, SLIME_GRANULE_BYTES);
    slime_component_main(startup_arg);
    slime_exit(0);
}
