#!/usr/bin/env python3

"""Judge closure sources that carry no in-tree digest, and the resolver that derives their identities.

The expectation lives in `scripts/lib/closure_derivation_exam.py`. This script runs the repository's
closure generator and image builder as commands, and imports neither. They must offer:

    python3 scripts/generate/generate-system-image-closures.py            write the committed sources
    python3 scripts/generate/generate-system-image-closures.py --check    refuse a stale source
    python3 scripts/generate/generate-system-image-closures.py --resolve NAME --output PATH --report REPORT
        write the resolved v2 closure for NAME to PATH, and a `contracts/closure-resolution/v1` record to
        REPORT with `name`, `derivationId` (the resolved closure's identity) and `dirty`
    python3 scripts/build/build-system-image.py contracts/system-image-closure/v3/closures/NAME.zti OUT
        build NAME and write OUT/build-result.zti, a `SystemImageBuildResult` that
        `contracts/system-image-closure/v2`'s `decodeSystemImageBuildResult` accepts, carrying
        `closureIdentity` and `dirty`

Both records cross a process boundary, so this script reads them only through those decoders.

Steps:

1. The judges' controls hold.
2. Every pinned closure has a source under `contracts/system-image-closure/v3/closures/`, no source
   holds a digest or an identity field, and no resolved v2 closure stays committed.
3. In a scratch checkout of `HEAD` with its submodules cloned locally, each sampled closure resolves
   twice to the same derivation id, both resolutions report `dirty: false`, and its resolved closure passes the
   independent checks: its recomputed identity is the derivation id, and every artifact's identity is
   the digest of the files git lists.
4. In the same checkout, a dirty input moves derivation ids exactly where it should:
   * a tracked edit to `components/applications/ros2-demo-publisher` moves `sel4-zenoh` and marks it
     dirty, and leaves `sel4-channel` unchanged and clean;
   * an untracked file under `components/lib` moves both and marks both dirty;
   * an ignored file under `components/lib` moves neither and marks both dirty;
   * a comment appended to a recipe under `just/` moves neither and marks neither dirty.
   After the tracked edit and after the recipe edit, the generator rewrites no file under
   `contracts/system-image-closure/` or `contracts/system-test-run/`, and `--check` passes.
5. In this checkout, `sel4-channel` builds clean with `dirty: false`, and again with an ignored file
   under `components/lib` with `dirty: true`. Both build results carry the derivation id the resolver
   reports for the clean tree. The ignored file is removed afterwards.

`--controls-only` runs step 1. `--skip-builds` stops after step 4.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import json
import os
import shutil
import subprocess
import tempfile
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import closure_derivation_exam as exam
import zutai_cli
from harness import ROOT

SUBMODULES = ("deps/rust-sel4", "deps/zutai")
ZUTAI_BINARY = Path("deps/zutai/target/release/zutai-cli")
GENERATOR = "scripts/generate/generate-system-image-closures.py"
BUILDER = "scripts/build/build-system-image.py"
SOURCE_DIR = Path("contracts/system-image-closure/v3/closures")
APPLICATION_EDIT = Path("components/applications/ros2-demo-publisher/src/main.rs")
UNTRACKED = Path("components/lib/src/closure_derivation_exam_untracked.rs")
IGNORED = Path("components/lib/target/closure-derivation-exam")
RECIPE_EDIT = Path("just/contracts.just")
TIMEOUT_SECONDS = 3600


class Findings(list):
    def add(self, step: str, messages: list[str]) -> None:
        self.extend(f"{step}: {message}" for message in messages)


def git(*arguments: str, cwd: Path = ROOT) -> str:
    return subprocess.run(["git", *arguments], cwd=cwd, check=True, capture_output=True, text=True).stdout


def run(checkout: Path, *arguments: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [_sys.executable, *arguments], cwd=checkout, capture_output=True, text=True, timeout=TIMEOUT_SECONDS,
        env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"},
    )


def tail(result: subprocess.CompletedProcess[str]) -> str:
    lines = (result.stderr or result.stdout).strip().splitlines()
    return lines[-1][:300] if lines else ""


@contextmanager
def scratch_checkout() -> Iterator[Path]:
    """A worktree of `HEAD`, its submodules cloned from this checkout at their pins, sharing the Zutai cache.

    Each submodule is a plain clone of this checkout's submodule, checked out at the gitlink's commit. No
    `submodule.*` setting is written: a worktree shares this repository's configuration, and a URL written
    there would outlive the scratch checkout.
    """
    parent = Path(tempfile.mkdtemp(prefix="slime-closure-derivations-"))
    scratch = parent / "checkout"
    git("worktree", "add", "--detach", "--quiet", str(scratch), "HEAD")
    try:
        for submodule in SUBMODULES:
            pin = git("rev-parse", f"HEAD:{submodule}").strip()
            git("clone", "--quiet", "--shared", "--no-checkout", str(ROOT / submodule), str(scratch / submodule))
            git("checkout", "--quiet", "--detach", pin, cwd=scratch / submodule)
            if git("rev-parse", "HEAD", cwd=scratch / submodule).strip() != pin:
                raise SystemExit(f"closure derivations: the scratch {submodule} is not at its pin")
        (scratch / ZUTAI_BINARY).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(zutai_cli.binary(), scratch / ZUTAI_BINARY)
        cache = ROOT / "build" / "zutai-cache"
        cache.mkdir(parents=True, exist_ok=True)
        (scratch / "build").mkdir()
        (scratch / "build" / "zutai-cache").symlink_to(cache)
        yield scratch
    finally:
        git("worktree", "remove", "--force", str(scratch))
        shutil.rmtree(parent, ignore_errors=True)


def step_sources(findings: Findings) -> None:
    sources = {path.stem: path.read_text(encoding="utf-8") for path in sorted((ROOT / SOURCE_DIR).glob("*.zti"))}
    resolved = sorted(path.name for path in (ROOT / exam.RESOLVED_V2_DIR).glob("*.zti"))
    findings.add("sources", exam.source_findings(sources, resolved))
    print(f"closure derivations: {len(sources)} source(s), {len(resolved)} committed resolved v2 closure(s)")


def resolve(checkout: Path, name: str, findings: Findings, step: str, *, judge: bool) -> dict | None:
    """Resolve one closure; with `judge`, check the resolved closure against the checkout's files."""
    with tempfile.TemporaryDirectory(prefix="slime-resolved-") as directory:
        output = Path(directory) / f"{name}.zti"
        record = Path(directory) / "report.zti"
        result = run(checkout, GENERATOR, "--resolve", name, "--output", str(output), "--report", str(record))
        if result.returncode != 0:
            findings.add(step, [f"{name}: --resolve exited {result.returncode}: {tail(result)}"])
            return None
        if not record.is_file():
            findings.add(step, [f"{name}: --resolve wrote no report to --report"])
            return None
        try:
            report = exam.decode_record(checkout / exam.REPORT_SCHEMA, "decodeResolutionReport", "ResolutionReport", record)
        except ValueError as error:
            findings.add(step, [f"{name}: {error}"])
            return None
        raw = json.loads(zutai_cli.evaluate("json", record, input_path=record, env_var="SLIME_UNUSED"))
        problems = exam.report_findings(name, report, raw)
        if problems:
            findings.add(step, problems)
            return None
        if judge:
            if not output.is_file():
                findings.add(step, [f"{name}: --resolve wrote no closure to --output"])
                return None
            value = json.loads(zutai_cli.evaluate("json", output, input_path=output, env_var="SLIME_UNUSED"))
            findings.add(step, exam.resolved_findings(name, value, report, checkout))
        return report


