#!/usr/bin/env python3
"""Build the product seL4 generation twice and verify identical admitted bytes."""

from __future__ import annotations

import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD = ROOT / "scripts" / "build" / "build-generation.py"
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from harness import load_script  # noqa: E402

CHECK = load_script("check_generation", "check/check-generation.py")


def fail(message: str) -> None:
    raise SystemExit(f"generation determinism check: {message}")


def build(output: Path, target_dir: Path) -> None:
    environment = os.environ.copy()
    environment["SLIME_TARGET_PROFILE"] = "aarch64-sel4-qemu-virt"
    environment["SLIME_SEL4_MANIFEST"] = "sel4"
    environment["CARGO_TARGET_DIR"] = str(target_dir)
    process = subprocess.run(
        [str(BUILD), str(output)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    sys.stdout.write(process.stdout)
    if process.returncode != 0:
        fail(f"builder exited with status {process.returncode}")


def compare(first: Path, second: Path, name: str) -> bytes:
    first_bytes = (first / name).read_bytes()
    second_bytes = (second / name).read_bytes()
    if first_bytes != second_bytes:
        fail(f"two identical builds produced different {name} bytes")
    return first_bytes


def rust_verdict(generation: bytes, scratch: Path, label: str) -> str:
    """What `Generation::decode` says about these bytes, by `DecodeError` name.

    `slime-root` reads a generation linked into its own image, so no host gate
    could previously reach the Rust validator with chosen bytes. Both readers
    must refuse a forged budget, and B77 asks for a *distinct* reason from each,
    so the Python answer alone is not evidence about the decoder.
    """
    blob = scratch / f"{label}.bin"
    blob.write_bytes(generation)
    process = subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "boot-contracts",
            "--example",
            "admit_generation",
            "--",
            str(blob),
        ],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"the Rust decoder failed to run on {label}: {process.stdout.strip()}")
    return process.stdout.strip().splitlines()[-1]


def undeclarable_cpu_budget_refused(generation: bytes, scratch: Path) -> int:
    """B77: a nonzero `budget_us`/`period_us` must be refused, not ignored.

    The builder writes both fields zero, so no fixture can exercise the guard;
    the only way to reach it is to forge the value a foreign producer could
    declare. Both fields are mutated independently, because one predicate
    covering both would still pass if only one were checked.

    The mutation is *resealed*. `check_generation` verifies the identity hash
    (`BadGenerationHash`) before it ever reaches the schedule table, so a bare
    byte flip is refused for the wrong reason and would leave the real
    predicate untested while this arm still looked green -- the failure mode
    `check-component-spec.py` documents. Recomputing the identity is what makes
    this a test of the schedule rule rather than of the hash.

    Both readers are asserted, with the distinct reason B77 asks for: the host
    oracle must say `UndeclarableCpuBudget` and the Rust decoder must say
    `NonZeroReserved`. The unmutated generation is checked to be admitted by
    both first, so an arm cannot pass by tripping a guard the baseline trips
    too.
    """
    schedules = int.from_bytes(
        generation[
            CHECK.GENERATION_HEADER_SCHEDULE_COUNT_OFFSET : CHECK.GENERATION_HEADER_SCHEDULE_COUNT_END
        ],
        "little",
    )
    if schedules == 0:
        fail("the product generation declares no schedule, so B77's guard is unreachable")
    table = int.from_bytes(
        generation[
            CHECK.GENERATION_HEADER_SCHEDULE_OFFSET_OFFSET : CHECK.GENERATION_HEADER_SCHEDULE_OFFSET_END
        ],
        "little",
    )
    record = table + 0 * CHECK.GENERATION_SCHEDULE.size
    # The baseline both readers start from. Without this, an arm could be
    # refused for a reason the unmutated generation shares and still pass.
    baseline = rust_verdict(generation, scratch, "baseline")
    if baseline != "admitted":
        fail(f"the Rust decoder refused the unmutated product generation: {baseline}")
    arms = (
        ("budget_us", CHECK.GENERATION_SCHEDULE_BUDGET_US_OFFSET),
        ("period_us", CHECK.GENERATION_SCHEDULE_PERIOD_US_OFFSET),
    )
    for field, field_offset in arms:
        at = record + field_offset
        if generation[at : at + 8] != bytes(8):
            fail(f"the product generation already declares a nonzero {field}")
        forged = bytearray(generation)
        forged[at : at + 8] = (50_000).to_bytes(8, "little")
        resealed = CHECK.generation_identity(bytes(forged))
        forged[
            CHECK.GENERATION_HEADER_IDENTITY_OFFSET : CHECK.GENERATION_HEADER_IDENTITY_END
        ] = resealed
        candidate = bytes(forged)
        # The reseal must hold, or the arm below would be testing the hash.
        if CHECK.generation_identity(candidate) != resealed:
            fail(f"{field} mutation did not reseal")
        try:
            CHECK.check_generation(candidate)
        except CHECK.CheckError as error:
            if str(error) != "UndeclarableCpuBudget":
                fail(
                    f"a nonzero {field} was refused as {error}, not UndeclarableCpuBudget; "
                    "the mutation tripped a different guard and the B77 rule is untested"
                )
        else:
            fail(f"a generation declaring a nonzero {field} was admitted by the host oracle")
        verdict = rust_verdict(candidate, scratch, f"forged-{field}")
        if verdict != "refused NonZeroReserved":
            fail(
                f"the Rust decoder answered {verdict!r} for a nonzero {field}, not "
                "'refused NonZeroReserved'; the two readers disagree about B77"
            )
    return len(arms)


def main() -> None:
    with tempfile.TemporaryDirectory(prefix="slime-generation-determinism-") as directory:
        root = Path(directory)
        first = root / "first"
        second = root / "second"
        build(first, root / "target-first")
        build(second, root / "target-second")
        generation = compare(first, second, "generation.bin")
        first_generation_one = (first / "generation-1.bin").read_bytes()
        second_generation_one = (second / "generation-1.bin").read_bytes()
        if first_generation_one != generation or second_generation_one != generation:
            fail("generation-1.bin is not the independently written generation.bin alias")
        bootstore = compare(first, second, "boot-store.bin")
        checked_generation = CHECK.check_generation(generation)
        checked_store = CHECK.check_bootstore(bootstore)
        if checked_store["selected"]["identity"] != checked_generation["identity"]:
            fail("boot store did not select the independently admitted generation")
        # The admitted baseline above is the mutation's starting point, so no
        # arm can pass by tripping a guard the unmutated generation also trips.
        budget_arms = undeclarable_cpu_budget_refused(generation, root)

    print(
        "generation determinism check: two isolated builds forced the sel4 manifest "
        "and produced byte-identical generation.bin and boot-store.bin; each build's "
        "independently written generation-1.bin alias matched its generation, "
        "generation and boot store admission passed, and "
        f"{budget_arms} resealed nonzero-CPU-budget mutations were refused as "
        "UndeclarableCpuBudget"
    )


if __name__ == "__main__":
    main()
