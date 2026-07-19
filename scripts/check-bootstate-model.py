#!/usr/bin/env python3

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MODEL_DIR = ROOT / "contracts" / "bootstate" / "model"
MODULE = "BootState.tla"
MAIN_CONFIG = MODEL_DIR / "BootState.cfg"
MUTATION_CONFIG = MODEL_DIR / "BootStateSkipAttempt.cfg"
SUCCESS = "BootState model passed: safety invariants, 9 cut witnesses, skip-attempt mutation rejected"
CUT_OPERATORS = (
    "CutBeforePending",
    "CutSlotA",
    "CutSlotB",
    "CutAfterPending",
    "CutAfterAttempt",
    "CutPromotion",
    "CutRollback",
    "CutSnapshot",
    "CutGc",
)


def run(arguments: list[str]) -> subprocess.CompletedProcess[str]:
    process = subprocess.run(
        arguments,
        cwd=MODEL_DIR,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return process


def output(process: subprocess.CompletedProcess[str]) -> str:
    return process.stdout + process.stderr


def require_success(process: subprocess.CompletedProcess[str]) -> None:
    if process.returncode != 0:
        sys.stderr.write(output(process))
        raise SystemExit(process.returncode)


def require_invariant_failure(
    process: subprocess.CompletedProcess[str], invariant: str, label: str
) -> None:
    combined = output(process)
    if process.returncode == 0 or invariant not in combined:
        sys.stderr.write(combined)
        raise SystemExit(f"{label} did not fail on {invariant}")


def tlc_command(tlc: str, config: Path, metadir: Path) -> list[str]:
    return [
        tlc,
        "-cleanup",
        "-workers",
        "1",
        "-metadir",
        str(metadir),
        "-config",
        str(config),
        MODULE,
    ]


def main() -> None:
    tlc = shutil.which("tlc")
    tlasany = shutil.which("tlasany")
    if tlc is None or tlasany is None:
        raise SystemExit("TLA+ tools not found; run inside `nix develop`")

    require_success(run([tlasany, MODULE]))

    declarations = MAIN_CONFIG.read_text()
    with tempfile.TemporaryDirectory(prefix="slime-bootstate-tlc-") as temporary:
        temporary_root = Path(temporary)
        require_success(
            run(tlc_command(tlc, MAIN_CONFIG, temporary_root / "positive"))
        )

        mutation = run(
            tlc_command(tlc, MUTATION_CONFIG, temporary_root / "mutation")
        )
        require_invariant_failure(
            mutation,
            "PendingAttemptConsumedBeforeTransfer",
            "skip-attempt mutation",
        )

        for operator in CUT_OPERATORS:
            witness_config = temporary_root / f"{operator}.cfg"
            witness_config.write_text(
                declarations.replace(
                    'RequiredCut = "none"', f"RequiredCut <- {operator}"
                )
                + "\nINVARIANT CutWitnessMissing\n"
            )
            witness = run(
                tlc_command(tlc, witness_config, temporary_root / operator)
            )
            require_invariant_failure(
                witness, "CutWitnessMissing", f"{operator} witness"
            )

    print(SUCCESS)


if __name__ == "__main__":
    main()
