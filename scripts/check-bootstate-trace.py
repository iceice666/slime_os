#!/usr/bin/env python3
"""M5.6c BootState model-implementation conformance check.

Boots the rollback power-cut scenario, collects the durable BootState
transition traces stage-0 and the generation-management service emit, and
validates each finite trace against the checked M5.6a/M5.6b state machines in
`contracts/bootstate/model/`.

Two conformance layers, neither a re-transcription of the model:

  * Abstract legality is decided by TLC over the real `BootState.tla` actions
    through the `TraceConformance` oracle. A record's durable post-state is
    accepted only when it is reachable in the model; a record that transfers
    control before the attempt decrement is durable has no reachable state and
    is rejected.
  * Concrete root binding maps the abstract roots the model does not carry onto
    the on-disk BootState identities. A promotion or collection against the
    wrong root is rejected here.

The check also proves the negative cases required by M5.6c: an attempt that was
not durably decremented, and a promotion or collection against the wrong root,
are all rejected; and that trace instrumentation stays bounded.
"""

from __future__ import annotations

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

from boot_contracts import (
    BOOTSTATE_SLOT_BYTES,
    BOOTSTATE_TRACE_MAX_LINE,
    BOOTSTATE_TRACE_VERSION,
    BOOTSTORE_CAPACITY,
    sha256,
)

ROOT = Path(__file__).resolve().parent.parent
MODEL_DIR = ROOT / "contracts" / "bootstate" / "model"
ORACLE_MODULE = "TraceConformance"
TRACE_PREFIX = "[bootstate-trace]"
TRACE_VERSION = BOOTSTATE_TRACE_VERSION
MAX_LINE = BOOTSTATE_TRACE_MAX_LINE
# One durable transition per boot, plus generous headroom, keeps the trace a
# bounded artifact rather than a new unbounded boot dependency.
MAX_TRACE_LINES_PER_BOOT = 4

CHECK_GENERATION_SPEC = importlib.util.spec_from_file_location(
    "check_generation", ROOT / "scripts" / "check-generation.py"
)
if CHECK_GENERATION_SPEC is None or CHECK_GENERATION_SPEC.loader is None:
    raise SystemExit("cannot load generation checker")
CHECK_GENERATION = importlib.util.module_from_spec(CHECK_GENERATION_SPEC)
CHECK_GENERATION_SPEC.loader.exec_module(CHECK_GENERATION)
check_bootstore = CHECK_GENERATION.check_bootstore

ACTIONS = {
    "consume-attempt",
    "promotion",
    "boot-known-good",
    "boot-exhausted-known-good",
    # `collect` is an adversarial checker-only action. `generation_root` names
    # the candidate identity; it is validated against observable retained
    # roots instead of the BootState transition oracle.
    "collect",
}
COMMITS = {"none", "after-attempt-commit", "health-promotion"}
SLOTS = {"A", "B"}
HEX32 = 64


class TraceError(Exception):
    pass


