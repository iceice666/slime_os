#!/usr/bin/env python3
"""Enforce the surviving seL4 storage-write authority allowlist."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
COMPOSITIONS = ROOT / "contracts" / "generation-manifest" / "v1" / "compositions"
# Holders that may write storage, per product fixture.
#
# IO2 moved storage-write authority out of `grants` and into
# `blockRingAuthority`: a writer is no longer a `block` capability with a
# `target`, but a table row binding one `holder`, on one device, to one ring,
# with independent read/write bits. The `-idle` instances that used to appear
# here are gone because they genuinely lost the authority -- they now hold only
# an endpoint token -- so this allowlist is strictly narrower than the one it
# replaces, not merely reshaped.
EXPECTED_WRITERS = {
    "sel4-filesystem.zti": {"sel4-filesystem-service"},
    "sel4-generation.zti": {"sel4-generation-manager"},
    "sel4-io-block.zti": {"io-block-probe"},
    "sel4-recovery.zti": {"sel4-recovery-probe"},
    "sel4-rollback.zti": {"sel4-rollback-probe"},
    "sel4-storage.zti": {"sel4-storage-probe"},
    "sel4-store.zti": {"sel4-store-probe"},
    "sel4-transfer.zti": {"sel4-transfer-probe"},
}
ENTRY = re.compile(r"\{(?P<body>[^{}]*?)\};", re.DOTALL)
HOLDER = re.compile(r'holder\s*=\s*"([^"]+)"')
RIGHTS = re.compile(r"rights\s*=\s*\[(.*?)\];", re.DOTALL)
AUTHORITY_BLOCK = re.compile(r"blockRingAuthority\s*=\s*\[(?P<body>.*?)\];\s*\n", re.DOTALL)


def fail(message: str) -> None:
    raise SystemExit(f"framework authority check: {message}")


def writers(path: Path) -> list[str]:
    """Every holder granted `blockWrite` by this fixture's ring-authority table.

    Scoped to the `blockRingAuthority` table rather than the whole file on
    purpose: `blockRead`-only rows, IO1 budgets, and notification bindings all
    carry a `holder`, and matching those would report authority nobody has.
    """
    text = path.read_text(encoding="utf-8")
    table = AUTHORITY_BLOCK.search(text)
    if table is None:
        return []
    result = []
    for match in ENTRY.finditer(table.group("body")):
        body = match.group("body")
        rights = RIGHTS.search(body)
        if rights is None or '"blockWrite"' not in rights.group(1):
            continue
        holder = HOLDER.search(body)
        if holder is None:
            fail(f"{path.relative_to(ROOT)} has blockWrite without a holder")
        result.append(holder.group(1))
    return result


def assert_no_grant_form_block_write() -> None:
    """Refuse a silent return to the pre-IO2 representation.

    Without this, moving one writer back into a `block` capability grant would
    make it invisible to `writers` above and the gate would pass while the
    authority it guards had escaped the table.
    """
    for path in sorted(COMPOSITIONS.glob("sel4*.zti")):
        text = path.read_text(encoding="utf-8")
        for match in re.finditer(r"\{(?P<body>.*?)\};", text, re.DOTALL):
            body = match.group("body")
            if "blockWrite" in body and "capabilityKind" in body:
                fail(
                    f"{path.relative_to(ROOT)} grants blockWrite as a capability; "
                    "storage-write authority belongs in blockRingAuthority"
                )


def main() -> None:
    assert_no_grant_form_block_write()
    actual: dict[str, set[str]] = {}
    for path in sorted(COMPOSITIONS.glob("sel4*.zti")):
        holders = set(writers(path))
        if holders:
            actual[path.name] = holders
    if actual != EXPECTED_WRITERS:
        missing = {name: holders for name, holders in EXPECTED_WRITERS.items() if actual.get(name) != holders}
        added = {name: holders for name, holders in actual.items() if EXPECTED_WRITERS.get(name) != holders}
        fail(f"storage-write authority drift; missing={missing}, added={added}")
    print(f"framework authority check: {len(actual)} product fixtures grant blockWrite only to approved service owners")


if __name__ == "__main__":
    main()
