#!/usr/bin/env python3

"""Run a Slime OS verification gate and report its typed observations.

This is the adapter `.devloop/policy.json` names, not a checker: it owns no
invariant of its own. A gate identity resolves here to existing `just` targets
and the shared store reader, and the result is reported as the typed
observations the policy declares. Requirement text never names a command, and
nothing here decides whether an observation satisfies an acceptance — devloop's
helpers do that against the item's own predicates.

It reads devloop's gate request on stdin (`item`, `acceptance`, `execution`,
`inputs`) and writes one JSON list of observations on stdout. A gate that ran
exits zero even when what it observed is bad news, so a failing check is
reported as a false observation with an attributable predicate rather than as a
gate that could not run.

`inputs` is a path devloop resolves, not decoded data, so a gate that needs
execution inputs reads that file itself. devloop binds its digest into the
evidence identity, which is what lets `just-target` take a target name from
inputs without loosening what the evidence is bound to: naming a different
target produces a different identity, and the earlier evidence no longer
applies.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT  # noqa: E402
import work_items  # noqa: E402

# A terminal record that cannot be read as a finished item. The gate proves the
# store reader refuses it rather than counting it as done.
CORRUPT = {
    "schema": "terminal-item/v1",
    "metadata": "---\nschema: work-item/v2\nid: 00000000-0000-7000-8000-00000000000b\n"
    "kind: task\nstate: open\ncreated: 2026-09-15T10:00:00Z\n---\n\n# Corrupt control\n",
}


class CannotRun(Exception):
    """The gate could not run at all, which is not evidence about the repository.

    Distinct from a check that ran and failed: that is reported as a false
    observation with exit zero, so devloop records it against the acceptance.
    """


def observation(identity: str, kind: str, *, integer: int = 0, boolean: bool = False) -> dict:
    return {
        "id": identity,
        "value": {"kind": kind, "intValue": integer, "boolValue": boolean, "textValue": ""},
    }


def recipe(target: str) -> tuple[bool, str]:
    finished = subprocess.run(
        ["just", target], cwd=ROOT, capture_output=True, text=True
    )
    return finished.returncode == 0, finished.stdout + finished.stderr


def declared_targets() -> set[str]:
    """Every recipe `just` currently publishes in this repository."""
    finished = subprocess.run(
        ["just", "--summary"], cwd=ROOT, capture_output=True, text=True
    )
    if finished.returncode:
        raise CannotRun(f"cannot list just targets: {finished.stderr.strip()}")
    return set(finished.stdout.split())


def just_target(request: dict) -> list[dict]:
    """Run one named `just` target and report whether it passed.

    The target is named in the execution inputs rather than in policy, because
    declaring typed observations for every check in this repository ahead of
    time is not practical and would block the policy behind adapters nobody
    needs yet. What policy keeps is the shape of the answer: one boolean.

    The name is checked against what `just` actually publishes before anything
    runs. This adapter executes what it is handed, so an unknown or malformed
    name is refused as a gate that could not run (exit 2) rather than recorded
    as a check that failed — the two are different claims, and only the second
    is evidence about the repository.
    """
    location = request.get("inputs")
    if not isinstance(location, str) or not location:
        raise CannotRun("the request carries no execution inputs path")
    try:
        inputs = json.loads(Path(location).read_text())
    except (OSError, ValueError) as error:
        raise CannotRun(f"cannot read execution inputs: {error}") from error
    if not isinstance(inputs, dict):
        raise CannotRun("execution inputs are not a JSON object")
    target = inputs.get("justTarget")
    if not isinstance(target, str) or not target:
        raise CannotRun(
            "execution inputs declare no `justTarget` string; "
            'name the recipe to run, for example {"justTarget": "sel4_qos_check"}'
        )
    if target not in declared_targets():
        raise CannotRun(
            f"{target!r} is not a recipe `just` publishes; gates run declared targets only"
        )
    passed, output = recipe(target)
    sys.stderr.write(f"devloop gate just-target ran `just {target}`: ")
    sys.stderr.write("passed\n" if passed else f"failed\n{output}")
    return [observation("passed", "bool", boolean=passed)]


def work_item_store(request: dict) -> list[dict]:
    tasks_passed, tasks_output = recipe("tasks_check")
    docs_passed, _ = recipe("docs_check")

    # Retired identities must resolve from the terminal record alone, offline.
    retired = work_items.retired()
    resolved = all(identity in work_items.identities() for identity in retired)
    if retired:
        resolved = resolved and not any(work_items.done(identity) is None for identity in retired)

    # And a record that does not decode must be refused, not believed.
    scratch = ROOT / "build" / "devloop-gate-terminal"
    scratch.mkdir(parents=True, exist_ok=True)
    fixture = scratch / "00000000-0000-7000-8000-00000000000b.json"
    fixture.write_text(json.dumps(CORRUPT))
    original = work_items.TERMINAL
    try:
        work_items.TERMINAL = scratch
        work_items.cache_clear()
        refused = bool(work_items.corrupt_terminal_records()) and not work_items.done(
            "00000000-0000-7000-8000-00000000000b"
        )
    finally:
        work_items.TERMINAL = original
        work_items.cache_clear()
        fixture.unlink(missing_ok=True)

    validated = 0
    for line in tasks_output.splitlines():
        if "validated through devloop" in line:
            validated = int(line.split(",")[1].split()[0])

    return [
        observation("tasksCheckPassed", "bool", boolean=tasks_passed),
        observation("docsCheckPassed", "bool", boolean=docs_passed),
        observation("retiredIdentitiesResolved", "bool", boolean=resolved),
        observation("corruptTerminalRecordRefused", "bool", boolean=refused),
        observation("specDrivenItemsValidated", "int", integer=validated),
    ]


GATES = {"work-item-store": work_item_store, "just-target": just_target}


def main() -> int:
    if len(sys.argv) != 2:
        sys.stderr.write("usage: devloop-gate.py <gate-id>\n")
        return 2
    identity = sys.argv[1]
    if identity not in GATES:
        sys.stderr.write(f"unknown gate {identity}: declare it in .devloop/policy.json\n")
        return 2
    # The request is read so a gate cannot be run outside devloop's contract,
    # and so the tested acceptance is visible in a transcript.
    request = json.load(sys.stdin)
    acceptance = request.get("acceptance", {}).get("id", "?")
    sys.stderr.write(f"devloop gate {identity} for acceptance {acceptance}\n")
    try:
        observations = GATES[identity](request)
    except CannotRun as error:
        sys.stderr.write(f"gate {identity}: {error}\n")
        return 2
    json.dump(observations, sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
