#include <slime/component_runtime.h>

void *memset(void *destination, int value, size_t len)
{
    uint8_t *bytes = destination;
    for (size_t index = 0; index < len; ++index) {
        bytes[index] = (uint8_t)value;
    }
    return destination;
}

void __assert_fail(
    const char *expression, const char *file, int line, const char *function)
{
    (void)expression;
    (void)file;
    (void)line;
    (void)function;
    for (;;) {
        __asm__ volatile(SLIME_SPIN_HINT);
    }
}

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
    seL4_SendWithMRs(SLIME_CONSOLE_SERVICE_SLOT, info, &mr0, &mr1, &mr2, &mr3);
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

static void copy_bytes(uint8_t *destination, const uint8_t *source, size_t len)
{
    size_t index;
    for (index = 0; index < len; ++index) {
        destination[index] = source[index];
    }
}

/* The answer a receive or a call left in the IPC buffer. Its label is the
 * byte length, which must fit both the words the kernel delivered and the
 * caller's buffer, and it carries no capability. A reply shorter than four
 * words leaves request words in the registers after it, which the length
 * bound keeps out of the copy. */
static int64_t collect_reply(
    const seL4_IPCBuffer *ipc_buffer,
    seL4_MessageInfo_t answer,
    uint8_t *reply,
    size_t reply_capacity)
{
    size_t reply_len = seL4_MessageInfo_get_label(answer);
    if (seL4_MessageInfo_get_extraCaps(answer) != 0
        || reply_len > reply_capacity
        || reply_len > seL4_MessageInfo_get_length(answer) * SLIME_WORD_BYTES) {
        return SLIME_ERR_INVALID_ARG;
    }
    copy_bytes(reply, (const uint8_t *)ipc_buffer->msg, reply_len);
    return (int64_t)reply_len;
}

int64_t slime_endpoint_exchange(
    uint32_t slot,
    const uint8_t *request,
    size_t request_len,
    uint8_t *reply,
    size_t reply_capacity)
{
    seL4_IPCBuffer *ipc_buffer = (seL4_IPCBuffer *)align_up((uintptr_t)_end, SLIME_GRANULE_BYTES);
    size_t request_words;
    seL4_Word badge = 0;
    seL4_Word mr0 = 0;
    seL4_Word mr1 = 0;
    seL4_Word mr2 = 0;
    seL4_Word mr3 = 0;
    seL4_MessageInfo_t info;
    seL4_MessageInfo_t answer;
    if (slot >= SLIME_NATIVE_REGION_SLOTS || request == NULL || reply == NULL
        || request_len > SLIME_MAX_MSG || reply_capacity > SLIME_MAX_MSG) {
        return SLIME_ERR_INVALID_ARG;
    }
    request_words = (request_len + SLIME_WORD_BYTES - 1U) / SLIME_WORD_BYTES;
    for (size_t index = 0; index < request_words; ++index) {
        ipc_buffer->msg[index] = 0;
    }
    copy_bytes((uint8_t *)ipc_buffer->msg, request, request_len);
    info = seL4_MessageInfo_new(request_len, 0, 0, request_words);
    seL4_SendWithMRs(
        SLIME_NATIVE_ENDPOINT_BASE + slot,
        info,
        &ipc_buffer->msg[0],
        &ipc_buffer->msg[1],
        &ipc_buffer->msg[2],
        &ipc_buffer->msg[3]);
    answer = seL4_RecvWithMRs(
        SLIME_NATIVE_ENDPOINT_BASE + slot,
        &badge,
        &mr0,
        &mr1,
        &mr2,
        &mr3);
    ipc_buffer->msg[0] = mr0;
    ipc_buffer->msg[1] = mr1;
    ipc_buffer->msg[2] = mr2;
    ipc_buffer->msg[3] = mr3;
    return collect_reply(ipc_buffer, answer, reply, reply_capacity);
}

int64_t slime_endpoint_call(
    uint32_t slot,
    const uint8_t *request,
    size_t request_len,
    uint8_t *reply,
    size_t reply_capacity)
{
    seL4_IPCBuffer *ipc_buffer = (seL4_IPCBuffer *)align_up((uintptr_t)_end, SLIME_GRANULE_BYTES);
    size_t request_words;
    seL4_MessageInfo_t answer;
    if (slot >= SLIME_NATIVE_REGION_SLOTS || request == NULL || reply == NULL
        || request_len > SLIME_MAX_MSG || reply_capacity > SLIME_MAX_MSG) {
        return SLIME_ERR_INVALID_ARG;
    }
    request_words = (request_len + SLIME_WORD_BYTES - 1U) / SLIME_WORD_BYTES;
    for (size_t index = 0; index < request_words; ++index) {
        ipc_buffer->msg[index] = 0;
    }
    copy_bytes((uint8_t *)ipc_buffer->msg, request, request_len);
    /* The first four words travel in registers both ways: libsel4 reads the
     * request's from msg[0..3] and writes the reply's back there, beside the
     * words the kernel places at msg[4..]. */
    answer = seL4_CallWithMRs(
        SLIME_NATIVE_ENDPOINT_BASE + slot,
        seL4_MessageInfo_new(request_len, 0, 0, request_words),
        &ipc_buffer->msg[0],
        &ipc_buffer->msg[1],
        &ipc_buffer->msg[2],
        &ipc_buffer->msg[3]);
    return collect_reply(ipc_buffer, answer, reply, reply_capacity);
}

int64_t slime_input_read(uint32_t slot, SlimeInputEvent *event)
{
    seL4_Word mr0 = slot;
    seL4_Word mr1 = 0;
    seL4_Word mr2 = 0;
    seL4_Word mr3 = 0;
    seL4_MessageInfo_t info = seL4_MessageInfo_new(SLIME_CONSOLE_LABEL_INPUT_READ, 0, 0, 1);
    seL4_MessageInfo_t answer = seL4_CallWithMRs(
        SLIME_CONSOLE_SERVICE_SLOT, info, &mr0, &mr1, &mr2, &mr3);
    (void)answer;
    if ((int64_t)mr0 == SLIME_ERR_WOULDBLOCK) {
        return SLIME_ERR_WOULDBLOCK;
    }
    if ((int64_t)mr0 < 0) {
        return (int64_t)mr0;
    }
    if (event == NULL) {
        return SLIME_ERR_INVALID_ARG;
    }
    event->code = (uint32_t)mr1;
    event->pressed = (uint8_t)(mr1 >> 32 != 0);
    return SLIME_ERR_SUCCESS;
}

void slime_exit(int64_t status)
{
    seL4_Word mr0 = (seL4_Word)status;
    seL4_MessageInfo_t info = seL4_MessageInfo_new(SLIME_LIFECYCLE_EXIT, 0, 0, 1);
    seL4_Word mr1 = 0;
    seL4_Word mr2 = 0;
    seL4_Word mr3 = 0;
    seL4_MessageInfo_t answer = seL4_CallWithMRs(
        SLIME_ROOT_SERVICE_SLOT, info, &mr0, &mr1, &mr2, &mr3);
    (void)answer;
    for (;;) {
        __asm__ volatile(SLIME_SPIN_HINT);
    }
}

void slime_component_start(uint32_t startup_arg)
{
    slime_component_main(startup_arg);
    slime_exit(0);
}
