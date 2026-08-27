#include <slime/component_runtime.h>
#include <slime/spawn.h>

#include "slisp.h"

#define INPUT_SLOT 1U
#define SPAWN_SERVICE_SLOT 2U
#define LINE_BYTES 128U

static size_t text_len(const char *text)
{
    size_t len = 0;
    while (text[len] != '\0') {
        ++len;
    }
    return len;
}

static void write_bytes(const uint8_t *bytes, size_t len)
{
    slime_debug_write(bytes, len);
}

static void write_text(const char *text)
{
    write_bytes((const uint8_t *)text, text_len(text));
}

static int spawn_command(const char *command)
{
    uint8_t encoded[SLIME_SPAWN_REQUEST_LEN];
    uint8_t response[SLIME_SPAWN_REPLY_LEN];
    SlimeSpawnRequest request = { 0 };
    SlimeSpawnReply reply;
    size_t length = text_len(command);
    int64_t received;
    if (length == 0 || length > sizeof(request.command)) {
        return 0;
    }
    request.flags = SLIME_SPAWN_REQUEST_FLAG_DETACHED;
    request.command_len = (uint16_t)length;
    for (size_t index = 0; index < length; ++index) {
        request.command[index] = (uint8_t)command[index];
    }
    slime_spawn_request_encode(&request, encoded);
    received = slime_endpoint_exchange(
        SPAWN_SERVICE_SLOT,
        encoded,
        sizeof(encoded),
        response,
        sizeof(response));
    return received == SLIME_SPAWN_REPLY_LEN
        && slime_spawn_reply_decode(response, (size_t)received, &reply)
        && reply.status == 0;
}

static void evaluate_line(char *line)
{
    char output[128];
    SlispEffect effect;
    SlispStatus status = slisp_session_prepare(line, &effect, output, sizeof(output));
    if (status == SLISP_OK && effect.kind == SLISP_EFFECT_SPAWN) {
        if (spawn_command(effect.command)) {
            write_text("=> spawned ");
            write_text(effect.command);
            write_text("\n");
        } else {
            write_text("! spawn\n");
        }
    } else if (status == SLISP_OK) {
        write_text("=> ");
        write_text(output);
        write_text("\n");
    } else {
        write_text("! ");
        write_text(slisp_status_name(status));
        write_text("\n");
    }
}

void slime_component_main(uint32_t startup_arg)
{
    char line[LINE_BYTES];
    size_t used = 0;
    (void)startup_arg;
    int resident_wait_observed = 0;
    slisp_session_reset();
    write_text("Slisp\nslisp> ");
    for (;;) {
        SlimeInputEvent event;
        int64_t status = slime_input_read(INPUT_SLOT, &event);
        if (status == SLIME_ERR_WOULDBLOCK) {
            if (!resident_wait_observed) {
                write_text("\n[slisp] resident input wait\nslisp> ");
                resident_wait_observed = 1;
            }
            __asm__ volatile("yield");
            continue;
        }
        if (status != SLIME_ERR_SUCCESS) {
            write_text("! input\n");
            slime_exit(1);
        }
        if (!event.pressed) {
            continue;
        }
        if (event.code == 1U) {
            write_text("\n[slisp] repl done\n");
            return;
        }
        if (event.code == 2U) {
            if (used != 0) {
                --used;
                write_text("\b \b");
            }
            continue;
        }
        if (event.code == 4U) {
            line[used] = '\0';
            write_text("\n");
            if (used != 0) {
                evaluate_line(line);
            }
            used = 0;
            write_text("slisp> ");
            continue;
        }
        if (event.code == 9U) {
            if (used + 1 < sizeof(line)) {
                line[used++] = ' ';
                write_text(" ");
            }
            continue;
        }
        if ((event.code & 0x100U) != 0 && used + 1 < sizeof(line)) {
            char character = (char)(event.code & 0xffU);
            line[used++] = character;
            write_bytes((const uint8_t *)&character, 1U);
        }
    }
}
