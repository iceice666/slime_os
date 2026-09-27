# Generation management, selection, rollback, and recovery

What currently exists between "a generation is bytes" and "this machine boots
that generation next time": the on-disk BootState record, the pre-admission
boot selector compiled into selector images, the userspace generation manager
and its command protocol, and the recovery and transfer reconstructions. Every
piece is QEMU-qualified against a virtio disk; none of it has been observed on
physical storage, and none of it yet constitutes rollbackable production
generations (see the boundary at the end).

[Generations](../concepts/generations.md) states the model. The
[component/system/image page](component-system-image.md) owns how a generation
is *built*; this page owns what happens to one after it is built.

## Ownership

| Concern | Owner | Notes |
| --- | --- | --- |
| Generation wire format and identity | `contracts/generation/v5/schema.zt`, `boot-contracts/src/generation.rs` | Identity is SHA-256 over the whole blob with the identity field zeroed. `SLIMEG2/3/4` decode as `UnsupportedVersion`; no migration path exists. |
| Admission of a decoded generation | `slime-root/src/generation.rs` (`Admission::admit`, `admit_total_slots`) | Target-profile qualification of every payload, whole-plan CSlot costing, fail-closed resource budgets. |
| BootState: the two redundant slots | `contracts/bootstate/v1/schema.zt`, `boot-contracts/src/bootstate.rs` | `known_good`, `pending`, `remaining_attempts`, `generation_root`, `state_root`, `accepted_release_sequence`, sequence, checksum. The state machine is model-checked in `contracts/bootstate/model/bootstate.zt`. |
| Pre-admission selection | `slime-root/src/boot_selector.rs`, block reader `slime-root/src/boot_selector_block.rs` | Compiled only into `slime_boot_selector` images; the product image embeds its generation and never selects. |
| Signed release and trust root | `contracts/release/v1/schema.zt`, `boot-contracts/src/release.rs` | Threshold signatures over generation identity, parent, sequence, target, boot-bundle identity. |
| Command protocol | `contracts/generation-management/v1/schema.zt` → `components/proto/src/generation.rs` | Fixed 64-byte request/reply: `LIST`, `INSPECT`, `STAGE`, `SELECT`, `ROLLBACK`. |
| Userspace manager | `components/services/sel4-generation-manager/src/main.rs` | The one component in its plane holding block authority; clients hold an endpoint and nothing else. |
| Recovery index | `contracts/recovery/v1/schema.zt`, probe `components/testkit/sel4-recovery-probe` | Signed index embedded in a recovery generation; reconstruction of BootState from it. |
| Transfer manifest | `contracts/transfer/v1/schema.zt`, probe `components/testkit/sel4-transfer-probe` | Read-only source device → writable receiver, closure verified before BootState changes. |

## BootState and the commit rule

Two 512-byte slots hold the record redundantly. Every writer — selector,
manager, probes — follows the same rule, which is the whole redundancy
argument: **write the older slot, flush, re-read both slots from the device,
and require that what a fresh boot would select is exactly what was written**
(`StateSlots::commit` in the manager; `commit_state` in the selector). A
transition therefore never overwrites the only valid root, and a torn write
leaves the previous record selectable. Both slots decoding to different
records at the same sequence is `ConflictingSlots`, refused rather than
resolved.

The record carries the *lineage* the system actually uses at runtime:
`known_good` and `pending` identities, the attempt budget, and the accepted
release sequence. The generation header's own 32-byte `parent` field exists in
the v5 schema but is written as zero by `scripts/build/build-generation.py`
and read by nothing in `slime-root`; treat BootState plus the signed release
sequence as the parent chain, not the header.

## Selection before admission

A selector image (`just sel4_boot_selection_check`, built with
`SLIME_BOOT_SELECTOR=1` and structurally proven to embed no generation
bytes) performs, in `boot_selector.rs::select`:

1. locate the GPT store partition; read the bounded boot-store directory
   (≤ 64 entries, checksum verified);
2. read both BootState slots; refuse if `generation_root` disagrees with the
   directory root;
3. if a pending generation has attempts left, **consume one attempt and
   commit before reading a single candidate byte** — a candidate that hangs
   still costs its attempt;
4. locate the running identity in the directory, re-hash the bytes against it,
   decode, require v5;
5. decode and verify the detached release against the built-in initial trust
   root and the expected boot-bundle identity; a pending generation's release
   sequence must exceed the accepted one, a known-good's must not;
6. if the pending budget is exhausted, roll back and commit.

The root then prints `SLIME_BOOT selected identity=… number=… pending=…
attempts=…`. Promotion happens later in the root's graph runtime: when the
required-instance health condition is met, `BootRuntime::confirm` promotes a
running pending generation (`SLIME_BOOT promoted`); a component calling the
`unhealthy` syscall yields `SLIME_BOOT unhealthy`, and the consumed attempt is
never repaired. Candidate size is bounded by a 4 MiB static buffer that the
generation builder and the gate both pin.

