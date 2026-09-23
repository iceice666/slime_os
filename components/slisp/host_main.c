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
    return 0;
}
