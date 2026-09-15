#ifndef SLIME_SLISP_H
#define SLIME_SLISP_H

#include <stddef.h>
#include <stdint.h>

typedef enum {
    SLISP_OK = 0,
    SLISP_ERR_SYNTAX = 1,
    SLISP_ERR_LIMIT = 2,
    SLISP_ERR_UNBOUND = 3,
    SLISP_ERR_TYPE = 4,
    SLISP_ERR_ARITY = 5,
    SLISP_ERR_DIV_ZERO = 6
} SlispStatus;
typedef enum {
    SLISP_EFFECT_NONE = 0,
    SLISP_EFFECT_SPAWN = 1,
    SLISP_EFFECT_PWM = 2
} SlispEffectKind;

/* `(pwm channel pulse_us)` or `(pwm channel pulse_us period_us)`: the values
 * are carried as parsed; the driver, not the reader, judges their bounds. */
typedef struct {
    SlispEffectKind kind;
    char command[17];
    uint32_t channel;
    uint32_t pulse_us;
    uint32_t period_us;
} SlispEffect;

SlispStatus slisp_run(const char *source, char *output, size_t output_capacity);
const char *slisp_status_name(SlispStatus status);
void slisp_session_reset(void);
SlispStatus slisp_session_run(const char *source, char *output, size_t output_capacity);
SlispStatus slisp_session_prepare(
    const char *source,
    SlispEffect *effect,
    char *output,
    size_t output_capacity);

#endif
