/*
 * Copyright 2026, Slime OS contributors
 *
 * SPDX-License-Identifier: BSD-2-Clause
 */

#pragma once

#include <sel4/config.h>

#if !defined(CONFIG_ARM_CORTEX_A73)
#error CONFIG_ARM_CORTEX_A73 is not defined
#endif

/* Cortex-A73 MPCore TRM r1p0, Debug: ID_AA64DFR0_EL1 reports BRPs = 5 and
 * WRPs = 3, i.e. six breakpoints and four watchpoints. */
#define seL4_NumHWBreakpoints           10
#define seL4_NumExclusiveBreakpoints     6
#define seL4_NumExclusiveWatchpoints     4

#ifdef CONFIG_HARDWARE_DEBUG_API

#define seL4_FirstBreakpoint             0
#define seL4_FirstWatchpoint             6

#define seL4_NumDualFunctionMonitors     0
#define seL4_FirstDualFunctionMonitor    (-1)

#endif /* CONFIG_HARDWARE_DEBUG_API */
