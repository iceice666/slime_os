"""The exam for test-run records that name their image closure (`contracts/system-test-run/v2`).

A version 1 record copied its closure's identity, so one change to a closure input rewrote every
record that boots the closure. This module owns what version 2 must be, and it never imports the
generator or the checker it judges:

* the schema: `contracts/system-test-run/v2/schema.zt` declares exactly the pinned fields;
* the records: every run names its closure by the name pinned here, and no version 1 record remains;
* the claim: a closure input change rewrites no test-run record and no negative build case
  (`scripts/check/check-system-test-run-references.py` runs that in a scratch checkout);
* the refusals: the repository's checker refuses a run that names an unknown or a wrong closure,
  or that carries an identity again.

The record judge is trusted only after it refuses each corrupted record set in `controls()`.
"""

from __future__ import annotations

import re
from collections.abc import Callable, Iterator

from harness import ROOT

SCHEMA = ROOT / "contracts" / "system-test-run" / "v2" / "schema.zt"
RUNS_V1 = "contracts/system-test-run/v1/runs"
RUNS_V2 = "contracts/system-test-run/v2/runs"
CLOSURE_GLOB = "contracts/system-image-closure/v*/closures/*.zti"
FORMAT_VERSION = 2

FIELDS = (
    "formatVersion", "name", "imageClosure", "executionKind", "executionProfile", "disks", "networks",
    "devices", "faultControls", "timeoutSeconds", "markerContractIdentity", "forbiddenOutcomes",
)

# The runs and the closure each boots, read from the version 1 records at 00af4fde (#886) by matching
# each copied identity with the closure it equals. A run absent from these tables was added later and
# must still name an existing closure.
EXEMPT = frozenset({
    "sel4-boot-selection",
    "sel4-generation-qemu-riscv-virt",
    "sel4-private-memory-cycles-qemu-riscv-virt",
    "sel4-private-memory-qemu-riscv-virt",
    "sel4-rollback-qemu-riscv-virt",
})
RENAMED = {
    "sel4-component-graph": "sel4",
    "sel4-device": "sel4",
    "sel4-qos": "sel4-qos-death",
    "sel4-reclamation": "sel4-reclamation-unwind",
    "sel4-stream": "sel4-stream-death",
}
SAME_NAME = frozenset({
    "sel4-boot", "sel4-c-runtime", "sel4-call", "sel4-channel", "sel4-clock-authority",
    "sel4-crossing", "sel4-demo", "sel4-directory", "sel4-entropy", "sel4-fault", "sel4-filesystem",
    "sel4-generation", "sel4-http", "sel4-http-public", "sel4-input", "sel4-io-block",
    "sel4-io-driver-authority", "sel4-io-driver-reset-in-flight", "sel4-io-lifetime",
    "sel4-io-link", "sel4-io-local", "sel4-io-network", "sel4-io-queue",
    "sel4-io-service-fault-in-flight", "sel4-io-tcp", "sel4-io-tcp-buffers",
    "sel4-io-tcp-impairment", "sel4-io-tcp-listener", "sel4-io-tcp-options", "sel4-large-image",
    "sel4-lifecycle-restart", "sel4-loan", "sel4-matrix", "sel4-mavlink", "sel4-net",
    "sel4-operation", "sel4-powerbox", "sel4-private-memory", "sel4-private-memory-cycles",
    "sel4-private-memory-fail-large-map", "sel4-private-memory-fail-second-allocation", "sel4-pwm",
    "sel4-recovery", "sel4-replay", "sel4-robot-runtime", "sel4-rollback", "sel4-sample",
    "sel4-saturation", "sel4-scheduling-class", "sel4-spawn", "sel4-storage", "sel4-store",
    "sel4-stress", "sel4-supervision", "sel4-traffic", "sel4-transfer", "sel4-visibility",
    "sel4-wait-set", "sel4-zenoh", "sel4-zenoh-external",
})


def expected_closure(run: str) -> str | None:
    """The closure a pinned run must name, `""` for an exempt run, or None for an unpinned run."""
    if run in EXEMPT:
        return ""
    if run in RENAMED:
        return RENAMED[run]
    if run in SAME_NAME:
        return run
    return None


PINNED = sorted(EXEMPT | set(RENAMED) | SAME_NAME)
DIGEST = re.compile(r"^[0-9a-f]{64}$")


def schema_findings(text: str) -> list[str]:
    """Where the schema's run record differs from the pinned fields and version."""
    findings = []
    block = re.search(r"^SystemTestRun :: type \{(.*?)^\};", text, re.M | re.S)
    fields = () if block is None else tuple(re.findall(r"^\s+(\w+) :", block.group(1), re.M))
    if fields != FIELDS:
        findings.append(f"SystemTestRun must declare {list(FIELDS)}, it declares {list(fields)}")
    if re.findall(r"^formatVersion :: Int = (\d+);", text, re.M) != [str(FORMAT_VERSION)]:
        findings.append(f"formatVersion must be stated once as {FORMAT_VERSION}")
    if "imageClosureIdentity" in text:
        findings.append("the schema still names imageClosureIdentity")
    return findings