## The userspace manager

`sel4-generation-manager` is policy, not root mechanism. It attaches an IO0
block ring from the userspace `virtio-blk-driver` under the plane's declared
`blockRingAuthority` (read and write, bounded sector limit), locates the store
partition, and serves one client over a native endpoint. Rules the serve loop
enforces:

- a request must carry the protocol magic and version, and must attach no
  capability — a client that sends one is a failure, not a feature;
- `STAGE` is refused before any write unless the identity is one the manager
  recognizes; a refused stage changes no disk bytes, which the gate proves by
  comparing the image;
- `SELECT` is the health confirmation: it promotes only the pending identity
  the client names; `ROLLBACK` with nothing pending is `STATUS_NO_PENDING`;
- the client's death is observed through its supervision capability, and the
  manager exits when its client is gone.

Each transition prints `[sel4-generation-manager] <op> seq=N pending=0|1
attempts=N release=N`. The gate (`just sel4_generation_check`) walks list →
unknown refusals → stage → rollback → refused select → select, on AArch64 and
(via `just riscv64_qemu_check`) RISC-V.

Current scope, stated plainly: the manager recognizes exactly two constant
identities (a known-good root and one candidate) and zeroes the reply's
flag/number/sequence metadata. It is a qualified protocol and BootState
implementation, not a generation store with a directory of real releases.

## Rollback, recovery, and transfer planes

Three testkit probes exercise the BootState contract from userspace, each the
only holder of block authority in its plane, each reaching disk over the
driver's IO0 rings:

- **Rollback** (`just sel4_rollback_check`): empty slots refused; genesis;
  stage with three attempts; attempts consumed 3 → 0; exhaustion; rollback;
  rollback idempotent; unauthorized promotion (wrong running identity, stale
  release) refused; promotion; both slots decode afterwards; the host re-reads
  the image and requires both slots to carry the BootState magic.
- **Recovery** (`just sel4_recovery_plane_check`): both slots corrupted →
  refused; a signed recovery index is read; the object closure it names is
  verified; BootState is reconstructed and converges on rerun; a second,
  read-only guard disk is proven byte-identical before and after, and a write
  to it is refused by driver rights.
- **Transfer** (`just sel4_transfer_check`): a read-only source device carries
  a transfer manifest; the receiver verifies the object and state closure and
  the signed release before staging; a tampered manifest is refused; stage and
  promote on the receiver.

The [release trust check](../../scripts/check/check-release-trust.py)
(`just release_trust_check`) is the host-side counterpart: it builds a signed
generation and proves below-threshold, missing, duplicate-key, malformed,
excess, wrong-target, stale-sequence, and unmagicked releases are refused, and
that trust-root rotation records are honored.

## Limits and boundaries

- **Not rollbackable production generations.** `AGENTS.md` lists them as
  unfinished. The product image `just run` boots embeds its generation; only
  selector images select from disk, and only QEMU virtio disks have been used.
  No physical NVMe write has been qualified, and the removable Framework
  medium carries no product or state partition by design
  (`contracts/boot-media/v1`).
- **State policies are decoded, not executed.** The generation format admits
  per-state `SNAPSHOT_BEFORE_UPGRADE` and `DISCARD_ON_ROLLBACK` policies and
  refuses unknown ones, but no code in `slime-root` or `components/` acts on
  them yet. Migration is by refusal: a superseded format counts as a failed
  generation.
- **The health-confirmation failure path is unexercised** by an observed QEMU
  failure in the upgrade/rollback check (see
  [component-system-image](component-system-image.md)); the selector's
  attempt-consumption and the `unhealthy` syscall path are exercised by
  `sel4_boot_selection_check`.
- **Two partition layouts coexist.** The selector reads BootState at
  partition-relative LBA 0–1 beneath a boot-store directory; the manager and
  the rollback/recovery/transfer probes use LBA 1024–1025 within the store
  partition `gpt::validate_store_partition` admits. No plane boots the selector
  against a disk the manager wrote; unifying them is unfinished work.
- **Disk-only rollback protection can itself be rolled back.** TPM-bound
  attempt counters and attestation are planned, not built
  ([authority and trust plan](../plans/authority-and-trust.md)).
- The legacy host-side `components/testkit/generation-{inspect,rollback,select,stage}`
  crates are declared by no seL4 composition and are dead on this kernel.

## Verification

- `just sel4_boot_selection_check` — selector: attempts, stale-format refusal,
  promotion, disk-unchanged proof, structural selector-image proof.
- `just sel4_generation_check` — manager protocol over IO0.
- `just sel4_rollback_check`, `just sel4_recovery_plane_check`,
  `just sel4_transfer_check` — BootState contract from userspace probes.
- `just release_trust_check` — signed-release refusals and rotation.
- `just contracts_check` / `just bootstate_model_check` — schema and
  model-checked state machine.
