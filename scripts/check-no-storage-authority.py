#!/usr/bin/env python3
# Storage-authority allowlist for the Framework-safe image.
#
# M5.3 introduces one explicit block-write right for disposable QEMU storage
# checks; M5.4 adds one explicit object-store right for the disposable QEMU
# store probe. M5.7 adds a common read-only NVMe backend. This checker proves
# that authority is neither ambient nor granted to the normal storage probe,
# and that the Framework boot path cannot enable test-only writes.

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL = ROOT / "kernel" / "src"
MANIFEST = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
INIT = ROOT / "components" / "src" / "init.S"
JUSTFILE = ROOT / "Justfile"

ALLOWED_SYSCALLS = {
    "SYS_YIELD",
    "SYS_SEND",
    "SYS_RECV",
    "SYS_EXIT",
    "SYS_SPAWN",
    "SYS_DEBUG_WRITE",
    "SYS_BLOCK_TRANSACT",
    "SYS_STORE_TRANSACT",
    "SYS_RECOVERY_RECONSTRUCT",
    "SYS_HEALTH_CONFIRM",
    "SYS_UNHEALTHY",
}
ALLOWED_KERNEL_OBJECTS = {
    "Endpoint",
    "Executable",
    "PciFunction",
    "DmaMemory",
    "Irq",
    "SharedBuffer",
    "BlockDevice",
    "ObjectStore",
    "GenerationControl",
}
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
    "RIGHT_BLOCK_WRITE",
    "RIGHT_STORE_READ",
    "RIGHT_STORE_WRITE",
    "RIGHT_HEALTH_CONFIRM",
    "RIGHT_BOOT_UPDATE",
    "RIGHT_ALL",
}


def fail(message: str) -> None:
    raise SystemExit(message)


def enum_variants(text: str, enum_name: str) -> set[str]:
    match = re.search(
        rf"pub enum {enum_name}\s*\{{(?P<body>.*?)^\}}", text, re.MULTILINE | re.DOTALL
    )
    if match is None:
        fail(f"cannot locate enum {enum_name}")
    variants: set[str] = set()
    for line in match.group("body").splitlines():
        candidate = line.strip().split("(", 1)[0].split("{", 1)[0].rstrip(",")
        if candidate and not candidate.startswith("//"):
            variants.add(candidate)
    return variants


def check_surfaces() -> None:
    syscall = (KERNEL / "syscall" / "mod.rs").read_text(encoding="utf-8")
    actual_syscalls = set(re.findall(r"pub const (SYS_[A-Z0-9_]+):", syscall))
    if actual_syscalls != ALLOWED_SYSCALLS:
        fail(f"kernel syscall surface changed: {sorted(actual_syscalls)}")

    capability = (KERNEL / "capability" / "mod.rs").read_text(encoding="utf-8")
    actual_objects = enum_variants(capability, "KernelObject")
    if actual_objects != ALLOWED_KERNEL_OBJECTS:
        fail(f"kernel object surface changed: {sorted(actual_objects)}")
    actual_rights = set(re.findall(r"pub const (RIGHT_[A-Z0-9_]+):", capability))
    if actual_rights != ALLOWED_RIGHTS:
        fail(f"capability rights surface changed: {sorted(actual_rights)}")


def grant_block(text: str, name: str) -> str:
    match = re.search(
        rf'\{{\s*name = "{re.escape(name)}";(?P<body>.*?)\n\s*\}};',
        text,
        re.DOTALL,
    )
    if match is None:
        fail(f"missing generation grant {name}")
    return match.group("body")


def check_explicit_grants() -> None:
    manifest = MANIFEST.read_text(encoding="utf-8")
    normal = grant_block(manifest, "block-read")
    if 'target = "storage-probe";' not in normal or 'rights = ["read";];' not in normal:
        fail("normal storage probe no longer has exactly read authority")

    for name, target in [
        ("block-write-check", "storage-writer"),
        ("block-fault-check", "storage-fault-probe"),
        ("store-access", "storage-store-probe"),
    ]:
        block = grant_block(manifest, name)
        if f'target = "{target}";' not in block or 'rights = ["read"; "write";];' not in block:
            fail(f"{name} is not an explicit test-component write grant")
        if "transferable = false;" not in block:
            fail(f"{name} became transferable")


def check_framework_path() -> None:
    bootstrap = (KERNEL / "bootstrap.rs").read_text(encoding="utf-8")
    if "generation.number" not in bootstrap or "storage_fault_probe" not in bootstrap:
        fail("storage test selection is no longer manifest-driven")
    justfile = JUSTFILE.read_text(encoding="utf-8")
    framework = re.search(
        r"framework_usb_image[^\n]*: framework_safety_check\n(?P<body>(?:    .*\n)+)",
        justfile,
    )
    if framework is None:
        fail("cannot locate Framework image recipe")
    body = framework.group("body")
    if "SLIME_GENERATION_NUMBER" in body or "virtio-blk" in body:
        fail("Framework image recipe enables disposable-QEMU storage writes")
    nvme = (KERNEL / "nvme.rs").read_text(encoding="utf-8")
    block_device = (KERNEL / "block_device.rs").read_text(encoding="utf-8")
    if "NVM_WRITE" in nvme or "pub fn write_sector" not in nvme or "NvmeError::ReadOnly" not in nvme:
        fail("Framework NVMe backend is not structurally read-only")
    nvme_write_arm = re.search(
        r"Self::Nvme\(device\)\s*=>\s*device\.write_sector",
        block_device,
    )
    if nvme_write_arm is None:
        fail("common block service no longer delegates NVMe write rejection")


def main() -> None:
    check_surfaces()
    check_explicit_grants()
    check_framework_path()
    print("Framework storage authority allowlist check: ok")


if __name__ == "__main__":
    main()