def judge_records(records: dict[str, object], *, v1_records: list[str], closures: set[str]) -> list[str]:
    """Findings for the decoded version 2 records, keyed by file stem."""
    findings = []
    if v1_records:
        findings.append(f"{len(v1_records)} version 1 record(s) remain under {RUNS_V1}, e.g. {v1_records[0]}")
    missing = [run for run in PINNED if run not in records]
    if missing:
        findings.append(f"{len(missing)} pinned run(s) have no version 2 record, e.g. {missing[0]}")
    for stem, record in sorted(records.items()):
        if not isinstance(record, dict):
            findings.append(f"{stem}: the record is not a record")
            continue
        if set(record) != set(FIELDS):
            findings.append(f"{stem}: fields {sorted(record)} are not {sorted(FIELDS)}")
            continue
        if record["formatVersion"] != FORMAT_VERSION:
            findings.append(f"{stem}: formatVersion {record['formatVersion']} is not {FORMAT_VERSION}")
        if record["name"] != stem:
            findings.append(f"{stem}: the record is named {record['name']!r}")
        named = record["imageClosure"]
        if not isinstance(named, str) or DIGEST.fullmatch(named):
            findings.append(f"{stem}: imageClosure {named!r} is not a closure name")
            continue
        expected = expected_closure(stem)
        if expected is not None and named != expected:
            findings.append(f"{stem}: names closure {named!r}, the run boots {expected!r}")
        elif expected is None and named == "":
            findings.append(f"{stem}: a run added after the pin must name a closure")
        if named and named not in closures:
            findings.append(f"{stem}: names closure {named!r}, which does not exist")
    return findings


# Controls: each corrupts an honest record set and requires `judge_records` to find it.

def _honest() -> dict[str, dict]:
    def record(run: str) -> dict:
        values = {
            "formatVersion": FORMAT_VERSION, "name": run, "imageClosure": expected_closure(run),
            "executionKind": "emulator", "executionProfile": "qemu-arm-virt", "disks": [], "networks": [],
            "devices": [], "faultControls": [], "timeoutSeconds": 240, "markerContractIdentity": "a" * 64,
            "forbiddenOutcomes": [],
        }
        return {field: values[field] for field in FIELDS}

    return {run: record(run) for run in PINNED}


CLOSURES = frozenset(SAME_NAME | set(RENAMED.values()))


def _edit(run: str, **fields: object) -> Callable[[dict], dict]:
    def apply(records: dict) -> dict:
        records[run] = {**records[run], **fields}
        return records
    return apply


def _record_controls() -> Iterator[tuple[str, bool]]:
    def findings(edit: Callable[[dict], dict] | None = None, *, v1: list[str] | None = None,
                 closures: frozenset[str] = CLOSURES) -> list[str]:
        records = _honest()
        if edit is not None:
            records = edit(records)
        return judge_records(records, v1_records=v1 or [], closures=set(closures))

    yield "records-honest", not findings()
    with_identity = lambda records: {**records, "sel4-zenoh": {**records["sel4-zenoh"], "imageClosureIdentity": "b" * 64}}  # noqa: E731
    without_name = lambda records: {k: {f: v for f, v in r.items() if f != "imageClosure"} if k == "sel4-zenoh" else r for k, r in records.items()}  # noqa: E731
    cases = {
        "records-v1-remain": findings(v1=["sel4-zenoh.zti"]),
        "records-run-missing": findings(lambda records: {k: v for k, v in records.items() if k != "sel4-qos"}),
        "records-identity-field": findings(with_identity),
        "records-name-field-missing": findings(without_name),
        "records-digest-as-name": findings(_edit("sel4-zenoh", imageClosure="c" * 64)),
        "records-wrong-closure": findings(_edit("sel4-zenoh", imageClosure="sel4-channel")),
        "records-renamed-run-own-name": findings(_edit("sel4-qos", imageClosure="sel4-qos")),
        "records-exempt-names-closure": findings(_edit("sel4-boot-selection", imageClosure="sel4-boot")),
        "records-closure-dropped": findings(_edit("sel4-zenoh", imageClosure="")),
        "records-unknown-closure": findings(closures=CLOSURES - {"sel4-zenoh"}),
        "records-version-1": findings(_edit("sel4-zenoh", formatVersion=1)),
        "records-name-mismatch": findings(_edit("sel4-zenoh", name="sel4-zenoh-2")),
        "records-added-run-exempt": findings(lambda records: {**records, "sel4-new": {**records["sel4-boot-selection"], "name": "sel4-new"}}),
        "records-added-run-unknown": findings(lambda records: {**records, "sel4-new": {**records["sel4-zenoh"], "name": "sel4-new", "imageClosure": "sel4-new"}}),
    }
    for name, found in cases.items():
        yield name, bool(found)
    added = findings(lambda records: {**records, "sel4-new": {**records["sel4-zenoh"], "name": "sel4-new"}})
    yield "records-added-run-honest", not added


def _schema_controls(text: str) -> Iterator[tuple[str, bool]]:
    yield "schema-honest", not schema_findings(text)
    edits = {
        "schema-identity-field": ("  imageClosure : Text;", "  imageClosureIdentity : Text;"),
        "schema-version-1": ("formatVersion :: Int = 2;", "formatVersion :: Int = 1;"),
        "schema-field-added": ("  imageClosure : Text;", "  imageClosure : Text;\n  imageDigest : Text;"),
    }
    for name, (old, new) in edits.items():
        assert text.count(old) == 1, name
        yield name, bool(schema_findings(text.replace(old, new)))


def honest_schema() -> str:
    """The parts of a v2 schema `schema_findings` reads, written from the pinned fields.

    The implementation lands the real schema; the controls must not depend on it existing.
    """
    types = {"disks": "List Fixture", "networks": "List Fixture", "devices": "List Fixture",
             "faultControls": "List FaultControl", "forbiddenOutcomes": "List Text"}
    lines = [f"  {field} : {types.get(field, 'Int' if field in ('formatVersion', 'timeoutSeconds') else 'Text')};"
             for field in FIELDS]
    return "SystemTestRun :: type {\n" + "\n".join(lines) + "\n};\n\nformatVersion :: Int = 2;\n"


def controls() -> list[tuple[str, bool]]:
    """(name, passed): the honest form accepted, or the corruption found."""
    return [*_schema_controls(honest_schema()), *_record_controls()]
