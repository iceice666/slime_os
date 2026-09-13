#include <slime/component_runtime.h>
#include <slime/pwm_servo.h>
#include <slime/spawn.h>

#include "slisp.h"

#define INPUT_SLOT 1U
#define SPAWN_SERVICE_SLOT 2U
#define PWM_SLOT 3U
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

static void write_number(uint32_t value)
{
    char digits[10];
    size_t index = sizeof(digits);
    if (value == 0) {
        digits[--index] = '0';
    }
    while (value != 0) {
        digits[--index] = (char)('0' + value % 10U);
        value /= 10U;
    }
    write_bytes((const uint8_t *)&digits[index], sizeof(digits) - index);
}

static const char *pwm_status_name(int32_t status)
{
    switch (status) {
    case SLIME_PWM_SERVO_STATUS_OK:
        return "ok";
    case SLIME_PWM_SERVO_STATUS_BAD_CHANNEL:
        return "bad-channel";
    case SLIME_PWM_SERVO_STATUS_BAD_PERIOD:
        return "bad-period";
    case SLIME_PWM_SERVO_STATUS_BAD_PULSE:
        return "bad-pulse";
    case SLIME_PWM_SERVO_STATUS_NO_DEVICE:
        return "no-device";
    case SLIME_PWM_SERVO_STATUS_DEVICE_ERROR:
        return "device-error";
    case SLIME_PWM_SERVO_STATUS_MALFORMED:
        return "malformed";
    default:
        return "unknown";
    }
}

/* One call to the pwm driver on slot 3. The reply's status is the driver's
 * word; a transport failure is reported apart from it. */
static int pwm_command(const SlispEffect *effect, SlimePwmServoReply *reply)
{
    uint8_t encoded[SLIME_PWM_SERVO_REQUEST_LEN];
    uint8_t response[SLIME_PWM_SERVO_REPLY_LEN];
    SlimePwmServoRequest request = { 0 };
    int64_t received;
    request.channel = effect->channel;
    request.period_us = effect->period_us;
    request.pulse_us = effect->pulse_us;
    request.flags = 0;
    slime_pwm_servo_request_encode(&request, encoded);
    received = slime_endpoint_exchange(
        PWM_SLOT,
        encoded,
        sizeof(encoded),
        response,
        sizeof(response));
    return received == SLIME_PWM_SERVO_REPLY_LEN
        && slime_pwm_servo_reply_decode(response, (size_t)received, reply);
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
    } else if (status == SLISP_OK && effect.kind == SLISP_EFFECT_PWM) {
        SlimePwmServoReply reply;
        if (!pwm_command(&effect, &reply)) {
            write_text("! pwm transport\n");
        } else if (reply.status == SLIME_PWM_SERVO_STATUS_OK) {
            write_text("=> pwm ");
            write_number(effect.channel);
            write_text(" ");
            write_number(effect.pulse_us);
            write_text("\n");
        } else {
            write_text("! pwm ");
            write_text(pwm_status_name(reply.status));
            write_text("\n");
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
#if defined(CONFIG_ARCH_RISCV64)
            __asm__ volatile("nop");
#else
            __asm__ volatile("yield");
#endif
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
