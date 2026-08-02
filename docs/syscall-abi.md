# Slime OS syscall ABI

The canonical semantic syscall surface is `kernel/src/syscall/mod.rs`; the
userspace constants and wrappers in `components/runtime/src/syscall.rs` mirror
it. Update this file in the same change that changes a syscall number,
argument, return convention, or architecture-specific calling convention.

## Semantic syscall table

Arguments use the semantic names `a0` through `a4`; each architecture maps
those positions to registers in its calling convention. Pointer arguments name
userspace memory, and capability arguments name slots in the caller's
capability table.

| Number | Constant | argc | Arguments and meaning | Success/error convention |
| --- | --- | --- | --- | --- |
| 0 | `SYS_YIELD` | 0 | Yield the current task to the scheduler. | Returns after the task is scheduled again; `rax` retains syscall number `0`, which is successful completion. |
| 1 | `SYS_SEND` | 5 | `a0=endpoint_slot`, `a1=payload_ptr`, `a2=payload_len`, `a3=cap_slots_ptr`, `a4=cap_count`; send at most `MAX_MSG` bytes and move at most `MAX_CAPS_PER_MSG` transferable capabilities. | `0` on delivery; otherwise a negative error. A failed send leaves attached capabilities with the sender. |
| 2 | `SYS_RECV` | 3 | `a0=endpoint_slot`, `a1=payload_out`, `a2=cap_slots_out`; receive one bounded message and its transferred capability slots. | Nonnegative payload byte count; otherwise a negative error. |
| 3 | `SYS_EXIT` | 1 | `a0=status`; terminate the current task with an exit status. | Does not return. |
| 4 | `SYS_SPAWN` | 3 | `a0=executable_slot`, `a1=grants_ptr`, `a2=grant_count`; start the executable with non-consuming, narrow capability grants. | Primary return is the nonnegative child task id and auxiliary return is the supervision capability slot; otherwise the primary return is a negative error. |
| 5 | `SYS_DEBUG_WRITE` | 2 | `a0=bytes_ptr`, `a1=byte_len`; write mapped bytes to the kernel diagnostic outputs. | Nonnegative byte count written; otherwise a negative error. |
| 6 | `SYS_BLOCK_TRANSACT` | 3 | `a0=block_slot`, `a1=request_ptr`, `a2=reply_out`; submit one fixed-size block-protocol request. | `0` means delivered; the block operation outcome is encoded in the reply buffer. A negative value is a syscall-level error. |
| 7 | `SYS_STORE_TRANSACT` | 3 | `a0=store_slot`, `a1=request_ptr`, `a2=reply_out`; submit one fixed-size object-store request. | `0` means delivered; the store operation outcome is encoded in the reply buffer. A negative value is a syscall-level error. |
| 8 | `SYS_HEALTH_CONFIRM` | 1 | `a0=generation_control_slot`; confirm the currently running pending generation. | `0` on confirmation; otherwise a negative error. |
| 9 | `SYS_UNHEALTHY` | 0 | Terminate the current task with the unhealthy reason. | Does not return. |
| 10 | `SYS_RECOVERY_RECONSTRUCT` | 3 | `a0=generation_control_slot`, `a1=block_slot`, `a2=flags`; scrub and reconstruct BootState on the authorized repair target. | `0` on reconstruction; otherwise a negative error. |
| 11 | `SYS_ENDPOINT_CREATE` | 1 | `a0=factory_slot`; mint a bounded endpoint pair through an `EndpointFactory`. | Primary and auxiliary returns are the two nonnegative endpoint capability slots; otherwise the primary return is a negative error. |
| 12 | `SYS_SUPERVISION_STATUS` | 1 | `a0=supervision_slot`; poll a child and consume the handle when a terminal result is returned. | `-3` means still live. Nonnegative primary values are typed statuses: `0` exit, `1` fault, `2` timeout, `3` peer loss, `4` unhealthy; auxiliary return carries exit status or fault detail where applicable. Other negative values are errors. |
| 13 | `SYS_CAP_DROP` | 1 | `a0=capability_slot`; release the caller's capability. | `0` on release; otherwise a negative error. |
| 14 | `SYS_DIRECTORY_INSPECT` | 4 | `a0=directory_slot`, `a1=required_rights`, `a2=root_out`, `a3=scope_out`; verify rights and return the immutable root plus bounded scope. | Nonnegative scope byte length; otherwise a negative error. |
| 15 | `SYS_DIRECTORY_DERIVE` | 4 | `a0=directory_slot`, `a1=relative_path_ptr`, `a2=path_len`, `a3=rights`; derive a subdirectory-scoped, narrow-rights capability. | Nonnegative derived capability slot; otherwise a negative error. |
| 16 | `SYS_DIRECTORY_COMMIT` | 3 | `a0=directory_slot`, `a1=expected_root_ptr`, `a2=new_root_ptr`; atomically replace the unscoped namespace root. | `0` on commit, `-3` when the expected root is stale, or another negative error. |
| 17 | `SYS_INPUT_READ` | 1 | `a0=input_slot`; read one decoded keyboard event through explicit input authority. | Primary `0` with the encoded event in the auxiliary return, `-3` when no event is ready, or another negative error. |
| 18 | `SYS_GENERATION_TRANSACT` | 3 | `a0=generation_control_slot`, `a1=request_ptr`, `a2=reply_out`; submit one fixed-size generation-management request. | `0` means delivered; the management outcome is encoded in the reply buffer. A negative value is a syscall-level error. |
| 19 | `SYS_GENERATION_RECEIVE` | 2 | `a0=receiver_block_slot`, `a1=source_block_slot`; receive and validate a generation from an authorized transfer source. | `0` on completed receive; otherwise a negative error. |
| 20 | `SYS_WAIT` | 2 | `a0=descriptors_ptr`, `a1=count`; park until one of up to `MAX_WAIT_SOURCES` endpoint, send-capacity, input, or supervision sources may be ready. | `0` after wake or immediate readiness; callers re-poll their sources. Malformed input returns a negative error. |
| 21 | `SYS_SHARED_BUFFER_CREATE` | 3 | `a0=factory_slot`, `a1=pages`, `a2=writable`; allocate a quota-charged shared buffer. | Primary return is the nonnegative capability slot and auxiliary return is the kernel-assigned buffer identity; otherwise the primary return is a negative error. |
| 22 | `SYS_SHARED_BUFFER_RELEASE` | 1 | `a0=buffer_slot`; release the holder's shared buffer and invalidate the capability. | `0` on release; otherwise a negative error. |
| 23 | `SYS_SHARED_BUFFER_MAP` | 5 | `a0=buffer_slot`, `a1=virtual_base`, `a2=offset`, `a3=length`, `a4=writable`; map an exact page-aligned subrange. | `0` on mapping; otherwise a negative error. |
| 24 | `SYS_SHARED_BUFFER_UNMAP` | 2 | `a0=buffer_or_loan_slot`, `a1=virtual_base`; remove the caller's exact mapping and return its mapping charge. | `0` on unmap; otherwise a negative error. |
| 25 | `SYS_SHARED_BUFFER_SEAL` | 1 | `a0=buffer_slot`; irreversibly seal a buffer read-only and downgrade live writable mappings. | `0` on seal; otherwise a negative error. |
| 26 | `SYS_SHARED_BUFFER_LOAN` | 4 | `a0=buffer_slot`, `a1=receiver_supervision_slot`, `a2=offset`, `a3=length`; create a receiver-bound, single-return loan of a sealed subrange. | Primary return is the nonnegative loan capability slot and auxiliary return is the kernel-assigned loan identity; otherwise the primary return is a negative error. |
| 27 | `SYS_SHARED_BUFFER_LOAN_MAP` | 4 | `a0=loan_slot`, `a1=virtual_base`, `a2=offset`, `a3=length`; map a read-only subrange relative to the loan. | `0` on mapping; otherwise a negative error. |
| 28 | `SYS_SHARED_BUFFER_RETURN` | 1 | `a0=loan_slot`; settle the receiver's loan once and invalidate its capability. | `0` on return; otherwise a negative error. |
| 29 | `SYS_SHARED_BUFFER_REVOKE` | 2 | `a0=buffer_slot`, `a1=loan_id`; settle an outstanding loan as its lender. | `0` on revoke; otherwise a negative error. |
| 30 | `SYS_CAP_TRANSFER` | 3 | `a0=endpoint_slot`, `a1=capability_slot`, `a2=descriptor_ptr`; consume one capability and move a descriptor-bound, narrow-rights copy to the endpoint peer. | `0` on delivery; otherwise a negative error and the source capability remains intact. |

