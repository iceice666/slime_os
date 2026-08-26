#!/usr/bin/env python3
"""M5.6c BootState model-implementation conformance check.

Boots the seL4 rollback power-cut scenario, collects the durable BootState
transition traces emitted by its generation-management component, and validates
each finite trace against the checked M5.6a/M5.6b state machines in
`contracts/bootstate/model/`.

Two conformance layers, neither a re-transcription of the model:

  * Abstract legality is decided by `zutai model-check` over the real typed
    `bootstate.zt` transition system. A record's durable post-state is accepted
    only when it is reachable in the model; a record that reads, decodes, or
    launches candidate bytes before the attempt decrement is durable has no
    reachable state and is rejected.
  * Concrete root binding maps the abstract roots the model does not carry onto
    the on-disk BootState identities. A promotion or collection against the
    wrong root is rejected here.

The check also proves the negative cases required by M5.6c: an attempt that was
not durably decremented, and a promotion or collection against the wrong root,
are all rejected; and that trace instrumentation stays bounded.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import subprocess
import sys
import tempfile
from pathlib import Path

from boot_contracts import (
    BOOTSTATE_SLOT_BYTES,
    BOOTSTATE_TRACE_MAX_LINE,
    BOOTSTATE_TRACE_PREFIX,
    BOOTSTATE_TRACE_VERSION,
    sha256,
)
from harness import ROOT, load_script
from zutai_cli import STDLIB, binary

MODEL_DIR = ROOT / "contracts" / "bootstate" / "model"
TRACE_PREFIX = BOOTSTATE_TRACE_PREFIX
TRACE_VERSION = BOOTSTATE_TRACE_VERSION
MAX_LINE = BOOTSTATE_TRACE_MAX_LINE
# Six live durable transitions plus bounded headroom for future pure-selection
# records keep the serial artifact finite without freezing this exact scenario.
MAX_TRACE_LINES_PER_BOOT = 8
ORACLE_TIMEOUT_SECONDS = 300

CHECK_GENERATION = load_script("check_generation", "check/check-generation.py")
ROLLBACK = load_script("sel4_rollback_plane", "check/check-sel4-rollback-plane.py")

ACTIONS = {
    "consume-attempt",
    "promotion",
    "boot-known-good",
    "boot-exhausted-known-good",
    "stage-pending",
    "rollback",
    # `collect` is an adversarial checker-only action. `generation_root` names
    # the candidate identity; it is validated against observable retained
    # roots instead of the BootState transition oracle.
    "collect",
}
COMMITS = {
    "none",
    "after-attempt-commit",
    "health-promotion",
    "after-pending-commit",
    "rollback-update",
}
SLOTS = {"A", "B"}
HEX32 = 64


class TraceError(Exception):
    pass





def parse_hex32(field: str, value: str) -> bytes:
    if len(value) != HEX32:
        raise TraceError(f"{field} is not a 32-byte hex identity: {value!r}")
    try:
        return bytes.fromhex(value)
    except ValueError as error:
        raise TraceError(f"{field} is not valid hex: {value!r}") from error


def parse_trace_line(line: str) -> dict:
    if len(line) > MAX_LINE:
        raise TraceError(f"trace line exceeds {MAX_LINE} bytes: {len(line)}")
    tokens = line.split()
    if not tokens or tokens[0] != TRACE_PREFIX:
        raise TraceError(f"missing trace prefix: {line!r}")
    if len(tokens) < 2 or tokens[1] != f"v{TRACE_VERSION}":
        raise TraceError(f"unexpected trace version: {line!r}")
    fields: dict[str, str] = {}
    for token in tokens[2:]:
        key, sep, value = token.partition("=")
        if not sep:
            raise TraceError(f"malformed field {token!r} in {line!r}")
        if key in fields:
            raise TraceError(f"duplicate field {key!r} in {line!r}")
        fields[key] = value
    required = {
        "action",
        "commit",
        "selected_slot",
        "target_slot",
        "sequence_before",
        "sequence_after",
        "attempts_before",
        "attempts_after",
        "known_good",
        "pending",
        "generation_root",
        "state_root",
    }
    missing = required - fields.keys()
    if missing:
        raise TraceError(f"missing fields {sorted(missing)} in {line!r}")

    action = fields["action"]
    if action not in ACTIONS:
        raise TraceError(f"unknown action {action!r}")
    commit = fields["commit"]
    if commit not in COMMITS:
        raise TraceError(f"unknown commit boundary {commit!r}")
    if fields["selected_slot"] not in SLOTS:
        raise TraceError(f"bad selected_slot {fields['selected_slot']!r}")
    if fields["target_slot"] != "-" and fields["target_slot"] not in SLOTS:
        raise TraceError(f"bad target_slot {fields['target_slot']!r}")

    record = {
        "action": action,
        "commit": commit,
        "selected_slot": fields["selected_slot"],
        "target_slot": None if fields["target_slot"] == "-" else fields["target_slot"],
        "sequence_before": int(fields["sequence_before"]),
        "sequence_after": int(fields["sequence_after"]),
        "attempts_before": int(fields["attempts_before"]),
        "attempts_after": int(fields["attempts_after"]),
        "known_good": parse_hex32("known_good", fields["known_good"]),
        "pending": None
        if fields["pending"] == "none"
        else parse_hex32("pending", fields["pending"]),
        "generation_root": parse_hex32("generation_root", fields["generation_root"]),
        "state_root": parse_hex32("state_root", fields["state_root"]),
    }
    record["raw"] = line
    return record


class Oracle:
    """Runs `zutai model-check` over bootstate.zt for abstract legality."""

    def __init__(self) -> None:
        self._cache: dict[tuple[str, str, int, int], bool] = {}

    def reachable(
        self, action: str, commit: str, attempts_before: int, attempts_after: int
    ) -> bool:
        key = (action, commit, attempts_before, attempts_after)
        if key in self._cache:
            return self._cache[key]

        with tempfile.TemporaryDirectory(
            prefix=".trace-query-", dir=MODEL_DIR
        ) as temporary:
            query_dir = Path(temporary)
            (query_dir / "bootstate.zt").write_bytes(
                (MODEL_DIR / "bootstate.zt").read_bytes()
            )
            (query_dir / "observation.zti").write_text(
                "{ "
                f'action = "{action}"; '
                f'commit = "{commit}"; '
                f"attemptsBefore = {attempts_before}; "
                f"attemptsAfter = {attempts_after}; "
                "}\n"
            )
            (query_dir / "query.zt").write_text(
                'm ::= import "bootstate.zt";\n'
                'obs ::= import "observation.zti";\n'
                "base ::= m.bootStateModel m.noFaults;\n"
                "model ::= base with {\n"
                "  safety = {;};\n"
                '  reachability = { { name = "observed"; reached = m.observedReached obs; }; };\n'
                "};\n"
                '{ scenarios = { { name = "trace"; model = model; expect = #safe; }; }; }\n'
            )
            environment = os.environ.copy()
            environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
            try:
                process = subprocess.run(
                    [str(binary()), "model-check", str(query_dir / "query.zt")],
                    cwd=ROOT,
                    env=environment,
                    check=False,
                    text=True,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    timeout=ORACLE_TIMEOUT_SECONDS,
                )
            except subprocess.TimeoutExpired as error:
                if error.stdout:
                    output = error.stdout
                    if isinstance(output, bytes):
                        output = output.decode(errors="replace")
                    sys.stdout.write(output)
                raise SystemExit(
                    "abstract oracle timed out after "
                    f"{ORACLE_TIMEOUT_SECONDS}s for {key!r}"
                ) from error

        combined = process.stdout
        if process.returncode == 0:
            reachable = True
        elif (
            process.returncode == 1
            and 'reachability "observed" never reached' in combined
        ):
            reachable = False
        else:
            sys.stdout.write(combined)
            raise SystemExit("oracle run failed unexpectedly")
        self._cache[key] = reachable
        return reachable


def check_transition_shape(record: dict) -> None:
    action = record["action"]
    selected = record["selected_slot"]
    target = record["target_slot"]
    sequence_before = record["sequence_before"]
    sequence_after = record["sequence_after"]

    if action in {"consume-attempt", "promotion", "stage-pending", "rollback"}:
        if target is None or target == selected:
            raise TraceError(f"{action} must target the other BootState slot")
        if sequence_after != sequence_before + 1:
            raise TraceError(f"{action} must advance the durable sequence by one")
    elif action in {"boot-known-good", "boot-exhausted-known-good"}:
        if target is not None:
            raise TraceError(f"{action} must not name a write target")
        if sequence_after != sequence_before:
            raise TraceError(f"{action} must not advance the durable sequence")
    elif action == "collect":
        if record["commit"] != "none" or target is not None:
            raise TraceError("collect checker records do not perform a BootState write")
        if sequence_after != sequence_before:
            raise TraceError("collect checker records must not change BootState sequence")


def validate_record(record: dict, oracle: Oracle, retained: set[bytes] | None = None) -> None:
    check_transition_shape(record)
    if record["action"] == "collect":
        if retained is None:
            raise TraceError("collect validation requires the retained-root set")
        check_collect(record["generation_root"], retained)
        return
    if not oracle.reachable(
        record["action"],
        record["commit"],
        record["attempts_before"],
        record["attempts_after"],
    ):
        raise TraceError(f"{record['action']} post-state is not reachable in the model")


def select_state(states: list[dict]) -> dict:
    if not states:
        raise TraceError("neither durable BootState slot decodes")
    highest = max(state["sequence"] for state in states)
    newest = [state for state in states if state["sequence"] == highest]
    if len(newest) == 2 and newest[0] != newest[1]:
        raise TraceError("equal-sequence BootState slots conflict")
    return newest[0]


def selected_state(disk: Path) -> dict:
    image = disk.read_bytes()
    partition_first_lba = 40
    states = []
    for relative_lba in (1024, 1025):
        start = (partition_first_lba + relative_lba) * 512
        slot = image[start : start + BOOTSTATE_SLOT_BYTES]
        try:
            states.append(CHECK_GENERATION.decode_bootstate(slot))
        except CHECK_GENERATION.CheckError:
            pass
    return select_state(states)


def check_trace_chain(records: list[dict], final: dict, oracle: Oracle) -> None:
    expected_actions = [
        "stage-pending",
        "consume-attempt",
        "consume-attempt",
        "consume-attempt",
        "rollback",
        "stage-pending",
        "promotion",
    ]
    if [record["action"] for record in records] != expected_actions:
        raise TraceError("live trace does not contain the complete rollback/promotion chain")
    if [record["attempts_after"] for record in records] != [3, 2, 1, 0, 0, 3, 0]:
        raise TraceError("live trace has the wrong durable attempt sequence")

    for record in records:
        validate_record(record, oracle)
    for previous, current in zip(records, records[1:], strict=False):
        if previous["sequence_after"] != current["sequence_before"]:
            raise TraceError("trace sequences are not contiguous")
        if previous["target_slot"] != current["selected_slot"]:
            raise TraceError("trace slot selection does not follow the durable write")
        if previous["attempts_after"] != current["attempts_before"]:
            raise TraceError("trace attempt counts are not contiguous")

    generation_root = records[0]["generation_root"]
    state_root = records[0]["state_root"]
    if any(record["generation_root"] != generation_root for record in records):
        raise TraceError("generation root changed inside the transition chain")
    if any(record["state_root"] != state_root for record in records):
        raise TraceError("state root changed inside the transition chain")

    first_stage, first_attempt, middle_attempt, last_attempt, rollback, second_stage, promotion = records
    if first_stage["pending"] is None or first_stage["known_good"] == first_stage["pending"]:
        raise TraceError("staging did not preserve distinct known-good and pending roots")
    for record in (first_attempt, middle_attempt, last_attempt):
        if record["known_good"] != first_stage["known_good"] or record["pending"] != first_stage["pending"]:
            raise TraceError("attempt consumption changed a retained identity")
    if rollback["known_good"] != first_stage["known_good"] or rollback["pending"] is not None:
        raise TraceError("rollback did not restore the retained known-good root")
    if second_stage["known_good"] != rollback["known_good"] or second_stage["pending"] != first_stage["pending"]:
        raise TraceError("restaging changed the generation identities")
    if promotion["known_good"] != second_stage["pending"] or promotion["pending"] is not None:
        raise TraceError("promotion did not select and clear the running pending root")

    for field in ("sequence", "known_good", "pending", "remaining_attempts", "generation_root", "state_root"):
        trace_field = {
            "sequence": "sequence_after",
            "remaining_attempts": "attempts_after",
        }.get(field, field)
        if final[field] != promotion[trace_field]:
            raise TraceError(f"final durable BootState disagrees with trace field {field}")


def retained_roots(records: list[dict], final: dict) -> set[bytes]:
    roots = {final["known_good"], final["generation_root"], final["state_root"]}
    if final["pending"] is not None:
        roots.add(final["pending"])
    for record in records:
        roots.add(record["known_good"])
        roots.add(record["generation_root"])
        roots.add(record["state_root"])
        if record["pending"] is not None:
            roots.add(record["pending"])
    return roots

def check_collect(candidate: bytes, roots: set[bytes]) -> None:
    if candidate in roots:
        raise TraceError("collection targets a retained root")


def collect_traces(output: str) -> list[dict]:
    records = []
    for line in output.splitlines():
        line = line.rstrip("\r")
        if line.startswith(TRACE_PREFIX):
            records.append(parse_trace_line(line))
    return records


def run_scenario() -> tuple[list[dict], dict]:
    pins = ROLLBACK.load_pins()
    profile = pins["qemu_arm_virt"]
    if not isinstance(profile, dict):
        raise TraceError("invalid seL4 QEMU profile")
    ROLLBACK.build_image()
    with tempfile.TemporaryDirectory() as directory:
        disk = Path(directory) / "rollback-plane.img"
        ROLLBACK.build_fixture(disk)
        transcript = ROLLBACK.boot(profile, disk)
        ROLLBACK.check_transcript(transcript)
        ROLLBACK.check_slots_durable(disk, 40)
        records = collect_traces(transcript)
        if not records:
            raise TraceError("the seL4 rollback plane emitted no BootState trace")
        if len(records) > MAX_TRACE_LINES_PER_BOOT:
            raise TraceError(
                f"the seL4 rollback plane emitted {len(records)} trace lines; bound is "
                f"{MAX_TRACE_LINES_PER_BOOT}"
            )
        final = selected_state(disk)
    return records, final


def assert_rejected(description: str, action) -> None:
    try:
        action()
    except TraceError:
        return
    raise SystemExit(f"validator accepted {description}; expected rejection")


def malformed_trace_corpus(example: str) -> None:
    mutations = (
        ("missing field", example.replace(" action=stage-pending", "")),
        ("duplicate field", example + " action=stage-pending"),
        ("wrong version", example.replace(" v1 ", " v99 ", 1)),
        ("oversized line", example + " " + "x" * MAX_LINE),
    )
    for description, line in mutations:
        assert_rejected(description, lambda line=line: parse_trace_line(line))


def main() -> None:
    oracle = Oracle()
    records, final = run_scenario()
    check_trace_chain(records, final, oracle)

    # Reading, decoding, or launching before the decrement is durable is unreachable.
    consume = next(record for record in records if record["action"] == "consume-attempt")
    stalled = dict(consume)
    stalled["attempts_after"] = stalled["attempts_before"]
    assert_rejected("an undurable attempt decrement", lambda: validate_record(stalled, oracle))

    wrong_commit = dict(consume)
    wrong_commit["commit"] = "none"
    assert_rejected("a wrong commit boundary", lambda: validate_record(wrong_commit, oracle))

    wrong_sequence = dict(consume)
    wrong_sequence["sequence_after"] = wrong_sequence["sequence_before"]
    assert_rejected("a repeated durable sequence", lambda: validate_record(wrong_sequence, oracle))

    wrong_root = [dict(record) for record in records]
    wrong_root[2]["state_root"] = sha256(b"wrong-state-root")
    assert_rejected(
        "a transition against the wrong state root",
        lambda: check_trace_chain(wrong_root, final, oracle),
    )

    wrong_promotion = [dict(record) for record in records]
    wrong_promotion[-1]["known_good"] = sha256(b"not-the-running-generation")
    assert_rejected(
        "promotion of the wrong generation",
        lambda: check_trace_chain(wrong_promotion, final, oracle),
    )

    roots = retained_roots(records, final)
    collect = dict(records[-1])
    collect.update(
        action="collect",
        commit="none",
        target_slot=None,
        sequence_after=collect["sequence_before"],
    )
    for root in roots:
        retained_collect = dict(collect, generation_root=root)
        assert_rejected(
            "collection of a retained root through trace dispatch",
            lambda retained_collect=retained_collect: validate_record(
                retained_collect, oracle, roots
            ),
        )
    validate_record(dict(collect, generation_root=sha256(b"orphan-object")), oracle, roots)

    conflict = dict(final)
    conflict["known_good"] = sha256(b"conflicting-known-good")
    assert_rejected(
        "equal-sequence divergent BootState slots",
        lambda: select_state([final, conflict]),
    )
    malformed_trace_corpus(records[0]["raw"])

    print(
        f"bootstate trace check: {len(records)} seL4 durable transitions conform "
        "to the M5.6a/M5.6b model; malformed records, stalled attempt, wrong "
        "commit/sequence/root/promotion, and retained-root collection rejected"
    )


if __name__ == "__main__":
    main()
