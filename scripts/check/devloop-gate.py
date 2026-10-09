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

devloop requires every acceptance's evidence to carry the identity it
completes under, inputs digest included, so the acceptances of one item that
share a qualification recipe each gate that same recipe. `just-target` and
`just-observations` run it once per execution identity and answer the later
acceptances from that run; see `run_once` for the bounds on reuse.

`just-target` reports one boolean: the recipe's exit. `just-observations`
additionally reports what the recipe's checker recorded through
`scripts/lib/devloop_observations.py`, typed as policy declares, so an
acceptance's predicate can name an expected count rather than a pass.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

from harness import ROOT  # noqa: E402
import devloop_diagnostics  # noqa: E402
import devloop_observations  # noqa: E402
import observation_capacity  # noqa: E402
import work_items  # noqa: E402

# A terminal record that cannot be read as a finished item. The gate proves the
# store reader refuses it rather than counting it as done.
CORRUPT = {
    "schema": "terminal-item/v1",
    "metadata": "---\nschema: work-item/v2\nid: 00000000-0000-7000-8000-00000000000b\n"
    "kind: task\nstate: open\ncreated: 2026-09-15T10:00:00Z\n---\n\n# Corrupt control\n",
}


# Completed `just-target` runs, one file per execution identity. Build output:
# a run is a local observation, never a record another checkout can inherit.
RUNS = ROOT / "build" / "devloop-gate-runs"
POLICY = ROOT / ".devloop" / "policy.json"

# How long after a run finishes it may answer another acceptance. devloop stamps
# the evidence it records at the moment this adapter returns, so a reused run
# is reported up to this much fresher than it was observed; the window is sized
# for acceptances gated one after another, not for resuming later.
REUSE_SECONDS = 3600

# The execution-identity fields a run is bound to. `now` and the recorded
# evidence change between acceptances without changing what was tested.
IDENTITY_FIELDS = ("requirements", "helper", "code", "policy", "inputs", "target", "image", "epoch")


class CannotRun(Exception):
    """The gate could not run at all, which is not evidence about the repository.

    Distinct from a check that ran and failed: that is reported as a false
    observation with exit zero, so devloop records it against the acceptance.
    """


def observation(
    identity: str, kind: str, *, integer: int = 0, boolean: bool = False, text: str = ""
) -> dict:
    return {
        "id": identity,
        "value": {"kind": kind, "intValue": integer, "boolValue": boolean, "textValue": text},
    }


def recipe(target: str, environment: dict[str, str] | None = None) -> tuple[bool, str]:
    return devloop_diagnostics.capture(target, environment)


def declared_targets() -> set[str]:
    """Every recipe `just` currently publishes in this repository."""
    finished = subprocess.run(["just", "--summary"], cwd=ROOT, capture_output=True, text=True)
    if finished.returncode:
        raise CannotRun(f"cannot list just targets: {finished.stderr.strip()}")
    return set(finished.stdout.split())


def code_fingerprint() -> str:
    """Detect a change to the policy's code closure while a recipe runs.

    Not devloop's code identity, and never compared with it: devloop refuses a
    gate whose closure changed, but only after this adapter has returned, so a
    run must not be kept for reuse on devloop's later verdict alone.
    """
    try:
        paths = json.loads(POLICY.read_text())["codePaths"]
    except (OSError, ValueError, KeyError) as error:
        raise CannotRun(f"cannot read the policy code paths: {error}") from error
    digest = hashlib.sha256()
    for name in paths:
        path = ROOT / name
        for file in [path] if path.is_file() else sorted(path.rglob("*")):
            if file.is_file() and not file.is_symlink():
                digest.update(file.relative_to(ROOT).as_posix().encode() + b"\0")
                digest.update(str(file.stat().st_mode & 0o111).encode() + b"\0")
                digest.update(hashlib.sha256(file.read_bytes()).digest())
    return digest.hexdigest()


def run_key(request: dict, gate: str, target: str) -> str:
    """The execution identity a run answers for, as a stable file name.

    The gate is part of it: a `just-target` run of a recipe carries no
    observations report, so it cannot answer a `just-observations` acceptance
    for the same recipe.
    """
    execution = request.get("execution")
    identity = execution.get("identity") if isinstance(execution, dict) else None
    if not isinstance(identity, dict) or not all(
        isinstance(identity.get(field), str) and identity[field] for field in IDENTITY_FIELDS
    ):
        raise CannotRun("the request carries no complete execution identity to bind a run to")
    bound = {field: identity[field] for field in IDENTITY_FIELDS}
    bound |= {"gate": gate, "justTarget": target}
    return hashlib.sha256(json.dumps(bound, sort_keys=True).encode()).hexdigest()


def reusable(key: str, now: float) -> dict | None:
    """The recorded run for `key`, or None when there is no usable run."""
    try:
        run = json.loads((RUNS / f"{key}.json").read_text())
    except (OSError, ValueError):
        return None
    if not isinstance(run, dict) or run.get("key") != key:
        return None
    finished, passed = run.get("finishedAt"), run.get("passed")
    if type(finished) not in (int, float) or type(passed) is not bool:
        return None
    if not 0 <= now - finished <= REUSE_SECONDS:
        return None
    return run


