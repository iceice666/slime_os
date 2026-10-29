#!/usr/bin/env python3

"""Judge test-run records that name their image closure (`contracts/system-test-run/v2`).

The expectation lives in `scripts/lib/test_run_reference_exam.py`. This script runs the repository's
own generators and checker as commands, in a scratch checkout of `HEAD`, and imports none of them:

1. The judges' controls must hold.
2. Every version 2 record decodes, declares the schema's fields and names the closure the exam pins
   for its run. No version 1 record remains.
3. Closure inputs change without touching a run. In the scratch checkout, one line is appended to
   `components/lib/src/lib.rs`, an input of every closure, and
   `python3 scripts/generate/generate-system-image-closures.py` regenerates the closures. No file under
   `contracts/system-test-run/` and no negative build case may change. Then
   `python3 scripts/generate/generate-system-test-runs.py --check` and
   `python3 scripts/check/check-system-test-run.py` must still pass.
4. `python3 scripts/check/check-system-test-run.py` refuses a run that names an unknown closure, a run
   that names another plane's closure, and a run that carries `imageClosureIdentity` again.

Each step reports every finding, so a run against version 1 records shows what remains. `--controls-only`
runs step 1.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import json
import os
import re
import shutil
import subprocess
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import component_sdk
import test_run_reference_exam as exam
import zutai_cli
from harness import ROOT

SUBMODULES = ("deps/rust-sel4", "deps/zutai")
ZUTAI_BINARY = Path("deps/zutai/target/release/zutai-cli")
INPUT_EDIT = Path("components/lib/src/lib.rs")
COMMAND_TIMEOUT_SECONDS = 3600


class Findings:
    def __init__(self) -> None:
        self.items: list[str] = []

    def add(self, step: str, messages: list[str]) -> None:
        self.items.extend(f"{step}: {message}" for message in messages)


def git(*arguments: str, cwd: Path = ROOT) -> str:
    return subprocess.run(["git", *arguments], cwd=cwd, check=True, capture_output=True, text=True).stdout


def command(scratch: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_sys.executable, *arguments], cwd=scratch, capture_output=True, text=True,
        timeout=COMMAND_TIMEOUT_SECONDS, env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
    )


@contextmanager
def scratch_checkout() -> Iterator[Path]:
    """A worktree of `HEAD` with the submodules the closure generator reads, sharing the Zutai cache."""
    parent = Path(tempfile.mkdtemp(prefix="slime-test-run-references-"))
    scratch = parent / "checkout"
    git("worktree", "add", "--detach", "--quiet", str(scratch), "HEAD")
    try:
        for submodule in SUBMODULES:
            pinned = git("rev-parse", f"HEAD:{submodule}").strip()
            current = git("rev-parse", "HEAD", cwd=ROOT / submodule).strip()
            if pinned != current:
                raise SystemExit(f"HEAD pins {submodule} at {pinned[:12]}, but this checkout has {current[:12]}")
            shutil.copytree(ROOT / submodule, scratch / submodule, ignore=component_sdk.COPY_IGNORE, dirs_exist_ok=True)
        binary = zutai_cli.binary()
        (scratch / ZUTAI_BINARY).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(binary, scratch / ZUTAI_BINARY)
        cache = ROOT / "build" / "zutai-cache"
        cache.mkdir(parents=True, exist_ok=True)
        (scratch / "build").mkdir()
        (scratch / "build" / "zutai-cache").symlink_to(cache)
        yield scratch
    finally:
        git("worktree", "remove", "--force", str(scratch))
        shutil.rmtree(parent, ignore_errors=True)


def decoded(path: Path) -> object:
    """A `.zti` record as data, through the pinned Zutai toolchain."""
    try:
        return json.loads(zutai_cli.evaluate("json", path, input_path=path, env_var="SLIME_UNUSED"))
    except (zutai_cli.ZutaiError, json.JSONDecodeError) as error:
        return f"undecodable: {error}"


def step_records(scratch: Path, findings: Findings) -> None:
    schema = scratch / exam.SCHEMA.relative_to(ROOT)
    if schema.is_file():
        findings.add("schema", exam.schema_findings(schema.read_text(encoding="utf-8")))
    else:
        findings.add("schema", [f"{exam.SCHEMA.relative_to(ROOT)} is missing"])
    runs = scratch / exam.RUNS_V2
    records = {path.stem: decoded(path) for path in sorted(runs.glob("*.zti"))} if runs.is_dir() else {}
    v1 = sorted(path.name for path in (scratch / exam.RUNS_V1).glob("*.zti"))
    closures = {path.stem for path in scratch.glob(exam.CLOSURE_GLOB)}
    findings.add("records", exam.judge_records(records, v1_records=v1, closures=closures))
    print(f"test-run references: {len(records)} version 2 record(s), {len(v1)} version 1 record(s)")


def changed(scratch: Path, *paths: str) -> list[str]:
    status = git("status", "--porcelain", "--untracked-files=all", "--", *paths, cwd=scratch)
    return [line[3:] for line in status.splitlines()]


def step_input_change(scratch: Path, findings: Findings) -> None:
    target = scratch / INPUT_EDIT
    target.write_bytes(target.read_bytes() + b"\n// test-run reference exam: a closure input changed\n")
    result = command(scratch, "scripts/generate/generate-system-image-closures.py")
    if result.returncode != 0:
        findings.add("input-change", [f"generate-system-image-closures.py exited {result.returncode}: {result.stderr[-400:]}"])
        return
    moved = changed(scratch, "contracts/system-image-closure")
    records = [path for path in changed(scratch, "contracts/system-test-run")]
    negative = [path for path in moved if "/negative/" in path]
    closures = [path for path in moved if "/closures/" in path]
    digests = [path for path in scratch.glob(exam.CLOSURE_GLOB) if re.search(r'identity = "[0-9a-f]{64}"', path.read_text())]
    if not closures and digests:
        findings.add("input-change", [f"the edit of {INPUT_EDIT} moved no closure, so it proves nothing"])
    print(f"test-run references: the input change moved {len(closures)} closure(s), "
          f"{len(negative)} negative case(s) and {len(records)} test-run file(s)")
    if records:
        findings.add("input-change", [f"{len(records)} test-run file(s) changed, e.g. {records[0]}"])
    if negative:
        findings.add("input-change", [f"{len(negative)} negative build case(s) changed, e.g. {negative[0]}"])
    for arguments in (("scripts/generate/generate-system-test-runs.py", "--check"), ("scripts/check/check-system-test-run.py",)):
        result = command(scratch, *arguments)
        if result.returncode != 0:
            tail = (result.stderr or result.stdout).strip().splitlines()[-1:] or [""]
            findings.add("input-change", [f"{' '.join(arguments)} exited {result.returncode} after the change: {tail[0][:300]}"])


def restore(scratch: Path) -> None:
    git("checkout", "--quiet", "--", ".", cwd=scratch)
    git("clean", "--quiet", "-fd", "--", "contracts", cwd=scratch)


REFUSALS = {
    "unknown-closure": ("sel4-zenoh", lambda text: text.replace('imageClosure = "sel4-zenoh";', 'imageClosure = "sel4-no-such-closure";')),
    "another-planes-closure": ("sel4-zenoh", lambda text: text.replace('imageClosure = "sel4-zenoh";', 'imageClosure = "sel4-channel";')),
    "identity-again": ("sel4-zenoh", lambda text: text.replace(
        'imageClosure = "sel4-zenoh";', 'imageClosure = "sel4-zenoh";\n  imageClosureIdentity = "' + "0" * 64 + '";')),
}


def step_refusals(scratch: Path, findings: Findings) -> None:
    baseline = command(scratch, "scripts/check/check-system-test-run.py")
    if baseline.returncode != 0:
        findings.add("refusals", [f"check-system-test-run.py fails on the unmodified checkout ({baseline.returncode})"])
        return
    for name, (run, edit) in REFUSALS.items():
        path = scratch / exam.RUNS_V2 / f"{run}.zti"
        if not path.is_file():
            findings.add("refusals", [f"{name}: {path.relative_to(scratch)} is missing"])
            continue
        original = path.read_text(encoding="utf-8")
        edited = edit(original)
        if edited == original:
            findings.add("refusals", [f"{name}: {run} does not name its closure as the exam expects"])
            continue
        path.write_text(edited, encoding="utf-8")
        try:
            result = command(scratch, "scripts/check/check-system-test-run.py")
        finally:
            path.write_text(original, encoding="utf-8")
        if result.returncode == 0:
            findings.add("refusals", [f"{name}: check-system-test-run.py accepted the edited {run} record"])
    print(f"test-run references: {len(REFUSALS)} edited record(s) put to check-system-test-run.py")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--controls-only", action="store_true", help="run only the judges' controls")
    arguments = parser.parse_args()
    results = exam.controls()
    failed = [name for name, passed in results if not passed]
    if failed:
        print(f"test-run references: FAIL: controls did not hold: {', '.join(failed)}", file=_sys.stderr)
        return 1
    print(f"test-run references: {len(results)} controls held")
    if arguments.controls_only:
        return 0
    if git("status", "--porcelain", "--untracked-files=no").strip():
        print("test-run references: FAIL: commit or stash tracked changes; the exam judges HEAD", file=_sys.stderr)
        return 1
    findings = Findings()
    with scratch_checkout() as scratch:
        step_records(scratch, findings)
        step_input_change(scratch, findings)
        restore(scratch)
        step_refusals(scratch, findings)
    if findings.items:
        for item in findings.items:
            print(f"test-run references: FAIL: {item}", file=_sys.stderr)
        return 1
    print(f"test-run references: PASS: {len(exam.PINNED)} pinned runs name their closure; "
          f"a closure input change rewrote no run; {len(REFUSALS)} edited runs refused; {len(results)} controls")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
