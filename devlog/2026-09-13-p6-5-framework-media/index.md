# P6.5: one raw image becomes the QEMU-proven Framework boot medium

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Verified |
| Scope | `contracts/boot-media/v1/schema.zt`, `scripts/lib/{framework_media,pc99_media}.py`, `scripts/build/{build-framework-media,build-sel4,write-removable-image}.py`, `scripts/check/{check-contracts,check-framework-media}.py`, `slime-root/{build.rs,src/graph_runtime/services.rs}`, `just/{generate,product}.just` |
| Work items | 01a09e12-124b-7ee5-b223-a024983557d8 |
| Gates | `just framework_media_check`, `just x86_64_sel4_root_boot_check`, `just fmt_check_all`, `just lint_all`, `just ruff` |
| Trigger | P6.4 left a reproducible EFI tree but no raw GPT/FAT32 artifact whose exact bytes could be written to USB and replayed under QEMU |
| Baseline | P6.2-P6.4's pinned q35/OVMF GRUB Multiboot2 tree and resident x86-64 product graph |

## Summary

P6.5 now produces `build/framework-media/slime-framework.img`, one 65 MiB raw GPT disk with exactly one FAT32 EFI System Partition and no product/state partition. A versioned Zutai contract defines the persisted identity record and fixed GPT/FAT geometry; the record binds the complete image, Framework target profile, source seL4 identity manifest, exact four-file EFI tree, partition layout, and image digest. The gate rebuilds the image twice byte-for-byte, rejects malformed or drifted media and unsafe writer targets, then boots the exact raw image under pinned q35/OVMF until the Framework-qualified resident Slisp graph reaches `slisp>`. No physical Framework or device-qualification claim is made.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `contracts/boot-media/v1` | Defines `BootMediaIdentity`, its nested records, bounded constants, deterministic GPT/FAT geometry, fixed GUIDs, and generated Python constants | Every persisted boot-medium fact has one versioned Zutai source of truth |
| `scripts/build/build-sel4.py` | Adds the isolated `framework13-ai300` build platform over the P6.4 pc99 kernel/prefix while compiling every executable for `x86_64-sel4-framework13-ai300`; excludes QEMU COM1 input authority | The first physical artifact contains no QEMU-qualified executable and needs no keyboard input |
| `scripts/lib/framework_media.py` | Builds protective MBR, primary/backup GPT headers and entry tables, one read-only ESP, deterministic FAT32 bytes, canonical identity; validates the complete record, GPT CRCs/geometry, partition set, required EFI file hashes, and tree digest | The bytes QEMU boots, the bytes the writer accepts, and the bytes the identity names are identical |
| `scripts/lib/pc99_media.py` | Boots a raw image through an ephemeral QEMU overlay as an alternative to the P6.2 synthetic FAT tree | Firmware may perform transient block writes without modifying the proven raw image |
| `scripts/build/write-removable-image.py` | Requires canonical media validation before target resolution, revalidates the removable whole disk after confirmation, handles partial writes, fsyncs, drops cache, and hashes the full written length | A partition, mounted/non-removable/read-only disk, short write/read, or read-back mismatch cannot become an approved USB write |
| `scripts/check/check-framework-media.py` | Owns deterministic rebuild, malformed/missing/drift/wrong-target/extra-partition controls, writer safety controls, and exact raw-media boot to the resident product marker | P6.5 has one narrow, reproducible, fail-closed verification boundary |
| `slime-root` | Emits `SLIME_ROOT READY target_profile=…` for the Framework-qualified resident graph | P6.6 can bind physical readiness evidence to the exact profile without enabling a device |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| Host timestamps, directory order, GUIDs, or FAT metadata make image bytes vary | `just framework_media_check` | The second `--no-build` raw image or identity record differs byte-for-byte |
| A forged/self-consistent record omits boot files or changes disk policy | `just framework_media_check` and canonical validation in `framework_media.py` | Missing/unknown record fields, non-canonical disk layout, or any file set other than the four pinned EFI paths is rejected before write or boot |
| QEMU proves a directory while USB receives different bytes | `just framework_media_check` | QEMU is launched with `build/framework-media/slime-framework.img`; image digest, GPT, FAT file digests, and Framework profile are checked first |
| The image gains writable product storage or an unexpected partition | `just framework_media_check` | Any partition table other than one read-only ESP fails, including the explicit extra-partition mutation |
| The writer targets internal or active storage | `just framework_media_check` | Partition, non-removable, read-only, or mounted-device controls fail closed; target identity is rechecked after confirmation |
| Existing qemu-pc99 behavior regresses | `just x86_64_sel4_root_boot_check` | P6.2/P6.3 root boot marker chain fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just framework_media_check` | pass — contracts and generated bindings current; two 68157440-byte images byte-identical; image SHA-256 `290ec23e165ffe788dec37fc8e26840951f0bcae70d80ee6dfb4120eefb55bd1`; missing image/file, digest drift, wrong target, malformed GPT/FAT tree, unexpected partition, unsafe target, short read/write rejected; exact raw image booted under pinned q35/OVMF to `SLIME_ROOT READY target_profile=x86_64-sel4-framework13-ai300` and `slisp>` | Direct |
| `just x86_64_sel4_root_boot_check` | pass — existing qemu-pc99 root boot path unchanged | Direct |
| `just fmt_check_all` | pass | Direct |
| `just lint_all` | pass | Direct |
| `just ruff` | pass | Direct |
| `just contracts_check` within `framework_media_check` | pass — including boot-media schema/binding freshness, 311 boot-contract tests, four model suites, and generation-v5 checks | Direct |

## Decisions

- **Decision:** Reuse the P6.4 pc99 seL4 kernel and installed prefix while building root, child, components, generation, manifest, and output directories under a distinct Framework platform/profile.
  **Rationale:** P6.5 owns target-qualified executable and media identity; P6.6/H1 own observed firmware and hardware facts. A second unobserved kernel configuration would add an unsupported machine claim.
  **Rejected alternative:** Labeling the qemu-pc99 graph as Framework media, which target admission correctly rejects and which would preserve QEMU input authority.

- **Decision:** Keep exactly one ESP and set GPT's read-only partition attribute; do not add a state partition.
  **Rationale:** No storage milestone owns writable on-media state yet. Absence is stronger and testable.
  **Rejected alternative:** Reserving an unused writable partition for later, which would violate P6.5's explicit boundary.

- **Decision:** Let QEMU write only to an ephemeral overlay while reading the exact raw image.
  **Rationale:** OVMF may issue block writes during boot; a snapshot overlay permits firmware behavior while preserving the artifact digest that P6.6 will write.
  **Rejected alternative:** Making the raw artifact writable during the gate or booting a separately assembled directory.

## Open risks and follow-ups

- [ ] P6.6 must observe two cold boots of this exact image on the named Framework, bind firmware and USB identities, expose bounded GOP evidence, and prove the protected internal-NVMe region unchanged.
- [ ] The Framework target still intentionally reuses the pc99 kernel configuration and HPET assumptions; H1 owns real ACPI/APIC/timer/device inventory and any resulting kernel changes.
- [ ] No physical removable disk was written in this change. Writer safety and complete read-back were exercised against bounded regular-file controls; the first real USB write belongs to P6.6 evidence.

## Artifacts and provenance

- Generated artifact: `build/framework-media/slime-framework.img` (not committed), SHA-256 `290ec23e165ffe788dec37fc8e26840951f0bcae70d80ee6dfb4120eefb55bd1`.
- Generated identity: `build/framework-media/slime-boot-media.identity.json` (not committed), identity `5c4c08f80b35220a0c5156938e3471208aa594507f8383f697eaa23195c37649`.
- Source identity: `build/slime-sel4-graph-framework13-ai300.identity.json` (not committed), generation identity prefix `0f1929f39bf5fa1f`.
- Related roadmap item: [P6.5](../../roadmap/07-architecture-portability.md#p65--deterministic-uefi-removable-media-image).

## Corrections

- **2026-09-13 — the recorded boot-media identity digest was wrong.** This
  entry's *Artifacts and provenance* section gives the identity as
  `5c4c08f80b35220a0c5156938e3471208aa594507f8383f697eaa23195c37649`. Rebuilding
  the image from the recorded commit reproduces the image digest
  `290ec23e…f55bd1` exactly, but the identity record it writes hashes to
  `5c4c08f80b35220aac60362145c10ca113fe576adfab06ea6e46691ef89d4c2d`, which
  `framework_media.record_identity` recomputes and the media gate accepts. The
  two values share exactly their first sixteen hex digits — the width the
  builder's own `identity:5c4c08f80b35220a…` progress line prints — so the
  recorded value was a truncated digest extended with unrelated digits rather
  than a digest of a different record. No artifact was wrong and no gate was
  weakened; the image digest, which is what the writer and QEMU compare, was
  recorded correctly. Corrected here rather than edited above because the body
  of a landed entry is frozen.
