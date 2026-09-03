#ifndef SLIME_COMPONENT_RUNTIME_H
#define SLIME_COMPONENT_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#include <sel4/types.h>
#include <sel4/arch/syscalls.h>

#include <slime/component_runtime_abi.h>
#include <slime/syscall_abi.h>

#ifdef __cplusplus
extern "C" {
#endif
typedef struct {
    uint32_t code;
    uint8_t pressed;
} SlimeInputEvent;

/* The instruction to execute inside a terminal spin.
 *
 * A hint, not a mechanism: nothing depends on it beyond not burning the core
 * while the root task observes the component's state and tears it down.
 * Selected explicitly per architecture rather than defaulting to one, because
 * a mnemonic that does not exist on the target is an assembler error rather
 * than a portable no-op — which is exactly how the previous `#else` reached
 * an x86-64 build with AArch64's `yield`.
 */
#if defined(CONFIG_ARCH_RISCV64)
#define SLIME_SPIN_HINT "nop"
#elif defined(CONFIG_ARCH_X86_64)
#define SLIME_SPIN_HINT "pause"
#elif defined(CONFIG_ARCH_AARCH64)
#define SLIME_SPIN_HINT "yield"
#else
#error "no spin hint selected for this architecture"
#endif

/* Implemented by each component. Returning is a clean lifecycle exit. */
void slime_component_main(uint32_t startup_arg);

void slime_debug_write(const uint8_t *bytes, size_t len);
int64_t slime_input_read(uint32_t slot, SlimeInputEvent *event);
int64_t slime_endpoint_exchange(
    uint32_t slot,
    const uint8_t *request,
    size_t request_len,
    uint8_t *reply,
    size_t reply_capacity);
void slime_exit(int64_t status) __attribute__((noreturn));

#ifdef __cplusplus
}
#endif

#endif
