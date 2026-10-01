#include <stdio.h>
#include <string.h>

#include <slime/pwm_servo.h>

#include "slisp.h"

typedef struct {
    const char *name;
    const char *source;
    SlispStatus status;
    const char *output;
} Vector;

static int string_vectors(void)
{
    static const Vector rejected[] = {
        { "string-value", "\"arg\"", SLISP_ERR_TYPE, "" },
        { "quoted-string", "'\"arg\"", SLISP_ERR_TYPE, "" },
        { "nested-string", "'(a (\"arg\"))", SLISP_ERR_TYPE, "" },
        { "explicit-quoted-string", "(quote (\"arg\"))", SLISP_ERR_TYPE, "" },
        { "string-define", "(define x '\"arg\")", SLISP_ERR_TYPE, "" },
        { "string-closure", "(fn () \"arg\")", SLISP_ERR_TYPE, "" },
        { "string-dead-branch", "(if true 1 \"arg\")", SLISP_ERR_TYPE, "" },
        { "string-integer-op", "(+ 1 \"arg\")", SLISP_ERR_TYPE, "" },
        { "string-function", "(\"arg\" 1)", SLISP_ERR_TYPE, "" },
        { "string-nested-spawn", "(do (spawn 'echo \"arg\"))", SLISP_ERR_TYPE, "" },
        { "unterminated-string", "\"arg", SLISP_ERR_SYNTAX, "" },
        { "unterminated-spawn", "(spawn 'echo \"arg)", SLISP_ERR_SYNTAX, "" },
        { "quote-missing", "'", SLISP_ERR_SYNTAX, "" },
    };
    static const Vector spawn_rejected[] = {
        { "spawn-string-command", "(spawn \"echo\")", SLISP_ERR_TYPE, "" },
        { "spawn-quoted-string-command", "(spawn '\"echo\")", SLISP_ERR_TYPE, "" },
        { "spawn-evaluated-string", "(spawn 'echo (quote \"arg\"))", SLISP_ERR_TYPE, "" },
        { "spawn-nested-string", "(spawn 'echo (\"arg\"))", SLISP_ERR_TYPE, "" },
        { "spawn-symbol-argument", "(spawn 'echo arg)", SLISP_ERR_TYPE, "" },
        { "spawn-five-arguments", "(spawn 'echo \"\" \"\" \"\" \"\" \"\")", SLISP_ERR_LIMIT, "" },
        { "spawn-late-type", "(spawn 'echo \"arg\" 1)", SLISP_ERR_TYPE, "" },
    };
    char output[128];
    SlispEffect effect;
    for (size_t index = 0; index < sizeof(rejected) / sizeof(rejected[0]); ++index) {
        slisp_session_reset();
        memset(&effect, 0xff, sizeof(effect));
        if (slisp_run(rejected[index].source, output, sizeof(output)) != rejected[index].status
            || slisp_session_run(rejected[index].source, output, sizeof(output))
                != rejected[index].status
            || slisp_session_prepare(rejected[index].source, &effect, output, sizeof(output))
                != rejected[index].status
            || effect.kind != SLISP_EFFECT_NONE || effect.argument_count != 0
            || effect.argument_bytes != 0 || output[0] != '\0') {
            fprintf(stderr, "%s: string refusal failed\n", rejected[index].name);
            return 0;
        }
    }
    for (size_t index = 0; index < sizeof(spawn_rejected) / sizeof(spawn_rejected[0]); ++index) {
        slisp_session_reset();
        memset(&effect, 0xff, sizeof(effect));
        if (slisp_session_prepare(spawn_rejected[index].source, &effect, output, sizeof(output))
                != spawn_rejected[index].status
            || effect.kind != SLISP_EFFECT_NONE || effect.command[0] != '\0'
            || effect.argument_count != 0 || effect.argument_bytes != 0 || output[0] != '\0') {
            fprintf(stderr, "%s: spawn refusal failed\n", spawn_rejected[index].name);
            return 0;
        }
        for (size_t byte = 0; byte < sizeof(effect.arguments); ++byte) {
            if (effect.arguments[byte] != 0) {
                fputs("failed spawn retained argument bytes\n", stderr);
                return 0;
            }
        }
    }
    slisp_session_reset();
    if (slisp_run("'(a (b) 3)", output, sizeof(output)) != SLISP_OK
        || strcmp(output, "(a (b) 3)") != 0
        || slisp_session_prepare("(spawn 'echo)", &effect, output, sizeof(output)) != SLISP_OK
        || effect.kind != SLISP_EFFECT_SPAWN || strcmp(effect.command, "echo") != 0
        || effect.argument_count != 0 || effect.argument_bytes != 0
        || slisp_session_prepare(
               "(spawn 'echo \"http://192.0.2.1/path?q=(x)\" \"two words\" \"\" \"\\raw\")",
               &effect, output, sizeof(output)) != SLISP_OK
        || effect.kind != SLISP_EFFECT_SPAWN || effect.argument_count != 4
        || effect.argument_lengths[0] != 27 || effect.argument_lengths[1] != 9
        || effect.argument_lengths[2] != 0 || effect.argument_lengths[3] != 4
        || effect.argument_bytes != 40
        || memcmp(effect.arguments, "http://192.0.2.1/path?q=(x)two words\\raw", 40) != 0) {
        fputs("literal spawn arguments or quote shorthand failed\n", stderr);
        return 0;
    }
    for (size_t length = 256; length <= 257; ++length) {
        char source[300];
        size_t prefix = strlen("(spawn 'echo \"");
        strcpy(source, "(spawn 'echo \"");
        memset(source + prefix, 'x', length);
        strcpy(source + prefix + length, "\")");
        slisp_session_reset();
        if (slisp_session_prepare(source, &effect, output, sizeof(output))
                != (length == 256 ? SLISP_OK : SLISP_ERR_LIMIT)
            || (length == 256 && (effect.kind != SLISP_EFFECT_SPAWN
                || effect.argument_count != 1 || effect.argument_bytes != 256
                || effect.argument_lengths[0] != 256
                || memcmp(effect.arguments, source + prefix, 256) != 0))
            || (length == 257 && (effect.kind != SLISP_EFFECT_NONE
                || effect.argument_count != 0 || effect.argument_bytes != 0))) {
            fputs("literal byte bound failed\n", stderr);
            return 0;
        }
    }
    for (size_t extra = 0; extra <= 1; ++extra) {
        char source[320];
        size_t used = strlen("(spawn 'echo \"");
        strcpy(source, "(spawn 'echo \"");
        memset(source + used, 'a', 128);
        used += 128;
        strcpy(source + used, "\" \"");
        used += 3;
        memset(source + used, 'b', 128 + extra);
        used += 128 + extra;
        strcpy(source + used, "\")");
        slisp_session_reset();
        if (slisp_session_prepare(source, &effect, output, sizeof(output))
                != (extra == 0 ? SLISP_OK : SLISP_ERR_LIMIT)
            || (extra == 0 && (effect.argument_count != 2 || effect.argument_bytes != 256
                || effect.argument_lengths[0] != 128 || effect.argument_lengths[1] != 128
                || effect.arguments[127] != 'a' || effect.arguments[128] != 'b'))
            || (extra == 1 && effect.kind != SLISP_EFFECT_NONE)) {
            fputs("aggregate literal byte bound failed\n", stderr);
            return 0;
        }
    }
    slisp_session_reset();
    {
        char source[64] = "(define x '(a (\"arg\")))";
        if (slisp_session_run(source, output, sizeof(output)) != SLISP_ERR_TYPE) {
            fputs("string escaped into a binding\n", stderr);
            return 0;
        }
        strcpy(source, "x");
        if (slisp_session_run(source, output, sizeof(output)) != SLISP_ERR_UNBOUND) {
            fputs("reused source retained a string binding\n", stderr);
            return 0;
        }
    }
    return 1;
}