def step_resolver(scratch: Path, findings: Findings) -> dict[str, dict]:
    clean: dict[str, dict] = {}
    for name in exam.SAMPLE:
        first = resolve(scratch, name, findings, "resolver", judge=True)
        second = resolve(scratch, name, findings, "resolver", judge=False)
        if first is None or second is None:
            continue
        if first["derivationId"] != second["derivationId"]:
            findings.add("resolver", [f"{name}: two resolutions gave different derivation ids"])
        if first["dirty"] or second["dirty"]:
            findings.add("resolver", [f"{name}: a clean checkout resolved dirty"])
        clean[name] = first
    print(f"closure derivations: {len(clean)} of {len(exam.SAMPLE)} sampled closure(s) resolved and judged")
    return clean


def restore(scratch: Path) -> None:
    git("checkout", "--quiet", "--", ".", cwd=scratch)
    git("clean", "--quiet", "-fd", "--", "components", "just", "contracts", cwd=scratch)
    marker = scratch / IGNORED
    if marker.exists():
        marker.unlink()


def unchanged_derived(scratch: Path, findings: Findings, step: str) -> None:
    result = run(scratch, GENERATOR)
    if result.returncode != 0:
        findings.add(step, [f"the generator exited {result.returncode}: {tail(result)}"])
        return
    moved = git("status", "--porcelain", "--untracked-files=all", "--",
                "contracts/system-image-closure", "contracts/system-test-run", cwd=scratch).splitlines()
    if moved:
        findings.add(step, [f"the generator rewrote {len(moved)} committed file(s), e.g. {moved[0][3:]}"])
    check = run(scratch, GENERATOR, "--check")
    if check.returncode != 0:
        findings.add(step, [f"--check exited {check.returncode}: {tail(check)}"])


