#include <slime/component_runtime.h>

#include "slisp.h"

#define INPUT_SLOT 1U
#define LINE_BYTES 128U

static size_t text_len(const char *text)
{
    size_t len = 0;
    while (text[len] != '\0') {
        ++len;
    }
    return len;
}

static void write_text(const char *text)
{
    slime_debug_write((const uint8_t *)text, text_len(text));
}

static void evaluate_line(char *line)
{
    char output[128];
    SlispStatus status = slisp_session_run(line, output, sizeof(output));
    if (status == SLISP_OK) {
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
    int resident_wait_reported = 0;
    slisp_session_reset();
    write_text("Slisp\nslisp> ");
    for (;;) {
        SlimeInputEvent event;
        int64_t status = slime_input_read(INPUT_SLOT, &event);
        if (status == SLIME_ERR_WOULDBLOCK) {
            if (!resident_wait_reported) {
                write_text("[slisp] resident input wait\n");
                resident_wait_reported = 1;
            }
            __asm__ volatile("yield");
            continue;
        }
        if (status != SLIME_ERR_SUCCESS) {
            write_text("! input\n");
            slime_exit(1);
        }
        resident_wait_reported = 0;
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
            }
            continue;
        }
        if ((event.code & 0x100U) != 0 && used + 1 < sizeof(line)) {
            line[used++] = (char)(event.code & 0xffU);
        }
    }
}