## Error model

The syscall return is an `i64`. Negative values are errors; nonnegative values
have the per-call meaning in the table.

| Value | Constant | Meaning |
| --- | --- | --- |
| 0 | `ERR_SUCCESS` | Successful completion or delivery. |
| -1 | `ERR_BAD_CAP` | Missing capability, wrong object kind, or insufficient rights. |
| -2 | `ERR_PEER_DEAD` | The endpoint peer is dead. |
| -3 | `ERR_WOULDBLOCK` | The operation is not ready without blocking, or an optimistic state check is stale. |
| -4 | `ERR_INVALID_ARG` | An argument, mapped range, descriptor, request, or bounded count is invalid. |
| -5 | `ERR_OUT_OF_MEMORY` | A task, capability, frame, object, byte, mapping, loan, or declared quota bound is exhausted. |

`SYS_SUPERVISION_STATUS` uses nonnegative primary returns as a typed termination
status rather than plain success. `SYS_BLOCK_TRANSACT`, `SYS_STORE_TRANSACT`, and
`SYS_GENERATION_TRANSACT` use `0` to mean that the request was delivered; the
operation-specific result is inside the reply buffer.

## x86-64 calling convention

The implemented x86-64 ABI enters through `int 0x80`. The kernel names vector
`0x80` as `SYSCALL_VECTOR`, and installs its IDT gate with attributes `0xEE`, so
ring 3 may invoke it.

