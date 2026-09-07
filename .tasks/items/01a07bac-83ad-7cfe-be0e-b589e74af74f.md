---
schema: work-item/v1
id: 01a07bac-83ad-7cfe-be0e-b589e74af74f
key: P6.PWM
kind: epic
state: open
created: 2026-09-07T11:41:38+00:00
tags:
  - architecture
  - hardware
---

# Servo PWM to a drone ESC on the H1V1

## Context

The H1V1 boots seL4, `slime-root`, and a resident Slisp shell over UART0 (P6.A-P6.C), but
nothing in the tree drives a pad. This lane makes `(pwm 0 1600)` at that prompt spin a drone
motor fast, `(pwm 0 1200)` slow, and `(pwm 0 0)` stop it, through the SoC's own PWM block.

It is deliberately cut into a board-specific bench probe, a board-neutral mechanism, and a
board-specific gate, so the general half carries no NT98690 detail.

## Exit conditions

Every child closes on its own observed evidence. This item closes when a servo PWM signal
programmed from the resident shell is observed driving an ESC on the named board.
