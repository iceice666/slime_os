#!/usr/bin/env python3

"""Gate `contracts/system-test-run/v1`: execution inputs are declared, bounded, and typed.

CP11's boundary is that a marker oracle, a timeout, a disk fixture, or an
injected device event cannot alter the image closure it exercises. That
separation is only real if both halves are checked: the closure side by
`just system_image_closure_check`, and this side here.

What this asserts:

1. every plane gate that boots a seL4 QEMU image has a record, and every record
   names a plane gate — neither set may contain a name the other lacks;
2. each record decodes through the generated contract binding, so the bounds and
   closed vocabularies the schema declares are enforced rather than assumed;
3. a record naming a closure resolves it, and a record naming none corresponds
   to an image the aggregate gate independently agrees is closure-exempt — so an
   empty identity is a checked statement rather than a blank field;
4. every declared timeout is positive and within the contract's ceiling, and
   every fault kind and execution kind is from the closed vocabulary;
5. the records agree with the checkers they were extracted from, which is what
   makes them a freeze rather than a snapshot that silently rots.

The records are declarations, not yet the execution path: the checkers still run
from their own constants, and CP15's migration is what makes these the input. The
freeze is what that migration will be verified against.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import importlib.util
import json
import re
import subprocess

from harness import ROOT
from zutai_cli import STDLIB, binary

RUN_ROOT = ROOT / "contracts" / "system-test-run" / "v1" / "runs"
CONTRACT = ROOT / "contracts" / "system-test-run" / "v1"
CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"
GENERATOR = ROOT / "scripts" / "generate" / "generate-system-test-runs.py"
AGGREGATE = ROOT / "scripts" / "check" / "check-system-image-aggregate.py"


def fail(message: str) -> None:
    raise SystemExit(f"system test run check: {message}")


def load(path: _Path):
    spec = importlib.util.spec_from_file_location(f"str_{path.stem.replace('-', '_')}", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


GENERATOR_MODULE = load(GENERATOR)


def aggregate_exempt_images() -> set[str]:
    """The images the aggregate gate declares closure-exempt.

    Read from that gate's source rather than by importing it: it runs its own
    assertions at import, and one gate silently executing another makes a
    failure attributable to the wrong checker.
    """
    text = AGGREGATE.read_text(encoding="utf-8")
    block = re.search(r"IMAGES_WITHOUT_CLOSURE = \{(.*?)\n\}", text, re.DOTALL)
    if block is None:
        fail("could not read the aggregate gate's exemption table")
    images = set(re.findall(r'"(slime-[a-z0-9.-]+\.elf)":', block.group(1)))
    if not images:
        fail("the aggregate gate's exemption table is empty, so no record may omit a closure")
    return images


def check_records_and_planes_correspond() -> int:
    """Every plane gate has a record and every record has a plane gate."""
    expected = {GENERATOR_MODULE.run_name(path) for path in GENERATOR_MODULE.plane_checkers()}
    present = {path.stem for path in RUN_ROOT.glob("*.zti")}
    if not expected:
        fail("no plane checker was found, so this gate asserts nothing")
    missing = sorted(expected - present)
    if missing:
        fail(f"plane gate(s) with no test-run record: {missing}")
    extra = sorted(present - expected)
    if extra:
        fail(f"test-run record(s) naming no plane gate: {extra}")
    return len(present)


def decode(path: _Path) -> dict:
    """Decode one record through the contract, so schema bounds are enforced.

    Two steps, matching the convention every other contract gate here uses: the
    contract's own checker must answer `#valid`, and only then is the JSON
    projection read. Reading the projection alone would prove shape but not the
    bounds and closed vocabularies the schema declares.
    """
    import os

    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_SYSTEM_TEST_RUN_PATH"] = str(path)
    process = subprocess.run(
        [str(binary()), "run", str(CONTRACT / "check.zt")],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0 or not process.stdout.startswith("#valid"):
        detail = (process.stderr or process.stdout).strip()
        fail(f"{path.stem}: the contract refused this record: {detail}")
    process = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        fail(f"{path.stem}: invalid JSON projection: {(process.stderr or process.stdout).strip()}")
    return json.loads(process.stdout)


def path_json(path: _Path) -> str:
    """The record's JSON projection."""
    import os

    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        fail(f"{path.stem}: could not project to JSON: {(process.stderr or process.stdout).strip()}")
    return process.stdout


def contract_vocabulary() -> dict[str, set[str]]:
    """The closed vocabularies and ceilings the schema declares."""
    schema = (CONTRACT / "schema.zt").read_text(encoding="utf-8")
    execution = set(re.findall(r'^execution[A-Za-z]+ :: Text = "([a-z-]+)";', schema, re.MULTILINE))
    faults = set(re.findall(r'^fault[A-Za-z]+ :: Text = "([a-z-]+)";', schema, re.MULTILINE))
    ceiling = re.search(r"^maxTimeoutSeconds :: Int = (\d+);", schema, re.MULTILINE)
    fixtures = re.search(r"^maxFixtures :: Int = (\d+);", schema, re.MULTILINE)
    if not execution or not faults or ceiling is None or fixtures is None:
        fail("the schema declares no execution kind, fault kind, or ceiling to check against")
    return {
        "execution": execution,
        "faults": faults,
        "maxTimeout": {int(ceiling.group(1))},
        "maxFixtures": {int(fixtures.group(1))},
    }