def named_target(request: dict) -> str:
    """The recipe the execution inputs name, checked against what `just` publishes.

    This adapter executes what it is handed, so an unknown or malformed name is
    refused as a gate that could not run (exit 2) rather than recorded as a
    check that failed — the two are different claims, and only the second is
    evidence about the repository.
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
    return target


def run_once(
    request: dict, gate: str, target: str, *, environment: dict[str, str] | None = None
) -> tuple[dict, str | None]:
    """Run `target` under this execution identity, or answer from a kept run.

    A run answers every acceptance gated under the same complete execution
    identity within `REUSE_SECONDS` of finishing, failing runs included, so a
    repeat request cannot be used to retry a failure. A run is kept only when
    the code closure was unchanged from its start to its end. Delete its file
    under `RUNS` to force the recipe to run again.

    Returns the run record and the recipe output, which is None for a reused
    run. The record's `report` field holds whatever `observations_report` read
    for a gate that collects one; `just-target` leaves it absent.
    """
    key = run_key(request, gate, target)
    if gate == "just-observations":
        try:
            observation_capacity.replay(key, request, target)
        except observation_capacity.Refused as error:
            raise CannotRun(str(error)) from error
        except (OSError, ValueError) as error:
            message = f"diagnostic capacity retention error for run {key}: {error}"
            try:
                devloop_diagnostics.notify(message)
            except (OSError, ValueError):
                pass
            raise CannotRun(message) from error
    previous = reusable(key, time.time())
    if previous is not None:
        sys.stderr.write(
            f"devloop gate {gate} reused the `just {target}` run {key[:12]} "
            f"under this execution identity: {'passed' if previous['passed'] else 'failed'}\n"
        )
        if previous.get("diagnosticReceipt"):
            try:
                devloop_diagnostics.announce(previous["diagnosticReceipt"], key, request, target)
            except (OSError, ValueError) as error:
                message = f"diagnostic retention error for run {key}: {error}"
                try:
                    devloop_diagnostics.notify(message)
                except (OSError, ValueError):
                    pass
                raise CannotRun(message) from error
        return previous, None
    before = code_fingerprint()
    try:
        result = recipe(target, environment)
        passed, output = result
        record = {"key": key, "justTarget": target, "passed": passed, "finishedAt": time.time()}
        if isinstance(result, devloop_diagnostics.Capture):
            receipt = devloop_diagnostics.retain(key, request, target, result)
            record["diagnosticReceipt"] = receipt
            if gate == "just-observations" and result.transcript_digest is None:
                observation_capacity.retain(key, request, target, result, receipt)
            devloop_diagnostics.announce(receipt, key, request, target)
    except observation_capacity.Refused as error:
        raise CannotRun(str(error)) from error
    except (OSError, ValueError) as error:
        message = f"diagnostic capture error for run {key}: {error}"
        try:
            devloop_diagnostics.notify(message)
        except (OSError, ValueError):
            pass
        raise CannotRun(message) from error
    if environment is not None:
        if isinstance(result, devloop_diagnostics.Capture) and result.transcript_digest is None:
            message = f"diagnostic transcript error for run {key}: stderr exceeds bounded digest buffer; raw capture retained"
            devloop_diagnostics.notify(message)
            raise CannotRun(message)
        record["report"] = observations_report(target, passed)
        record["transcriptDigest"] = (
            result.transcript_digest
            if isinstance(result, devloop_diagnostics.Capture)
            else hashlib.sha256(output.encode()).hexdigest()
        )
    if code_fingerprint() == before:
        RUNS.mkdir(parents=True, exist_ok=True)
        partial = RUNS / f"{key}.json.partial"
        partial.write_text(json.dumps(record) + "\n")
        partial.replace(RUNS / f"{key}.json")
    sys.stderr.write(f"devloop gate {gate} ran `just {target}`: ")
    sys.stderr.write("passed\n" if passed else f"failed\n{output}")
    return record, output


def just_target(request: dict) -> list[dict]:
    """Run one named `just` target and report whether it passed.

    The target is named in the execution inputs rather than in policy, because
    declaring typed observations for every check in this repository ahead of
    time is not practical and would block the policy behind adapters nobody
    needs yet. What policy keeps is the shape of the answer: one boolean.
    """
    target = named_target(request)
    record, _ = run_once(request, "just-target", target)
    return [observation("passed", "bool", boolean=record["passed"])]


def declared_observations(gate: str) -> dict[str, str]:
    """The observation ids and kinds policy declares for `gate`."""
    try:
        gates = json.loads(POLICY.read_text())["gates"]
        declared = next(g for g in gates if g["id"] == gate)["observations"]
        return {d["id"]: d["kind"] for d in declared}
    except (OSError, ValueError, KeyError, StopIteration, TypeError) as error:
        raise CannotRun(f"cannot read the policy declarations for gate {gate}: {error}") from error


def clear_observations_report(target: str) -> None:
    """Remove a stale report so only this run's checker can produce one."""
    try:
        devloop_observations.report_path(target).unlink(missing_ok=True)
    except (OSError, ValueError) as error:
        raise CannotRun(f"cannot clear the observations report for {target}: {error}") from error


