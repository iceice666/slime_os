#ifndef SLIME_COMPONENT_RUNTIME_H
#define SLIME_COMPONENT_RUNTIME_H

#include <stddef.h>
#include <stdint.h>

#include <sel4/types.h>
#include <sel4/sel4_arch/syscalls.h>

#include <slime/component_runtime_abi.h>
#include <slime/syscall_abi.h>

#ifdef __cplusplus
extern "C" {
#endif


/* Implemented by each component. Returning is a clean lifecycle exit. */
void slime_component_main(uint32_t startup_arg);

void slime_debug_write(const uint8_t *bytes, size_t len);
void slime_exit(int64_t status) __attribute__((noreturn));

#ifdef __cplusplus
}
#endif

#endif
