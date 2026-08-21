#!/usr/bin/env python3
"""Reject architecture-specific mechanism in surviving neutral Rust trees."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
NEUTRAL_ROOTS = (
    ROOT / "components" / "runtime" / "src",
    # CP3: component sources are per-crate; the shared helpers moved to
    # `components/lib`. Both roots are listed so the scan still reaches
    # every component source rather than one crate's.
    ROOT / "components" / "bins",
    ROOT / "components" / "lib" / "src",
    ROOT / "slime-root" / "src",
)
FORBIDDEN = re.compile(
    r"(?i)(?:\bx86_64\b|\bx86-64\b|\bR_X86_64\w*\b|qemu-system-x86_64|"
    r"\b(?:rax|rbx|rcx|rdx|cr[0-8]|efer)\b|\b(?:invlpg|wrmsr|rdmsr)\b|0x8664)"
)


def fail(message: str) -> None:
    raise SystemExit(f"architecture portability check: {message}")


def main() -> None:
    checked = 0
    violations: list[str] = []
    for root in NEUTRAL_ROOTS:
        if not root.is_dir():
            fail(f"missing neutral source tree {root.relative_to(ROOT)}")
        for path in sorted(root.rglob("*.rs")):
            checked += 1
            for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                if FORBIDDEN.search(line):
                    violations.append(f"{path.relative_to(ROOT)}:{line_number}: {line.strip()}")
    if checked == 0:
        fail("neutral source set is empty")
    if violations:
        fail("architecture-specific token in neutral source:\n" + "\n".join(violations))
    print(f"architecture portability check: {checked} neutral Rust files contain no x86-only mechanism")


if __name__ == "__main__":
    main()
