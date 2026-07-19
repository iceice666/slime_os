#!/usr/bin/env python3
# M5.1: storage-authority allowlist.
#
# This script evolved from the original "no storage mechanism exists" check.
# The kernel now carries generic device-resource capabilities (PCI functions,
# DMA memory, interrupts, shared buffers) and a block request/reply protocol.
# Those are *mechanisms*, not *authority*: authority is granted only through
# explicit capability grants recorded in the generation manifest.
#
# The property this script proves is narrower and stronger:
#
#   No component receives ambient disk-write authority.
#
# Concretely:
#   1. No forbidden storage-write token (nvme/ahci/ata/virtio_blk write path,
#      SYS_BLOCK_WRITE, RIGHT_BLOCK_WRITE, disk_write) appears in kernel
#      source outside an explicitly allowlisted file.
#   2. The kernel object surface is exactly the allowlisted set — no object
#      type smuggles a storage-write handle.
#   3. The syscall surface is exactly the allowlisted set — no syscall hands
#      out storage-write authority to an arbitrary caller.
#   4. The capability rights bitfield does not define RIGHT_BLOCK_WRITE or any
#      storage-write right. (M5.3 will introduce an explicitly granted write
#      right and update this allowlist at that time.)
#
# When M5.3 adds a granted block-write right, the allowlist here must be
# extended to require that the right appears only behind an explicit
# generation-manifest grant to the storage service, not as an ambient syscall
# or a default component capability.

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL = ROOT / "kernel" / "src"

# Tokens that indicate a storage-write path. Pure read capability names
# (virtio, block) are NOT forbidden — the block protocol module is allowed —
# but write-specific tokens are.
FORBIDDEN = re.compile(
    r"\b(?:nvme|ahci|ata|disk[_-]?write|SYS_BLOCK_WRITE|RIGHT_BLOCK_WRITE|"
    r"RIGHT_DISK_WRITE|block[_-]?write[_-]?right|storage[_-]?write)\b",
    re.IGNORECASE,
)
ALLOWED_FILES = {
    KERNEL / "main.rs",  # Runtime diagnostic states that the authority is absent.
    KERNEL / "virtio_blk.rs",  # M5.2 transport defines only read requests.
}

# The exact syscall surface the kernel is allowed to expose. M5.1 keeps the
# M2/M3 set; device-resource operations are gated by capability rights on the
# existing SYS_SEND/SYS_RECV/SYS_SPAWN path plus the M5.1 capability-derive
# syscall added below.
ALLOWED_SYSCALLS = {
    "SYS_YIELD",
    "SYS_SEND",
    "SYS_RECV",
    "SYS_EXIT",
    "SYS_SPAWN",
    "SYS_DEBUG_WRITE",
    "SYS_BLOCK_TRANSACT",
}

# The exact kernel-object surface the kernel is allowed to expose. M5.1 adds
# the four generic device-resource objects. No object here represents a
# storage-write handle; a future storage-write object would require extending
# this allowlist with a grant-traceability proof.
ALLOWED_KERNEL_OBJECTS = {
    "Endpoint",
    "Executable",
    "PciFunction",
    "DmaMemory",
    "Irq",
    "SharedBuffer",
    "BlockDevice",
}

# Rights bit names the kernel is allowed to define. Notably absent:
# RIGHT_BLOCK_WRITE / RIGHT_DISK_WRITE / any storage-write right.
ALLOWED_RIGHTS = {
    "RIGHT_SEND",
    "RIGHT_RECV",
    "RIGHT_TRANSFER",
    "RIGHT_EXEC",
    "RIGHT_MAP_MMIO",
    "RIGHT_DMA_PIN",
    "RIGHT_DMA_RELEASE",
    "RIGHT_IRQ_ACK",
    "RIGHT_BUFFER_WRITE",
    "RIGHT_MAP",
    "RIGHT_BLOCK_READ",
    # Composite mask, not a grantable right. Must never include a
    # storage-write bit (forbidden by the forbidden-token check).
    "RIGHT_ALL",
}


def fail(message: str) -> None:
    raise SystemExit(message)


def rust_sources() -> list[Path]:
    return sorted(KERNEL.rglob("*.rs"))


def check_forbidden_tokens() -> None:
    findings: list[str] = []
    for path in rust_sources():
        if path in ALLOWED_FILES:
            continue
        for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
            if FORBIDDEN.search(line):
                findings.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()}")
    if findings:
        fail("storage-write authority token found in kernel surface:\n" + "\n".join(findings))


def check_syscall_surface() -> None:
    text = (KERNEL / "syscall" / "mod.rs").read_text(encoding="utf-8")
    actual = set(re.findall(r"pub const (SYS_[A-Z0-9_]+):", text))
    if actual != ALLOWED_SYSCALLS:
        extra = actual - ALLOWED_SYSCALLS
        missing = ALLOWED_SYSCALLS - actual
        fail(
            "kernel syscall surface changed from the allowlist:\n"
            f"  extra:   {sorted(extra)}\n"
            f"  missing: {sorted(missing)}\n"
            "Storage-write syscalls are forbidden; new syscalls must be "
            "explicitly allowlisted here."
        )


def enum_variants(text: str, enum_name: str) -> set[str]:
    match = re.search(rf"pub enum {enum_name}\s*\{{(?P<body>.*?)^\}}", text, re.MULTILINE | re.DOTALL)
    if match is None:
        fail(f"cannot locate enum {enum_name}")
    variants: set[str] = set()
    for line in match.group("body").splitlines():
        candidate = line.strip().split("(", 1)[0].split("{", 1)[0].rstrip(",")
        if candidate and not candidate.startswith("//"):
            variants.add(candidate)
    return variants


def check_capability_surface() -> None:
    text = (KERNEL / "capability" / "mod.rs").read_text(encoding="utf-8")
    actual = enum_variants(text, "KernelObject")
    if actual != ALLOWED_KERNEL_OBJECTS:
        extra = actual - ALLOWED_KERNEL_OBJECTS
        missing = ALLOWED_KERNEL_OBJECTS - actual
        fail(
            "kernel object surface changed from the allowlist:\n"
            f"  extra:   {sorted(extra)}\n"
            f"  missing: {sorted(missing)}\n"
            "A new object type that carries storage-write authority is "
            "forbidden until it is covered by an explicit grant-traceability "
            "proof in this allowlist."
        )


def check_rights_surface() -> None:
    text = (KERNEL / "capability" / "mod.rs").read_text(encoding="utf-8")
    actual = set(re.findall(r"pub const (RIGHT_[A-Z0-9_]+):", text))
    if actual != ALLOWED_RIGHTS:
        extra = actual - ALLOWED_RIGHTS
        missing = ALLOWED_RIGHTS - actual
        fail(
            "capability rights surface changed from the allowlist:\n"
            f"  extra:   {sorted(extra)}\n"
            f"  missing: {sorted(missing)}\n"
            "RIGHT_BLOCK_WRITE / RIGHT_DISK_WRITE / any storage-write right is "
            "forbidden at M5.1. M5.3 may add an explicitly granted write right "
            "and must extend this allowlist with a grant-traceability proof."
        )


def main() -> None:
    check_forbidden_tokens()
    check_syscall_surface()
    check_capability_surface()
    check_rights_surface()
    print("kernel storage authority allowlist check: ok")


if __name__ == "__main__":
    main()
