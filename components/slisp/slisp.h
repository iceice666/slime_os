#ifndef SLIME_SLISP_H
#define SLIME_SLISP_H

#include <stddef.h>

typedef enum {
    SLISP_OK = 0,
    SLISP_ERR_SYNTAX = 1,
    SLISP_ERR_LIMIT = 2,
    SLISP_ERR_UNBOUND = 3,
    SLISP_ERR_TYPE = 4,
    SLISP_ERR_ARITY = 5,
    SLISP_ERR_DIV_ZERO = 6
} SlispStatus;

SlispStatus slisp_run(const char *source, char *output, size_t output_capacity);
const char *slisp_status_name(SlispStatus status);
void slisp_session_reset(void);
SlispStatus slisp_session_run(const char *source, char *output, size_t output_capacity);

#endif