| Role | Register |
| --- | --- |
| Syscall number | `rax` |
| `a0` | `rdi` |
| `a1` | `rsi` |
| `a2` | `rdx` |
| `a3` | `r10` |
| `a4` | `r8` |
| Primary return | `rax` |
| Auxiliary return | `rdx` |

The auxiliary return is defined for `SYS_SPAWN`, `SYS_ENDPOINT_CREATE`,
`SYS_SHARED_BUFFER_CREATE`, `SYS_SHARED_BUFFER_LOAN`,
`SYS_SUPERVISION_STATUS`, and `SYS_INPUT_READ`. All general-purpose registers
are saved by the trap stub and restored from the mutable trap frame. Registers
not used for a return therefore retain their input values; in particular,
`rcx` and `r11` are not implicit clobbers as they are for the x86-64 `syscall`
instruction.

## AArch64 calling convention

The architecture-specific trap instruction is `svc`. The kernel currently
exports only `arch::x86_64`, so AArch64 syscall entry is not implemented.
Register assignment, exception-entry state, return registers, and clobbers are
not yet defined. The semantic syscall table and error model above are shared by
contract; P2, the AArch64 QEMU vertical slice, defines and implements the
calling convention without changing those semantics.

## RV64 calling convention

The architecture-specific trap instruction is `ecall`. The kernel currently
exports only `arch::x86_64`, so RV64 syscall entry is not implemented. Register
assignment, trap-entry state, return registers, and clobbers are not yet
defined. The semantic syscall table and error model above are shared by
contract; P3, the RV64 QEMU vertical slice, defines and implements the calling
convention without changing those semantics.

## Cross-architecture invariant

Syscall numbers, error values, capability checks, message bounds, and transfer
semantics are identical across calling conventions. Only the trap instruction
and the mapping between semantic arguments or returns and architecture
registers differ.