def observations_report(target: str, passed: bool) -> dict[str, object] | None:
    """Read what the recipe's checker recorded, if it recorded anything.

    A passing recipe that recorded nothing is a harness fault — the recipe is
    not wired to report — and is refused as a gate that could not run. A
    failing recipe may legitimately stop before it reports; its `passed` is
    false and every predicate over a missing observation fails with it.
    """
    try:
        raw = devloop_observations.report_path(target).read_text()
    except FileNotFoundError:
        if passed:
            raise CannotRun(
                f"`just {target}` passed but recorded no observations; its checker must "
                "call devloop_observations.record(...) with what it observed"
            ) from None
        return None
    except (OSError, ValueError) as error:
        raise CannotRun(f"cannot read the observations report for {target}: {error}") from error
    try:
        report = json.loads(raw)
    except ValueError as error:
        raise CannotRun(f"the observations report for {target} is not JSON: {error}") from error
    if not isinstance(report, dict):
        raise CannotRun(f"the observations report for {target} is not a JSON object")
    return report


def typed_observations(
    gate: str, report: dict[str, object] | None, reserved: dict[str, object]
) -> list[dict]:
    """Encode a report as the typed observations policy declares for `gate`.

    `reserved` are observations the adapter itself owns; a checker that reports
    one of them is refused, because a checker cannot vouch for its own exit
    code or transcript. Every reported id must be declared with the matching
    kind: an unknown or mistyped observation is an unresolved gate, not a
    failed check.
    """
    declared = declared_observations(gate)
    observations: list[dict] = []
    for identifier, value in sorted((report or {}).items()):
        if identifier in reserved:
            raise CannotRun(f"observation {identifier!r} is recorded by the gate, not the checker")
        kind = declared.get(identifier)
        if kind is None:
            raise CannotRun(
                f"observation {identifier!r} is not declared for gate {gate} in "
                f"{POLICY.relative_to(ROOT)}; declare it there before a checker reports it"
            )
        if kind == "bool" and type(value) is bool:
            observations.append(observation(identifier, "bool", boolean=value))
        elif kind == "int" and type(value) is int:
            observations.append(observation(identifier, "int", integer=value))
        elif kind == "text" and isinstance(value, str):
            observations.append(observation(identifier, "text", text=value))
        else:
            raise CannotRun(
                f"observation {identifier!r} is declared {kind} but the checker reported "
                f"{type(value).__name__}"
            )
    for identifier, value in reserved.items():
        kind = declared.get(identifier)
        if kind == "bool":
            observations.append(observation(identifier, "bool", boolean=bool(value)))
        elif kind == "text":
            observations.append(observation(identifier, "text", text=str(value)))
        else:
            raise CannotRun(
                f"gate {gate} must declare {identifier!r} in {POLICY.relative_to(ROOT)}"
            )
    return observations


def just_observations(request: dict) -> list[dict]:
    """Run one named `just` target and report what its checker observed.

    Like `just-target`, but the recipe's checker reports typed observations
    through `scripts/lib/devloop_observations.py` — counts of cases observed,
    negative controls refused, bytes delivered, handles rejected — and an
    acceptance's predicate reads the one it needs. That is what lets a
    planning item state an expected number rather than "the recipe passed":
    an implementation that satisfies the predicate has to produce the number
    from a checker under `codePaths`, where a review can see how it was
    counted.

    The adapter owns `passed` (the recipe's exit) and `transcriptDigest` (the
    recipe's combined output), so a reviewer can tie recorded evidence to a
    transcript. The report is deleted before the recipe starts, so a leftover
    from an earlier run cannot answer for this one.
    """
    target = named_target(request)
    clear_observations_report(target)
    environment = dict(os.environ) | {devloop_observations.ENVIRONMENT: target}
    record, _ = run_once(request, "just-observations", target, environment=environment)
    reserved = {"passed": record["passed"], "transcriptDigest": record.get("transcriptDigest", "")}
    return typed_observations("just-observations", record.get("report"), reserved)


def work_item_store(request: dict) -> list[dict]:
    # Counts need the complete text, independently of the bounded raw log.
    def complete_recipe(target: str) -> tuple[bool, str]:
        key = run_key(request, "work-item-store", target)
        try:
            result = devloop_diagnostics.capture(target, None, complete_text=True)
            receipt = devloop_diagnostics.retain(key, request, target, result)
            devloop_diagnostics.announce(receipt, key, request, target)
            return result[0], result.complete_text
        except (OSError, ValueError) as error:
            message = f"diagnostic capture error for run {key}: {error}"
            try:
                devloop_diagnostics.notify(message)
            except (OSError, ValueError):
                pass
            raise CannotRun(message) from error

    tasks_passed, tasks_output = complete_recipe("tasks_check")
    docs_passed, _ = complete_recipe("docs_check")

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


GATES = {
    "work-item-store": work_item_store,
    "just-target": just_target,
    "just-observations": just_observations,
}


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
