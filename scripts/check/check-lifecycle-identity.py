#!/usr/bin/env python3
"""B42: no wire record or public runtime type exposes a bare task id.

Lifecycle authority is a capability. A numeric task id is not authority — it
is a name anyone can forge by counting — so sending one across a process
boundary and accepting it back as a wait handle makes the identity ambient.
The spawn protocol now carries the supervision capability instead, and this
refuses the shape that would bring the numeric identity back.

Name-based on purpose. A field called `task_id`, `taskId`, or `task_handle` in
a schema or a public runtime struct is the reintroduction this guards against;
a local variable holding a root-side `TaskId` is not, and the root's own
in-memory model is explicitly out of scope. What crosses a boundary is what
matters.
"""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

# Schemas define what crosses a persistence or process boundary; the generated
# Rust and the runtime's public surface are the same records seen from Rust.
SEARCHED = (
    ("contracts", "**/schema.zt"),
    ("components/proto/src", "*.rs"),
    ("components/runtime/src", "**/*.rs"),
)

# `task_id`, `taskId`, `task_handle`, and the same shapes under `child`/`thread`.
FORBIDDEN = re.compile(
    r"\b(?:task|child|thread)_?(?:id|handle)\b",
    re.IGNORECASE,
)

# A declaration, not a mention: `name : Int;` in Zutai, `pub name: T` in Rust,
# or a packed field descriptor. A comment explaining the ban is not a breach.
DECLARATION = re.compile(
    r"""(?x)
    (?: ^\s* (?P<zt>[A-Za-z_][A-Za-z0-9_]*) \s* : \s* \w )
  | (?: ^\s* pub \s+ (?P<rs>[A-Za-z_][A-Za-z0-9_]*) \s* : )
  | (?: name \s* = \s* "(?P<packed>[A-Za-z_][A-Za-z0-9_]*)" )
    """
)


def fail(message: str) -> None:
    raise SystemExit(f"lifecycle identity check: {message}")


def main() -> int:
    if Path.cwd().resolve() != ROOT:
        fail(f"run from repository root: {ROOT}")
    breaches: list[str] = []
    scanned = 0
    for directory, pattern in SEARCHED:
        base = ROOT / directory
        if not base.is_dir():
            fail(f"missing search root {directory}")
        for path in sorted(base.glob(pattern)):
            scanned += 1
            for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
                declaration = DECLARATION.search(line)
                if declaration is None:
                    continue
                named = (
                    declaration.group("zt")
                    or declaration.group("rs")
                    or declaration.group("packed")
                    or ""
                )
                if FORBIDDEN.fullmatch(named):
                    rel = path.relative_to(ROOT)
                    breaches.append(f"{rel}:{number}: {named}")
    if not scanned:
        fail("no files scanned; the search roots are wrong")
    if breaches:
        joined = "\n  ".join(breaches)
        fail(
            "a task-id-shaped lifecycle field is declared where a capability "
            f"belongs (B42):\n  {joined}"
        )
    print(f"lifecycle identity check: {scanned} files carry no bare task id")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
