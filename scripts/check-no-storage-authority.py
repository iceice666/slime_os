#!/usr/bin/env python3

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
KERNEL = ROOT / "kernel" / "src"

FORBIDDEN = re.compile(
    r"\b(?:nvme|ahci|ata|virtio[_-]?blk|block[_-]?device|storage[_-]?device|disk[_-]?write)\b",
    re.IGNORECASE,
)
ALLOWED_FILES = {
    KERNEL / "main.rs",  # Runtime diagnostic states that the authority is absent.
}
EXPECTED_SYSCALLS = {
    "SYS_YIELD",
    "SYS_SEND",
    "SYS_RECV",
    "SYS_EXIT",
    "SYS_SPAWN",
    "SYS_DEBUG_WRITE",
}
EXPECTED_KERNEL_OBJECTS = {"Endpoint", "Executable"}


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
        fail("storage authority token found in kernel surface:\n" + "\n".join(findings))


def check_syscall_surface() -> None:
    text = (KERNEL / "syscall" / "mod.rs").read_text(encoding="utf-8")
    actual = set(re.findall(r"pub const (SYS_[A-Z0-9_]+):", text))
    if actual != EXPECTED_SYSCALLS:
        fail(f"kernel syscall surface changed: expected {sorted(EXPECTED_SYSCALLS)}, got {sorted(actual)}")


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
    if actual != EXPECTED_KERNEL_OBJECTS:
        fail(
            f"kernel object surface changed: expected {sorted(EXPECTED_KERNEL_OBJECTS)}, got {sorted(actual)}"
        )


def main() -> None:
    check_forbidden_tokens()
    check_syscall_surface()
    check_capability_surface()
    print("kernel storage authority check: ok")


if __name__ == "__main__":
    main()
