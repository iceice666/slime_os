# Temporary QEMU keyboard input reaches the resident Slisp shell

| Field | Value |
|---|---|
| Date | 2026-08-27 |
| Kind | Change |
| Status | Verified |
| Scope | QEMU product build selection, root device mapping, console input dispatch, Slisp line editing, product composition documentation |
| Roadmap | P5.2, H3 |
| Gates | `just sel4_component_graph_check`, `just slisp_core_check` |
| Trigger | The resident Slisp prompt had an input capability but the product source always returned `WouldBlock`, so `just run` could not accept typed commands |
| Baseline | Deterministic input planes used root-owned scripts; product generation 1 had an intentionally empty input source |

## Summary

The QEMU product now accepts terminal keystrokes in the resident Slisp shell. The QEMU-only build maps the `virt` machine's PL011 receive page through BootInfo device authority before the higher-address virtio scan, polls one byte for each existing `InputRead`, normalizes carriage return and Delete, and keeps an empty FIFO as `WouldBlock`. Slisp echoes accepted characters, erases Backspace visibly, and emits its resident-wait diagnostic only once on a separate line instead of between keystrokes. This is explicitly temporary root mechanism for QEMU; it does not claim the transport-independent seat service or physical HID work owned by H3/H4.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| QEMU build selection | `scripts/build/build-sel4.py` enables `slime_qemu_keyboard` only for the QEMU component-graph product variant | Physical targets and deterministic plane images do not compile or consume a QEMU MMIO address |
| Root device mechanism | Added a polling PL011 RX view and reserved `0x0900_0000` before the existing `0x0a00_0000` virtio scan | Device-untyped retype remains monotonic while both UART and virtio pages stay mapped through declared BootInfo authority |
| Input dispatch | Empty scripts may carry a live UART source; script-backed planes retain per-task deterministic cursors and precedence | The existing input capability and event encoding remain the only shell-facing ABI |
| Slisp REPL | Echoes printable characters and spaces, renders Backspace as erase, and reports the first empty input wait once before redrawing a clean prompt | A serial user can see and edit the command without debug markers splitting it |
| Product description | Records the QEMU-only temporary input path and its `WouldBlock` behavior | Documentation no longer claims generation 1 always has an empty source |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| QEMU keyboard mapping or quiet-prompt behavior regresses | `just sel4_component_graph_check` | missing healthy graph, typed `(+ 1 1)` / `=> 2`, prompt, or single resident input-wait marker; root fatal during device mapping; marker embedded in typed input |
| Slisp input/event semantics regress | `just slisp_core_check` | missing evaluator results, refusal, or clean termination in the scripted input plane |
| Rust changes violate repository build conventions | `just fmt_check_all`, `just lint_all` | formatter or clippy failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Interactive `qemu-system-aarch64 ... -kernel build/slime-sel4-graph.elf`; typed `(+ 20 22)` | Serial showed `SLIME_ROOT QEMU keyboard ready uart=0x9000000`, echoed the expression, returned `=> 42`, and displayed the next `slisp>` prompt | Direct |
| Same interactive boot; typed `(+ 20 23`, Delete, `2)` and Enter | Delete was normalized to Backspace, the visible line became `(+ 20 23 2)`, and Slisp returned `=> 42` | Direct |
| `just sel4_component_graph_check` | Pass after the UART mapping and quiet-prompt cutover; after startup drained, the gate sent `(+ 1 1)` one byte at a time with an empty-FIFO interval, observed one separate resident-wait marker followed by the uninterrupted expression and `=> 2`, and generation 1 remained healthy | Direct |
| `just slisp_core_check` | Pass; deterministic scripted input and evaluator behavior remained intact | Direct |
| `just test_sel4_root` | Pass, 184/184 tests across 19 modules | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just lint_all` | Pass | Direct |

## Decisions

- **Decision:** reuse QEMU `virt`'s PL011 serial RX instead of introducing virtio-input or a new protocol. **Rationale:** `just run` already owns an interactive serial terminal, and Slisp already consumes a typed input capability; polling connects those two existing boundaries with no serialized-format change. **Rejected alternative:** add virtio-keyboard and a new userspace driver stack for a temporary development aid.
- **Decision:** map PL011 before probing virtio. **Rationale:** the object allocator advances monotonically within a device untyped, and PL011's physical page precedes the virtio window. **Rejected alternative:** weaken `DeviceFramePassed` or remap after the scan; both would hide an allocator invariant violation.
- **Decision:** keep this path QEMU-product-only. **Rationale:** a literal QEMU address in physical images would be false platform support and would bypass H3's transport-independent input service. **Rejected alternative:** select the address at runtime from the target-profile name without a device-tree driver.

## Open risks and follow-ups

- [ ] H3 still owns the real seat/input service, USB HID transport, press/release state, modifiers, focus, hotplug, and a userspace driver boundary. This temporary path supplies bytes, not those semantics.
- [ ] The product input wait is polling through repeated Slisp `InputRead` calls; an interrupt-backed queue should replace it when the portable input service lands.
- [ ] Interactive QEMU typing is an observed scenario, not yet an automated stdin-driving gate; `sel4_component_graph_check` continues to guard startup and blocked-idle behavior.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: command output captured in the interactive QEMU session and named repository gates
- Serial/debugger/model output: `SLIME_ROOT QEMU keyboard ready uart=0x9000000`; `(+ 20 22)`; `=> 42`; Backspace-edited `(+ 20 23 2)`; `=> 42`
- Related roadmap item: [`roadmap/04-platform-hardware.md#H3`](../../roadmap/04-platform-hardware.md), [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md)
