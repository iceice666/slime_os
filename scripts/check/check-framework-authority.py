#!/usr/bin/env python3
"""Enforce the surviving seL4 storage-write authority allowlist."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FIXTURES = ROOT / "contracts" / "generation" / "v1" / "fixtures"
EXPECTED_WRITERS = {
    "sel4-filesystem.zti": "sel4-filesystem-service",
    "sel4-generation.zti": "sel4-generation-manager",
    "sel4-recovery.zti": "sel4-recovery-probe",
    "sel4-rollback.zti": "sel4-rollback-probe",
    "sel4-storage.zti": "sel4-storage-probe",
    "sel4-store.zti": "sel4-store-probe",
    "sel4-transfer.zti": "sel4-transfer-probe",
}
GRANT = re.compile(r"\{(?P<body>.*?)\};", re.DOTALL)
TARGET = re.compile(r'target\s*=\s*"([^"]+)"')
RIGHTS = re.compile(r"rights\s*=\s*\[(.*?)\];", re.DOTALL)


def fail(message: str) -> None:
    raise SystemExit(f"framework authority check: {message}")


def writers(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    result = []
    for match in GRANT.finditer(text):
        body = match.group("body")
        rights = RIGHTS.search(body)
        if rights is None or '"blockWrite"' not in rights.group(1):
            continue
        target = TARGET.search(body)
        if target is None:
            fail(f"{path.relative_to(ROOT)} has blockWrite without a target")
        result.append(target.group(1))
    return result


def main() -> None:
    actual: dict[str, str] = {}
    for path in sorted(FIXTURES.glob("sel4*.zti")):
        holders = writers(path)
        if len(holders) > 1:
            fail(f"{path.relative_to(ROOT)} grants blockWrite to multiple holders: {holders}")
        if holders:
            actual[path.name] = holders[0]
    if actual != EXPECTED_WRITERS:
        missing = sorted(set(EXPECTED_WRITERS.items()) - set(actual.items()))
        added = sorted(set(actual.items()) - set(EXPECTED_WRITERS.items()))
        fail(f"storage-write authority drift; missing={missing}, added={added}")
    print(f"framework authority check: {len(actual)} product fixtures grant blockWrite only to approved service owners")


if __name__ == "__main__":
    main()
