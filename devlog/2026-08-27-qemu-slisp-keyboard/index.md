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

The QEMU product now accepts terminal keystrokes in the resident Slisp shell and routes command symbols through generation-authorized spawn-service dispatch. The QEMU-only build maps the `virt` machine's PL011 receive page through BootInfo device authority before the higher-address virtio scan, polls one byte for each existing `InputRead`, normalizes carriage return and Delete, and keeps an empty FIFO as `WouldBlock`. Slisp echoes accepted characters, erases Backspace visibly, and emits its resident-wait diagnostic only once on a separate line instead of between keystrokes. An unbound top-level symbol such as `sysinfo`, or explicit `(spawn (quote sysinfo))`, now becomes a bounded detached `spawn/v1` request; pure Lisp expressions and persistent bindings retain their evaluator semantics. The generation declares each command's executable and launch-context endpoint, while spawn-service owns and reclaims detached supervision handles. This remains temporary QEMU shell mechanism; it does not claim the transport-independent seat service or physical HID work owned by H3/H4.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| QEMU build selection | `scripts/build/build-sel4.py` enables `slime_qemu_keyboard` only for the QEMU component-graph product variant | Physical targets and deterministic plane images do not compile or consume a QEMU MMIO address |
| Root device mechanism | Added a polling PL011 RX view and reserved `0x0900_0000` before the existing `0x0a00_0000` virtio scan | Device-untyped retype remains monotonic while both UART and virtio pages stay mapped through declared BootInfo authority |
| Input dispatch | Empty scripts may carry a live UART source; script-backed planes retain per-task deterministic cursors and precedence | The existing input capability and event encoding remain the only shell-facing ABI |
| Slisp REPL | Echoes printable characters and spaces, renders Backspace as erase, and reports the first empty input wait once before redrawing a clean prompt | A serial user can see and edit the command without debug markers splitting it |
| Shell command dispatch | Added Slisp effect selection, generated C `spawn/v1` bindings, a C endpoint exchange primitive, and detached spawn-service ownership | Command execution uses the existing bounded protocol and generation authority without treating every Lisp expression as an external command |
| Command launch context | Product and demo compositions declare per-command context endpoints; spawn-service strips its client-only detached flag before delivery | Spawned components receive an ordinary valid launch context on their declared slot 0 |
| Product description | Records the QEMU-only temporary input path and its `WouldBlock` behavior | Documentation no longer claims generation 1 always has an empty source |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| QEMU keyboard mapping or quiet-prompt behavior regresses | `just sel4_component_graph_check` | missing healthy graph, typed `(+ 1 1)` / `=> 2`, prompt, or single resident input-wait marker; root fatal during device mapping; marker embedded in typed input |
| Slisp input/event semantics regress | `just slisp_core_check` | missing evaluator results, refusal, or clean termination in the scripted input plane |
| Shell command authority or launch-context delivery regresses | `just sel4_component_graph_check` | `sysinfo` is refused, exits nonzero, lacks its profile marker, or Slisp fails to regain its prompt |
| Detached spawn validation regresses | `cargo test -p slime-proto --test spawn` | detached request accepts a client-supplied budget or no longer round-trips |
| Rust changes violate repository build conventions | `just fmt_check_all`, `just lint_all` | formatter or clippy failure |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Interactive `qemu-system-aarch64 ... -kernel build/slime-sel4-graph.elf`; typed `(+ 20 22)` | Serial showed `SLIME_ROOT QEMU keyboard ready uart=0x9000000`, echoed the expression, returned `=> 42`, and displayed the next `slisp>` prompt | Direct |
| Same interactive boot; typed `(+ 20 23`, Delete, `2)` and Enter | Delete was normalized to Backspace, the visible line became `(+ 20 23 2)`, and Slisp returned `=> 42` | Direct |
| `just sel4_component_graph_check` | Pass after the command-dispatch cutover; after startup drained, the gate sent `(+ 1 1)` and `sysinfo` one byte at a time, observed `=> 2`, the sysinfo launch-context/profile markers, `=> spawned sysinfo`, and generation 1 remained healthy | Direct |
| Interactive `qemu-system-aarch64 ... -kernel build/slime-sel4-graph.elf`; typed `sysinfo` | Serial echoed `sysinfo`, spawn-service accepted the request, sysinfo printed `command=sysinfo args=0 env=0 cwd=none stdin=none` and `spawned through profile`, exited 0, Slisp printed `=> spawned sysinfo`, and the next prompt appeared | Direct |
| `just slisp_core_check` | Pass; deterministic scripted input and evaluator behavior remained intact | Direct |
| `just test_sel4_root` | Pass, 184/184 tests across 19 modules | Direct |
| `just fmt_check_all` | Pass | Direct |
| `just lint_all` | Pass | Direct |