def check_records(vocabulary: dict[str, set[str]]) -> tuple[int, int, int]:
    """Each record is typed, bounded, closed-vocabulary, and closure-consistent."""
    exempt_images = aggregate_exempt_images()
    closures = {}
    for path in sorted(CLOSURE_ROOT.glob("*.zti")):
        from system_image_closure import compile_closure

        closures[compile_closure(path).identity.hex()] = path.stem

    max_timeout = next(iter(vocabulary["maxTimeout"]))
    max_fixtures = next(iter(vocabulary["maxFixtures"]))
    resolved, exempt, faults = 0, 0, 0

    for path in sorted(RUN_ROOT.glob("*.zti")):
        record = decode(path)
        name = record["name"]
        if name != path.stem:
            fail(f"{path.stem}: declares name {name!r}, so the file and the record disagree")
        if record["formatVersion"] != 1:
            fail(f"{name}: declares format version {record['formatVersion']}, expected 1")
        if record["executionKind"] not in vocabulary["execution"]:
            fail(f"{name}: execution kind {record['executionKind']!r} is not in the vocabulary")

        timeout = record["timeoutSeconds"]
        if timeout <= 0:
            fail(f"{name}: declares a non-positive timeout, so its run would be unbounded")
        if timeout > max_timeout:
            fail(f"{name}: timeout {timeout} exceeds the contract ceiling {max_timeout}")

        for section in ("disks", "networks", "devices"):
            if len(record[section]) > max_fixtures:
                fail(f"{name}: declares more {section} than the contract admits")
            for fixture in record[section]:
                if not fixture["name"]:
                    fail(f"{name}: a {section} fixture has no name")

        for control in record["faultControls"]:
            if control["kind"] not in vocabulary["faults"]:
                fail(f"{name}: fault kind {control['kind']!r} is not in the vocabulary")
            faults += 1

        identity = record["imageClosureIdentity"]
        if identity:
            if identity not in closures:
                fail(f"{name}: names closure identity {identity} that resolves to no closure")
            if closures[identity] != name:
                fail(
                    f"{name}: names the closure for {closures[identity]!r}, so a run would "
                    "exercise another plane's image"
                )
            resolved += 1
        else:
            checker = next(
                path
                for path in GENERATOR_MODULE.plane_checkers()
                if GENERATOR_MODULE.run_name(path) == name
            )
            image = GENERATOR_MODULE.booted_image(checker)
            if image not in exempt_images:
                fail(
                    f"{name}: names no closure, but {image} is not one the aggregate gate "
                    "declares closure-exempt"
                )
            exempt += 1

    return resolved, exempt, faults


def check_records_match_their_checkers() -> None:
    """The freeze holds: no record drifted from the gate it was extracted from."""
    process = subprocess.run(
        ["python3", str(GENERATOR), "--check"],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"records disagree with their checkers:\n{process.stdout.strip()}")


def check_controls(vocabulary: dict[str, set[str]]) -> int:
    """This gate refuses each drift it claims to catch."""
    import copy
    import tempfile

    template = json.loads(path_json(RUN_ROOT / "sel4-channel.zti"))
    controls = [
        ("an unadmitted execution kind", {"executionKind": "hypervisor"}, "not in the vocabulary"),
        ("a zero timeout", {"timeoutSeconds": 0}, "unbounded"),
        (
            "a timeout beyond the contract ceiling",
            {"timeoutSeconds": next(iter(vocabulary["maxTimeout"])) + 1},
            "exceeds the contract ceiling",
        ),
        (
            "an unresolvable closure identity",
            {"imageClosureIdentity": "0" * 64},
            "resolves to no closure",
        ),
        (
            "an unadmitted fault kind",
            {"faultControls": [{"kind": "ambient", "target": "x", "value": "y"}]},
            "is not in the vocabulary",
        ),
    ]
    refused = 0
    for label, patch, needle in controls:
        value = copy.deepcopy(template)
        value.update(patch)
        with tempfile.TemporaryDirectory(prefix="slime-test-run-control-") as scope:
            scratch = _Path(scope) / "runs"
            scratch.mkdir()
            for path in RUN_ROOT.glob("*.zti"):
                (scratch / path.name).write_text(path.read_text(encoding="utf-8"), encoding="utf-8")
            (scratch / "sel4-channel.zti").write_text(
                GENERATOR_MODULE.render(
                    value["name"],
                    value["imageClosureIdentity"],
                    {
                        "timeoutSeconds": value["timeoutSeconds"],
                        "drives": len(value["disks"]),
                        "devices": [device["name"] for device in value["devices"]],
                        "forbiddenOutcomes": value["forbiddenOutcomes"],
                        "faults": [control["kind"] for control in value["faultControls"]],
                    },
                ).replace('executionKind = "emulator"', f'executionKind = "{value["executionKind"]}"'),
                encoding="utf-8",
            )
            try:
                _check_scratch(scratch, vocabulary)
            except SystemExit as error:
                if needle not in str(error):
                    fail(f"control {label!r} failed for the wrong reason: {error}")
                refused += 1
            else:
                fail(f"control {label!r} was accepted, so this gate does not catch it")
    return refused


def _check_scratch(scratch: _Path, vocabulary: dict[str, set[str]]) -> None:
    """Run the record assertions against a scratch directory."""
    global RUN_ROOT
    saved = RUN_ROOT
    RUN_ROOT = scratch
    try:
        check_records(vocabulary)
    finally:
        RUN_ROOT = saved


vocabulary = contract_vocabulary()
record_count = check_records_and_planes_correspond()
resolved, exempt, fault_count = check_records(vocabulary)
check_records_match_their_checkers()
control_count = check_controls(vocabulary)

print(
    f"system test run check: {record_count} record(s) correspond one-to-one with the plane "
    f"gates that boot a seL4 image, each decoded through the contract with closed execution "
    f"and fault vocabularies and a bounded timeout; {resolved} name a closure that resolves to "
    f"their own plane and {exempt} name none because their image is declared closure-exempt; "
    f"{fault_count} fault control(s) declared; every record matches the checker it was frozen "
    f"from; and {control_count} named control(s) refused"
)
