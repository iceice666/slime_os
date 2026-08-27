#include <stdio.h>
#include <string.h>

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
    return 0;
}
