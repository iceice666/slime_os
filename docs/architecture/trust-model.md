# Trust model

Who is trusted, what each boundary enforces today, and what is explicitly
*not* enforced. This page collects statements that otherwise live scattered
across the capability matrix, the target profiles, the I/O substrate, and the
plans, so that a security question has one place to start. Each statement
points at its owner; nothing here is authoritative on its own.

The one-sentence version: **the pinned seL4 kernel and `slime-root` are the
trusted computing base; every component holds exactly the authority its
admitted generation declares, and every claim beyond QEMU is bound to a named
target and an observed boot.**

## Trusted computing base

| Trusted | Why it must be | Identity pinned by |
| --- | --- | --- |
| Pinned seL4 kernel | From `just run` onward, the only privileged code. | `sel4/pins.toml` `[sel4]` commit and kernel-config hashes; `just sel4_pin_check` |
| Kernel loader (rust-sel4 fork) | Runs before the kernel; sets up the boot environment. | `sel4/pins.toml` `[rust_sel4*]`; per-platform loader forks pinned separately |
| `slime-root` | Owns all dynamic mechanism: object allocation, task construction, the syscall surface, supervision, and admission. A root bug is a system bug. | Built with `panic = "abort"` — resource rollback and fault supervision are explicit, never Rust unwinding. Its tree digest is part of every system-image closure. |
| The admitted generation | Declares every executable, instance, grant, budget, and interface the root will construct; the root builds nothing else. | SHA-256 identity over the whole blob, every payload re-hashed, version and reserved fields refused if unexpected (`boot-contracts/src/generation.rs`). |
| Host toolchain and build | Produces the bytes above. | Both Rust toolchains, target-spec bytes, and the seL4 prefix pinned; `cargo-deny` refuses unknown registries and unlisted git sources (`deny.toml`); closures digest every source tree the image is built from. |
| Firmware on physical targets | Hands control to the loader. | Named only (vendor/version in the CPU-boot observation); not verified. Secure Boot was off in the recorded Framework boot. |

Components are **not** trusted. A component is a fault-containment unit that
receives its capabilities from the root at spawn and can obtain more only by
transfer from a holder that already has them.

## Boundaries and what they enforce

### Root ↔ component