def step_sensitivity(scratch: Path, clean: dict[str, dict], findings: Findings) -> None:
    pair = ("sel4-channel", "sel4-zenoh")
    if not all(name in clean for name in pair):
        findings.add("sensitivity", ["the clean resolutions it compares against are missing"])
        return

    def append(path: Path, text: str) -> None:
        target = scratch / path
        target.write_text(target.read_text(encoding="utf-8") + text, encoding="utf-8")

    def create(path: Path, text: str) -> None:
        target = scratch / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")

    cases = (
        ("tracked edit", lambda: append(APPLICATION_EDIT, "\n// closure derivation exam\n"),
         {"sel4-channel": (False, False), "sel4-zenoh": (True, True)}, True),
        ("untracked file", lambda: create(UNTRACKED, "// closure derivation exam\n"),
         {"sel4-channel": (True, True), "sel4-zenoh": (True, True)}, False),
        ("ignored file", lambda: create(IGNORED, "closure derivation exam\n"),
         {"sel4-channel": (False, True), "sel4-zenoh": (False, True)}, False),
        ("recipe comment", lambda: append(RECIPE_EDIT, "\n# closure derivation exam\n"),
         {"sel4-channel": (False, False), "sel4-zenoh": (False, False)}, True),
    )
    for label, mutate, expected, check_derived in cases:
        mutate()
        try:
            for name, (moves, dirty) in expected.items():
                report = resolve(scratch, name, findings, f"sensitivity ({label})", judge=False)
                if report is None:
                    continue
                moved = report["derivationId"] != clean[name]["derivationId"]
                if moved != moves:
                    findings.add(f"sensitivity ({label})", [f"{name}: derivation id {'moved' if moved else 'did not move'}, expected it {'to move' if moves else 'to stay'}"])
                if report["dirty"] != dirty:
                    findings.add(f"sensitivity ({label})", [f"{name}: dirty is {report['dirty']}, expected {dirty}"])
            if check_derived:
                unchanged_derived(scratch, findings, f"sensitivity ({label})")
        finally:
            restore(scratch)
    print(f"closure derivations: {len(cases)} dirty-input case(s) resolved")


def build(output: Path) -> dict:
    result = run(ROOT, BUILDER, str(SOURCE_DIR / "sel4-channel.zti"), str(output))
    if result.returncode != 0:
        return {"error": f"the builder exited {result.returncode}: {tail(result)}"}
    record = output / "build-result.zti"
    if not record.is_file():
        return {"error": "the builder wrote no build-result.zti"}
    try:
        return exam.decode_record(ROOT / exam.CLOSURE_SCHEMA, "decodeSystemImageBuildResult", "SystemImageBuildResult", record)
    except ValueError as error:
        return {"error": str(error)}


def step_builds(findings: Findings) -> None:
    clean = resolve(ROOT, "sel4-channel", findings, "builds", judge=False)
    if clean is None:
        return
    marker = ROOT / IGNORED
    if marker.exists():
        findings.add("builds", [f"{IGNORED} already exists; remove it before the exam"])
        return
    with tempfile.TemporaryDirectory(prefix="slime-derivation-build-") as directory:
        for label, dirty in (("clean", False), ("ignored file", True)):
            if dirty:
                marker.parent.mkdir(parents=True, exist_ok=True)
                marker.write_text("closure derivation exam\n", encoding="utf-8")
            try:
                result = build(Path(directory) / label.replace(" ", "-"))
            finally:
                if dirty and marker.exists():
                    marker.unlink()
            if "error" in result:
                findings.add("builds", [f"{label}: {result['error']}"])
                continue
            findings.add("builds", exam.build_result_findings(label, result, clean["derivationId"], dirty))
    print("closure derivations: sel4-channel built clean and with an ignored input file")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--controls-only", action="store_true", help="run only the judges' controls")
    parser.add_argument("--skip-builds", action="store_true", help="stop before the two image builds")
    arguments = parser.parse_args()
    results = exam.controls(ROOT)
    failed = [name for name, passed in results if not passed]
    if failed:
        print(f"closure derivations: FAIL: controls did not hold: {', '.join(failed)}", file=_sys.stderr)
        return 1
    print(f"closure derivations: {len(results)} controls held")
    if arguments.controls_only:
        return 0
    if git("status", "--porcelain", "--untracked-files=no").strip():
        print("closure derivations: FAIL: commit or stash tracked changes; the exam judges HEAD", file=_sys.stderr)
        return 1
    findings = Findings()
    step_sources(findings)
    with scratch_checkout() as scratch:
        clean = step_resolver(scratch, findings)
        step_sensitivity(scratch, clean, findings)
    if not arguments.skip_builds:
        step_builds(findings)
    if findings:
        for item in findings:
            print(f"closure derivations: FAIL: {item}", file=_sys.stderr)
        return 1
    print(f"closure derivations: PASS: {len(exam.PINNED_CLOSURES)} pinned sources hold no digest; "
          f"{len(exam.SAMPLE)} sampled closures resolve to their independently computed identities; "
          f"4 dirty-input cases move exactly the expected derivation ids; {len(results)} controls")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