int main(void)
{
    static const Vector vectors[] = {
        { "integer", "42", SLISP_OK, "42" },
        { "reader", "(quote (a (b) 3))", SLISP_OK, "(a (b) 3)" },
        { "closure", "(let ((x 40)) ((fn (y) (+ x y)) 2))", SLISP_OK, "42" },
        { "nested-closure", "(((fn (x) (fn (y) (+ x y))) 20) 22)", SLISP_OK, "42" },
        { "parallel-let", "(let ((x 1)) (let ((x 2) (y x)) (+ x y)))", SLISP_OK, "3" },
        { "false-branch", "(if false 7 9)", SLISP_OK, "9" },
        { "nil-truth", "(if nil 7 9)", SLISP_OK, "7" },
        { "sequence", "(do (+ 1 2) (* 6 7))", SLISP_OK, "42" },
        { "syntax-open", "(+ 1 2", SLISP_ERR_SYNTAX, "" },
        { "syntax-close", ")", SLISP_ERR_SYNTAX, "" },
        { "trailing", "1 2", SLISP_ERR_SYNTAX, "" },
        { "unbound", "missing", SLISP_ERR_UNBOUND, "" },
        { "type", "(+ true 1)", SLISP_ERR_TYPE, "" },
        { "arity", "(+ 1)", SLISP_ERR_ARITY, "" },
        { "divide", "(/ 1 0)", SLISP_ERR_DIV_ZERO, "" },
    };
    size_t index;
    for (index = 0; index < sizeof(vectors) / sizeof(vectors[0]); ++index) {
        char output[128];
        SlispStatus status = slisp_run(vectors[index].source, output, sizeof(output));
        if (status != vectors[index].status || strcmp(output, vectors[index].output) != 0) {
            fprintf(
                stderr,
                "%s: got status=%s output=%s\n",
                vectors[index].name,
                slisp_status_name(status),
                output);
            return 1;
        }
    }
    puts("Slisp core: 15 behavior vectors passed");
    slisp_session_reset();
    {
        char output[128];
        if (slisp_session_run("(define answer 40)", output, sizeof(output)) != SLISP_OK
            || strcmp(output, "40") != 0
            || slisp_session_run("(+ answer 2)", output, sizeof(output)) != SLISP_OK
            || strcmp(output, "42") != 0) {
            fputs("persistent define failed\n", stderr);
            return 1;
        }
    }
    puts("Slisp session: persistent define passed");
    {
        char output[128];
        SlispEffect effect;
        if (slisp_session_prepare("sysinfo", &effect, output, sizeof(output)) != SLISP_OK
            || effect.kind != SLISP_EFFECT_SPAWN
            || strcmp(effect.command, "sysinfo") != 0
            || slisp_session_prepare(
                   "(spawn (quote echo))", &effect, output, sizeof(output))
                != SLISP_OK
            || effect.kind != SLISP_EFFECT_SPAWN
            || strcmp(effect.command, "echo") != 0
            || slisp_session_prepare("(spawn sysinfo)", &effect, output, sizeof(output))
                != SLISP_ERR_TYPE
            || slisp_session_prepare("(+ answer 2)", &effect, output, sizeof(output)) != SLISP_OK
            || effect.kind != SLISP_EFFECT_NONE
            || strcmp(output, "42") != 0) {
            fputs("effect selection failed\n", stderr);
            return 1;
        }
    }
    puts("Slisp effects: spawn selection passed");
    {
        char output[128];
        SlispEffect effect;
        /* `(pwm 0 1600)` as `components/proto/tests/pwm_servo.rs` pins it. */
        static const uint8_t pinned[SLIME_PWM_SERVO_REQUEST_LEN] = {
            'P', 'W', 'M', 'S', 1, 0, 0, 0, 0, 0, 0, 0, 0x20, 0x4e, 0, 0,
            0x40, 0x06, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        };
        uint8_t encoded[SLIME_PWM_SERVO_REQUEST_LEN];
        SlimePwmServoRequest request = { 0 };
        if (slisp_session_prepare("(pwm 0 1600)", &effect, output, sizeof(output)) != SLISP_OK
            || effect.kind != SLISP_EFFECT_PWM || effect.channel != 0 || effect.pulse_us != 1600
            || effect.period_us != 20000
            || slisp_session_prepare("(pwm 1 1500 4000)", &effect, output, sizeof(output))
                != SLISP_OK
            || effect.kind != SLISP_EFFECT_PWM || effect.channel != 1 || effect.pulse_us != 1500
            || effect.period_us != 4000
            || slisp_session_prepare("(pwm 0)", &effect, output, sizeof(output)) != SLISP_ERR_ARITY
            || slisp_session_prepare("(pwm 0 1 2 3)", &effect, output, sizeof(output))
                != SLISP_ERR_ARITY
            || slisp_session_prepare("(pwm x 1600)", &effect, output, sizeof(output))
                != SLISP_ERR_TYPE
            || slisp_session_prepare("(pwm 0 -1)", &effect, output, sizeof(output))
                != SLISP_ERR_TYPE
            || slisp_session_prepare("(+ 1 2)", &effect, output, sizeof(output)) != SLISP_OK
            || effect.kind != SLISP_EFFECT_NONE) {
            fputs("pwm effect selection failed\n", stderr);
            return 1;
        }
        request.channel = 0;
        request.period_us = 20000;
        request.pulse_us = 1600;
        slime_pwm_servo_request_encode(&request, encoded);
        if (memcmp(encoded, pinned, sizeof(pinned)) != 0) {
            fputs("pwm request encoding drifted from the pinned vector\n", stderr);
            return 1;
        }
    }
    puts("Slisp effects: pwm selection and the pinned request vector passed");
    if (!string_vectors()) {
        return 1;
    }
    return 0;
}