- **Authentication is the badge.** Every root request arrives on a badged
  endpoint the root minted at task construction; the root resolves the badge
  to the task and ignores any identity the message claims. A component cannot
  speak as another. ([capability-matrix](../capability-matrix.md), "Badge
  authentication"; `slime-root/src/ipc.rs`)
- **Rights are bits the root checks, one per operation.** Narrowing only;
  `narrow` never widens, and kinds refuse bits meaningless for them. New
  rights ship with a gate. (matrix, "Grammar")
- **Object creation is root-only.** Endpoints and notifications are created
  while decoding the generation; userspace holds, derives, and transfers, and
  cannot forge object identities. The one runtime mint (`SHARED BUFFER
  CREATE`) is both right-gated and budget-bounded.
- **Transfer checks transferability on every moved capability**, on both the
  export/import protocol and `SPAWN` grants; a receiver declared
  `deterministic` cannot be widened by import.
- **Code is only boot-verified generation bytes.** An `Executable` capability
  references a hash-verified, target-qualified module; there is no path from
  data to executable authority. Bare ELF payloads are refused at admission.
- **Everything is bounded.** Tasks, capabilities per task, message bytes,
  shared buffers, mappings, loans, timers, threads, IO resources, directory
  depth — each has a hard bound named in the matrix. "Unbounded" is not an
  error strategy; exhaustion is a typed refusal.
- **Allocation requires grant *and* budget.** A grant without a declared
  budget entry, or a budget without a grant, allocates nothing.

### Component ↔ component

- Native endpoint IPC between components is declared as a grant in the system
  spec and materialized by the root; the root neither sees nor mediates the
  traffic afterwards. A received capability arrives with exactly the rights
  the sender named and the root authenticated.
- Fabric routes are authorized by the generation-provisioned control endpoint
  a request arrives on, never by the route name, direction, or type identity
  in the request; `just fabric_authority_check` observes a byte-identical
  forged route being denied with no capability attached.
- Shared-buffer slices carry no physical address or IOVA; ordinary service
  clients never receive raw DMA or MMIO authority.

### Fault containment

A faulting component thread is left suspended by the kernel; the root decodes
the fault into a portable record retaining no kernel-virtual, physical, CSpace,
or object identifier (`slime-root/src/fault.rs`). Its parent observes a typed
termination reason, deliberately not an address, because an address would leak
the child's layout (`supervision.rs`). Teardown runs Suspend → IO → Buffers →
Arena and retains the record until the arena is reclaimed, so a raw VSpace
capability is never reused while still nameable. Termination reclaims TCBs,
CNode, VSpace, IPC buffers, frames, IO resources, and charges; other subtrees'
accounts are unaffected. `just sel4_fault_check` boots a plane whose
interposition hop is compiled to die and observes the degradation envelope.

### Memory

Private component memory is a deny-by-default generation quota, not a
capability; a region cannot be transferred, loaned, sealed, shared,
file-backed, or made executable. Isolation is observed by
`sel4-private-memory-isolation`: a foreign access faults with its own task and
exact address, and a readmitted holder sees zeroed pages. See
[private memory](private-memory.md).

### Storage and generations

A generation is admitted only if every payload hashes and is qualified for the
exact target profile; a superseded format is refused, never migrated. Selector
images verify a detached release against the built-in initial trust root
(threshold signatures; sequence must advance) before reading candidate bytes,
and BootState commits older-slot-first so no transition overwrites the only
valid root. `just release_trust_check` proves the refusals. See
[generation management](generation-management.md).

### Devices

A driver receives only generation-declared resources: one device, exact MMIO
subranges, declared interrupt sources, a bounded DMA account, queue bindings,
and supervision. On QEMU, virtio transports share one granule, so MMIO is
mediated `read32`/`write32` rather than mapped. Per-ring block rights and
sector ceilings are driver policy the root copies but does not interpret.
See [I/O substrate](io-substrate.md).

## Explicitly not enforced today

Read these as the current attacker envelope, each with its owner:

- **DMA containment.** QEMU DMA is trusted; no IOMMU/SMMU claim exists for any
  target. A hostile or buggy driver with DMA authority is not contained by
  hardware. On the Framework's AMD platform the kernel cannot supply it either:
  seL4's `KernelIOMMU` drives Intel VT-d only, so containment is pending Slime
  mechanism. ([targets](targets-and-portability.md); [Framework
  plan](../plans/framework-hardware.md); [AMD IOMMU
  ownership](../decisions/amd-iommu-ownership.md))
- **x86-64 W^X.** seL4's x86-64 mapping attributes expose no NX bit, so
  data-page execute prevention is unenforced there; the probe reports
  `wx_execute=unenforced` rather than passing vacuously. AArch64 and RISC-V
  map component data execute-never.
- **AArch64 EL0 timer registers.** QEMU exposes the physical counter/timer
  registers to EL0 globally because the root uses that path; clock-service
  authority does not hide the raw registers from hostile native code. Closing
  this needs a kernel/platform change ([runtime authority](runtime-authority.md)).
- **Named but ungated rights bits** (`storeRead`, `storeWrite`,
  `healthConfirm`, `bootUpdate`) exist in the vocabulary without an enforcing
  operation; reassign deliberately (matrix, "Ungated residue").
- **Revocation, leases, and provenance** — a granted capability is held until
  the holder dies; there is no subtree revoke, expiry, or use-after-revoke
  refusal. ([authority plan](../plans/authority-and-trust.md) A1)
- **Secrets as capabilities**, accelerator DMA authority, TPM-bound attestation,
  and distributed (cross-machine) capabilities are plans, not code (A2–A5).
- **Disk-only rollback protection can be rolled back** by whoever can write
  the disk; there is no TPM-bound attempt counter.
- **Physical claims stop at the CPU boot.** One named Framework 13 booted the
  product image from removable media without writing internal NVMe. No
  device, input, network, display-driver, suspend, or daily-driver claim
  follows, and the recorded observation is bound to the exact image that was
  booted ([Framework runbook](../operations/framework-cpu-boot-runbook.md)).
- **Firmware and hardware** below the loader are named, not verified.
- **Host-side tooling** (`just`, Python checkers, Nix) is trusted to produce
  and judge evidence; the gate-control check proves the marker matchers fail
  closed, not that the host is uncompromised.

## Verification pointers

- `just sel4_pin_check`, `just deny`, `just system_image_builder_check` —
  supply-chain identity.
- `just sel4_gate_control_check` — every marker gate rejects forged, missing,
  or reordered evidence.
- `just fabric_authority_check`, `just sel4_fault_check`,
  `just private_memory_isolation_check`, `just release_trust_check`,
  `just framework_safety_check` (the storage-writer allowlist) — the
  boundary proofs named above.
- [`capability-matrix.md`](../capability-matrix.md) — the exhaustive rights
  and bounds table; change it in the same commit as the surface it describes.
