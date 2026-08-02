#!/usr/bin/env python3
# Storage-authority allowlist for the Framework-safe image.
#
# M5.3 introduces one explicit block-write right for disposable QEMU storage
# checks; M5.4 adds one explicit object-store right for the disposable QEMU
# store probe. M5.7 adds a common read-only NVMe backend. This checker proves
# that authority is neither ambient nor granted to the normal storage probe,
# and that the Framework boot path cannot enable test-only writes.

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re

from harness import ROOT

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
    "SYS_HEALTH_CONFIRM",
    "SYS_UNHEALTHY",
    "SYS_RECOVERY_RECONSTRUCT",
    "SYS_ENDPOINT_CREATE",
    "SYS_SUPERVISION_STATUS",
    "SYS_CAP_DROP",
    "SYS_DIRECTORY_INSPECT",
    "SYS_DIRECTORY_DERIVE",
    "SYS_DIRECTORY_COMMIT",
    "SYS_INPUT_READ",
    "SYS_GENERATION_TRANSACT",
    "SYS_GENERATION_RECEIVE",
    "SYS_WAIT",
    "SYS_SHARED_BUFFER_CREATE",
    "SYS_SHARED_BUFFER_RELEASE",
    "SYS_SHARED_BUFFER_MAP",
    "SYS_SHARED_BUFFER_UNMAP",
    "SYS_SHARED_BUFFER_SEAL",
    "SYS_SHARED_BUFFER_LOAN",
    "SYS_SHARED_BUFFER_LOAN_MAP",
    "SYS_SHARED_BUFFER_RETURN",
    "SYS_SHARED_BUFFER_REVOKE",
    "SYS_CAP_TRANSFER",
}
ALLOWED_KERNEL_OBJECTS = {
    "Endpoint",
    "EndpointFactory",
    "SharedBufferFactory",
    "Input",
    "Executable",
    "Supervision",
    "PciFunction",
    "DmaMemory",
    "Irq",
    "SharedBuffer",
    "SharedBufferLoan",
    "BlockDevice",
    "ObjectStore",
    "Directory",
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
    "RIGHT_BUFFER_MAP",
    "RIGHT_BLOCK_READ",
    "RIGHT_BLOCK_WRITE",
    "RIGHT_STORE_READ",
    "RIGHT_STORE_WRITE",
    "RIGHT_HEALTH_CONFIRM",
    "RIGHT_BOOT_UPDATE",
    "RIGHT_SPAWN",
    "RIGHT_ENDPOINT_CREATE",
    "RIGHT_SUPERVISE",
    "RIGHT_DIRECTORY_READ",
    "RIGHT_DIRECTORY_WRITE",
    "RIGHT_DIRECTORY_LIST",
    "RIGHT_DIRECTORY_DERIVE",
    "RIGHT_INPUT_READ",
    "RIGHT_BUFFER_CREATE",
    "RIGHT_BUFFER_LOAN",
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
    return set(
        re.findall(
            r"^    ([A-Za-z][A-Za-z0-9_]*)\s*(?:\([^\n]*\)|\{|,)",
            match.group("body"),
            re.MULTILINE,
        )
    )


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
    if 'target = "storage-probe";' not in normal or 'rights = ["blockRead";];' not in normal:
        fail("normal storage probe no longer has exactly read authority")

    for name, target in [
        ("block-write-check", "storage-writer"),
        ("block-fault-check", "storage-fault-probe"),
        ("store-access", "storage-store-probe"),
    ]:
        block = grant_block(manifest, name)
        expected = '["storeRead"; "storeWrite";]' if name == "store-access" else '["blockRead"; "blockWrite";]'
        if f'target = "{target}";' not in block or f"rights = {expected};" not in block:
            fail(f"{name} is not an explicit test-component write grant")
        if "transferable = false;" not in block:
            fail(f"{name} became transferable")


def check_framework_path() -> None:
    # Which storage component a profile runs is generation data, not a kernel
    # source decision. B10 replaced the `generation.number` comparison that used
    # to pick it with a boot-layout lookup over the candidate set, and B11 made
    # the candidates themselves profile-declared -- so the invariant to assert is
    # that the kernel still offers the set and lets the layout choose, rather
    # than naming one probe in source.
    #
    # This previously required the strings `generation.number` and
    # `storage_fault_probe` to appear in `bootstrap.rs`. Both were proxies for
    # the old mechanism and neither survived B10, which left the check passing
    # only until something else removed them.
    bootstrap = (KERNEL / "runtime" / "bootstrap.rs").read_text(encoding="utf-8")
    if "STORAGE_COMPONENTS" not in bootstrap or "one_of(" not in bootstrap:
        fail("storage component selection is no longer layout-driven")
    for probe in ("storage-probe", "storage-writer", "storage-fault-probe", "storage-store-probe"):
        if f'"{probe}"' not in bootstrap:
            fail(f"{probe} is no longer an offered storage candidate")
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
    nvme = (KERNEL / "drivers" / "nvme.rs").read_text(encoding="utf-8")
    block_device = (KERNEL / "storage" / "block_device.rs").read_text(encoding="utf-8")
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