def run(
    arguments: list[str],
    *,
    environment: dict[str, str] | None = None,
    allow_failure: bool = False,
) -> str:
    process = subprocess.run(
        arguments,
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0 and not allow_failure:
        sys.stdout.write(process.stdout)
        raise SystemExit(process.returncode)
    return process.stdout


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
    return record


class Oracle:
    """Runs TLC over the real BootState model to decide abstract legality."""

    def __init__(self) -> None:
        tlc = shutil.which("tlc")
        if tlc is None:
            raise SystemExit("TLA+ tools not found; run inside `nix develop`")
        self.tlc = tlc
        self._cache: dict[tuple[str, str, int, int], bool] = {}

    def reachable(
        self, action: str, commit: str, attempts_before: int, attempts_after: int
    ) -> bool:
        key = (action, commit, attempts_before, attempts_after)
        if key in self._cache:
            return self._cache[key]
        config = (
            f'CONSTANT ObsAction = "{action}"\n'
            f'CONSTANT ObsCommit = "{commit}"\n'
            f"CONSTANT ObsAttemptsBefore = {attempts_before}\n"
            f"CONSTANT ObsAttemptsAfter = {attempts_after}\n"
            "SPECIFICATION Spec\n"
            "CONSTANTS\n"
            "    Generations = {gen_G1, gen_G2}\n"
            "    NoGeneration = gen_None\n"
            "    NoGraphRoot = graph_None\n"
            "    MaxAttempts = 2\n"
            "    MaxSequence = 5\n"
            "    MaxEpoch = 1\n"
            '    RequiredCut = "none"\n'
            "CHECK_DEADLOCK FALSE\n"
            "INVARIANT NoObservedReach\n"
        )
        with tempfile.TemporaryDirectory(prefix="slime-trace-tlc-") as temporary:
            config_path = Path(temporary) / "oracle.cfg"
            config_path.write_text(config)
            process = subprocess.run(
                [
                    self.tlc,
                    "-cleanup",
                    "-workers",
                    "1",
                    "-metadir",
                    str(Path(temporary) / "meta"),
                    "-config",
                    str(config_path),
                    f"{ORACLE_MODULE}.tla",
                ],
                cwd=MODEL_DIR,
                check=False,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
        combined = process.stdout
        # The witness invariant is violated exactly when the observed
        # post-state is reachable in the model.
        reachable = "NoObservedReach is violated" in combined
        if not reachable and "Error:" in combined and "Invariant" not in combined:
            sys.stdout.write(combined)
            raise SystemExit("oracle run failed unexpectedly")
        self._cache[key] = reachable
        return reachable


def retained_roots(store: dict) -> set[bytes]:
    """Roots observable in the on-disk BootState record.

    Running, rollback, staged, and additional persistent roots are kernel-only
    manager state and cannot be reconstructed from this disk artifact; those
    categories remain covered by the M5.6b model and kernel GC tests.
    """
    state = store["state"]
    roots = {state["known_good"], state["generation_root"], state["state_root"]}
    if state["pending"] is not None:
        roots.add(state["pending"])
    return roots


def check_concrete(record: dict, store: dict) -> None:
    """Bind the record's abstract roots to the on-disk BootState identities."""
    state = store["state"]
    valid_identities = {generation["identity"] for generation in store["generations"]}

    if record["generation_root"] != state["generation_root"]:
        raise TraceError("generation_root does not match the on-disk BootState")
    if record["state_root"] != state["state_root"]:
        raise TraceError("state_root does not match the on-disk BootState")

    if record["action"] == "collect":
        # The collected object is named by generation_root; a collection
        # against any retained root is rejected.
        raise TraceError("collect handled separately")

    if record["action"] == "promotion":
        # A promotion sets known-good to the previously running pending
        # generation and clears pending. Any other valid directory identity is
        # still the wrong generation.
        if state["pending"] is None:
            raise TraceError("promotion has no pending generation to promote")
        if record["known_good"] != state["pending"]:
            raise TraceError("promotion does not select the running pending generation")
        if record["pending"] is not None:
            raise TraceError("promotion does not clear pending")
        if record["known_good"] not in valid_identities:
            raise TraceError("promotion known-good is not a valid generation")
        if record["attempts_after"] != 0:
            raise TraceError("promotion must clear pending attempts")
        if record["attempts_before"] != state["remaining_attempts"]:
            raise TraceError("promotion attempts_before does not match pending state")
    else:
        if record["known_good"] != state["known_good"]:
            raise TraceError("known_good does not match the on-disk BootState")
        if record["pending"] != state["pending"]:
            raise TraceError("pending does not match the on-disk BootState")


def check_collect(candidate: bytes, store: dict) -> None:
    """Reject collecting an object reachable from any retained root."""
    if candidate in retained_roots(store):
        raise TraceError("collection targets a retained root")


def check_transition_shape(record: dict) -> None:
    action = record["action"]
    selected = record["selected_slot"]
    target = record["target_slot"]
    sequence_before = record["sequence_before"]
    sequence_after = record["sequence_after"]

    if action == "consume-attempt":
        if target is None or target == selected:
            raise TraceError(f"{action} must target the other BootState slot")
        if sequence_after != sequence_before + 1:
            raise TraceError(f"{action} must advance the durable sequence by one")
    elif action == "promotion":
        # Promotion traces are accepted only as adversarial checker inputs
        # until the kernel has a real durable slot-write path for confirmation.
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


def validate_record(record: dict, store: dict, oracle: Oracle) -> None:
    check_transition_shape(record)
    if record["action"] == "collect":
        check_collect(record["generation_root"], store)
        return
    if not oracle.reachable(
        record["action"],
        record["commit"],
        record["attempts_before"],
        record["attempts_after"],
    ):
        raise TraceError(
            f"{record['action']} post-state is not reachable in the model"
        )
    check_concrete(record, store)


def collect_traces(output: str) -> list[dict]:
    records = []
    for line in output.splitlines():
        line = line.rstrip("\r")
        if line.startswith(TRACE_PREFIX):
            records.append(parse_trace_line(line))
    return records


def bootstore_bytes(image: Path) -> bytes:
    extracted = Path("/tmp/slime-os-trace-boot-store.bin")
    extracted.unlink(missing_ok=True)
    subprocess.run(
        ["mcopy", "-o", "-i", str(image), "::/boot/boot-store.bin", str(extracted)],
        check=True,
    )
    data = extracted.read_bytes()
    if len(data) != BOOTSTORE_CAPACITY:
        raise SystemExit("extracted boot store has unexpected size")
    return data


def run_scenario(image: Path) -> tuple[list[dict], dict]:
    image.unlink(missing_ok=True)
    kernel = ROOT / "kernel" / "target" / "x86_64-unknown-none" / "debug" / "slime_os-kernel"

    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "99"
    environment["SLIME_PENDING_GENERATION"] = "1"
    environment["SLIME_PENDING_ATTEMPTS"] = "2"
    run(
        [
            str(ROOT / "kernel" / "scripts" / "build-iso.sh"),
            str(kernel),
            str(image),
            "64",
        ],
        environment=environment,
    )

    store = check_bootstore(bootstore_bytes(image))
    all_records: list[dict] = []
    for _ in range(3):
        environment = os.environ.copy()
        environment["SLIME_BOOT_IMAGE"] = str(image)
        environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
        output = run(
            [
                str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
                str(kernel),
                "-display",
                "none",
            ],
            environment=environment,
            allow_failure=True,
        )
        records = collect_traces(output)
        if not records:
            raise SystemExit("a rollback boot emitted no BootState trace")
        if len(records) > MAX_TRACE_LINES_PER_BOOT:
            raise SystemExit(
                f"a boot emitted {len(records)} trace lines; bound is "
                f"{MAX_TRACE_LINES_PER_BOOT}"
            )
        all_records.extend(records)
    return all_records, store


def assert_rejected(description: str, action) -> None:
    try:
        action()
    except TraceError:
        return
    raise SystemExit(f"validator accepted {description}; expected rejection")


def main() -> None:
    image = Path(sys.argv[1] if len(sys.argv) > 1 else "/tmp/slime-os-bootstate-trace.img")
    oracle = Oracle()

    records, store = run_scenario(image)

    # 1. Every rollback_check scenario trace is accepted by the models.
    consume = [record for record in records if record["action"] == "consume-attempt"]
    if len(consume) < 2:
        raise SystemExit("expected at least two durable attempt decrements")
    for record in records:
        validate_record(record, store, oracle)

    # 2. A trace that transfers control before the attempt decrement is durable
    #    is rejected: the post-state (attempts unchanged) is unreachable.
    stalled = dict(consume[0])
    stalled["attempts_after"] = stalled["attempts_before"]
    assert_rejected(
        "an attempt that was not durably decremented",
        lambda: validate_record(stalled, store, oracle),
    )

    # 3a. A promotion against the wrong known-good root is rejected.
    wrong_promotion = dict(consume[0])
    wrong_promotion["action"] = "promotion"

    # Commit labels and durable sequences are part of the model-implementation
    # boundary, not decorative metadata.
    wrong_commit = dict(consume[0])
    wrong_commit["commit"] = "none"
    assert_rejected(
        "a consume-attempt with the wrong commit boundary",
        lambda: validate_record(wrong_commit, store, oracle),
    )
    wrong_sequence = dict(consume[0])
    wrong_sequence["sequence_after"] = wrong_sequence["sequence_before"]
    assert_rejected(
        "a durable transition that does not advance its sequence",
        lambda: validate_record(wrong_sequence, store, oracle),
    )
    wrong_promotion["commit"] = "health-promotion"
    wrong_promotion["attempts_after"] = 0
    wrong_promotion["pending"] = None
    wrong_promotion["known_good"] = sha256(b"not-a-real-generation")
    assert_rejected(
        "a promotion against the wrong known-good root",
        lambda: validate_record(wrong_promotion, store, oracle),
    )
    wrong_promotion_attempts = dict(consume[0])
    wrong_promotion_attempts["action"] = "promotion"
    wrong_promotion_attempts["commit"] = "health-promotion"
    wrong_promotion_attempts["attempts_before"] = 0
    wrong_promotion_attempts["attempts_after"] = 0
    wrong_promotion_attempts["known_good"] = store["state"]["pending"]
    wrong_promotion_attempts["pending"] = None
    assert_rejected(
        "a promotion with an unrelated attempt count",
        lambda: validate_record(wrong_promotion_attempts, store, oracle),
    )

    # 3b. A promotion whose roots disagree with the on-disk BootState is
    #     rejected even when the known-good identity itself is valid.
    wrong_root_promotion = dict(consume[0])
    wrong_root_promotion["action"] = "promotion"
    wrong_root_promotion["commit"] = "health-promotion"
    wrong_root_promotion["attempts_after"] = 0
    wrong_root_promotion["pending"] = None
    wrong_root_promotion["state_root"] = sha256(b"wrong-state-root")
    assert_rejected(
        "a promotion against the wrong state root",
        lambda: validate_record(wrong_root_promotion, store, oracle),
    )

    # 4. Collection records use the normal validator dispatch. A retained root
    # is rejected, while a genuinely unreachable object is accepted, proving
    # the check is not vacuous.
    collect = dict(consume[0])
    collect["action"] = "collect"
    collect["commit"] = "none"
    collect["target_slot"] = None
    collect["sequence_after"] = collect["sequence_before"]
    collect["generation_root"] = store["state"]["known_good"]
    assert_rejected(
        "a collection of the known-good root",
        lambda: validate_record(collect, store, oracle),
    )
    collect["generation_root"] = store["state"]["generation_root"]
    assert_rejected(
        "a collection of the generation root",
        lambda: validate_record(collect, store, oracle),
    )
    collect["generation_root"] = sha256(b"orphan-object")
    validate_record(collect, store, oracle)

    print(
        f"bootstate trace check: {len(records)} durable transitions conform to "
        "the M5.6a/M5.6b models; stalled-attempt, wrong-root promotion, and "
        "retained-root collection rejected"
    )


if __name__ == "__main__":
    main()
