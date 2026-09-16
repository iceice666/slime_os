#!/usr/bin/env python3

"""Decide whether this repository's rules allow devloop to proceed.

devloop asks before it starts work and before it completes it; the answer is
Slime OS policy, so it lives here rather than in devloop. Approval is not
evidence: it says the repository's own preconditions hold, never that an
acceptance was observed.

Two standing rules apply, both from `AGENTS.md` and `CONTRIBUTING.md`:

* **Planning lands first.** The item must already exist on canonical `main`, so
  implementation runs against a landed canonical UUID rather than one invented
  in the branch. Read from `origin/main` in the local object store; no network.
* **Backlog first.** An open or active backlog defect blocks new track work
  while a milestone is active, exactly as `just tasks_check` enforces it.

Reads devloop's request on stdin and writes `{"approved": bool}`. Refusals are
explained on stderr, because a bare `false` is not actionable.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT  # noqa: E402
import work_items  # noqa: E402

CANONICAL = "origin/main"


def landed(identity: str) -> bool:
    """Whether canonical `main` already carries this identity."""
    for path in (f".tasks/items/{identity}.md", f".tasks/terminal/{identity}.json"):
        finished = subprocess.run(
            ["git", "cat-file", "-e", f"{CANONICAL}:{path}"],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )
        if finished.returncode == 0:
            return True
    return False


def main() -> int:
    request = json.load(sys.stdin)
    item = request.get("item", {})
    identity = item.get("id", "")
    reasons: list[str] = []

    if not identity:
        reasons.append("the request names no canonical UUID")
    elif not landed(identity):
        reasons.append(
            f"{identity} is not on {CANONICAL}: land the work-item-only planning change "
            "before implementing it"
        )

    blocking = work_items.open_backlog()
    active_milestones = [
        entry
        for entry in work_items.items()
        if entry["kind"] == "milestone" and entry["state"] == "active"
    ]
    if blocking and active_milestones:
        names = ", ".join(str(entry["key"] or entry["id"]) for entry in blocking)
        reasons.append(f"backlog-first: resolve, defer, or block {names} before track work")

    for reason in reasons:
        sys.stderr.write(f"devloop approval refused: {reason}\n")
    json.dump({"approved": not reasons}, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