## Decisions

- **Decision:** reuse QEMU `virt`'s PL011 serial RX instead of introducing virtio-input or a new protocol. **Rationale:** `just run` already owns an interactive serial terminal, and Slisp already consumes a typed input capability; polling connects those two existing boundaries with no serialized-format change. **Rejected alternative:** add virtio-keyboard and a new userspace driver stack for a temporary development aid.
- **Decision:** map PL011 before probing virtio. **Rationale:** the object allocator advances monotonically within a device untyped, and PL011's physical page precedes the virtio window. **Rejected alternative:** weaken `DeviceFramePassed` or remap after the scan; both would hide an allocator invariant violation.
- **Decision:** keep this path QEMU-product-only. **Rationale:** a literal QEMU address in physical images would be false platform support and would bypass H3's transport-independent input service. **Rejected alternative:** select the address at runtime from the target-profile name without a device-tree driver.
- **Decision:** represent shell launch as a Slisp top-level effect rather than a Lisp primitive that performs IPC inside the evaluator. **Rationale:** the evaluator stays pure and host-testable while the freestanding component owns capability effects. **Rejected alternative:** embed seL4 and spawn-service knowledge in `slisp.c`.
- **Decision:** use endpoint send-then-receive rather than nested `seL4_Call`. **Rationale:** spawn-service performs root calls while handling the request; on the non-MCS kernel those calls can replace the implicit reply capability. **Rejected alternative:** direct reply after nested root RPC, which is not a stable caller-return path.
- **Decision:** make detached supervision service-owned and clear the detached transport flag before child delivery. **Rationale:** Slisp needs launch acceptance, not lifecycle authority, and child launch context must remain the ordinary `spawn/v1` shape. **Rejected alternative:** transfer an unused supervision capability to the C client or teach every child about a client/service transport flag.

## Open risks and follow-ups

- [ ] H3 still owns the real seat/input service, USB HID transport, press/release state, modifiers, focus, hotplug, and a userspace driver boundary. This temporary path supplies bytes, not those semantics.
- [ ] The product input wait is polling through repeated Slisp `InputRead` calls; an interrupt-backed queue should replace it when the portable input service lands.
- [ ] The current shell command syntax supports a bounded command symbol only; arguments, environment entries, working-directory authority, stream capabilities, and completion status presentation remain future shell work.

## Artifacts and provenance

- Focused report: this entry
- Raw transcript: command output captured in the interactive QEMU session and named repository gates
- Serial/debugger/model output: `SLIME_ROOT QEMU keyboard ready uart=0x9000000`; `(+ 20 22)`; `=> 42`; Backspace-edited `(+ 20 23 2)`; `=> 42`; `sysinfo`; `[sysinfo] spawned through profile`; `=> spawned sysinfo`
- Related roadmap item: [`roadmap/04-platform-hardware.md#H3`](../../roadmap/04-platform-hardware.md), [`roadmap/07-architecture-portability.md`](../../roadmap/07-architecture-portability.md)
