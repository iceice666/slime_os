"""Run the pinned devloop distribution against this repository's toolchain.

devloop owns the requirements body, its semantic helpers, and the evidence
contracts; Slime OS owns only the choice to use them and the environment they
run in. This module is that environment, and nothing else: it does not decode a
payload, evaluate a predicate, or decide whether evidence is sufficient. Every
such answer comes from `devloop` itself, so there is no second implementation of
its semantics here.

Two facts shape it.

* devloop validates by compiling a trusted wrapper over its helpers with
  `zutai-cli compile --emit bin` and running that binary. It therefore needs the
  compiler, its standard library, its native runtime archive, and LLVM's `llc`
  and `clang`.

* This repository already builds `zutai-cli` from the `deps/zutai` submodule
  (`scripts/lib/zutai_cli.py`), and that revision is what every generated
  binding and contract check resolves against. Putting a second compiler on
  `PATH` would let a gate validate requirements with one Zutai and generate
  contracts with another. Instead the submodule's build is reused and its
  revision is asserted to equal devloop's own pin, because devloop's helper
  identity — and therefore every recorded evidence entry — is bound to that
  revision.

Validation is offline: no network, no Git history, no remote projection.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from functools import lru_cache
from pathlib import Path

from harness import ROOT
import zutai_cli

# Slime OS's consumer configuration: gate identities, their declared typed
# observations, the code closure a gate tests, and the approval program.
POLICY = ROOT / ".devloop" / "policy.json"
RUNTIME_ARCHIVE = zutai_cli.ZUTAI_ROOT / "target" / "release" / "libzutai_rt.a"


class DevloopError(Exception):
    """`devloop` exited non-zero; the message is its diagnostic."""


def _require(program: str, hint: str) -> str:
    resolved = shutil.which(program)
    if resolved is None:
        raise SystemExit(f"{program} is not on PATH: {hint}")
    return resolved


@lru_cache(maxsize=1)
def executable() -> str:
    return _require(
        "devloop",
        "enter the repository dev shell (`nix develop`), which provides the pinned release",
    )


@lru_cache(maxsize=1)
def pinned_revision() -> str:
    """The Zutai revision the installed devloop release was built against."""
    finished = subprocess.run(
        [
            _require("python3", "the dev shell provides it"),
            "-c",
            "import devloop.core as core; print(core.PIN)",
        ],
        capture_output=True,
        text=True,
        env=_environment(),
    )
    if finished.returncode:
        raise SystemExit(
            "cannot read devloop's compiler pin: "
            f"{(finished.stderr or finished.stdout).strip()}"
        )
    return finished.stdout.strip()


@lru_cache(maxsize=1)
def submodule_revision() -> str:
    finished = subprocess.run(
        ["git", "-C", str(zutai_cli.ZUTAI_ROOT), "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
    )
    if finished.returncode:
        raise SystemExit(
            "cannot read the deps/zutai submodule revision: "
            f"{(finished.stderr or finished.stdout).strip()}"
        )
    return finished.stdout.strip()


def _runtime_archive() -> Path:
    """The native runtime archive devloop links its validators against."""
    if RUNTIME_ARCHIVE.exists():
        return RUNTIME_ARCHIVE
    finished = subprocess.run(
        [
            "cargo",
            "build",
            "--release",
            "-q",
            "--manifest-path",
            str(zutai_cli.ZUTAI_MANIFEST),
            "-p",
            "zutai-rt",
        ],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    if finished.returncode or not RUNTIME_ARCHIVE.exists():
        raise SystemExit(
            "cannot build the Zutai native runtime archive devloop links against: "
            f"{(finished.stderr or finished.stdout).strip()}"
        )
    return RUNTIME_ARCHIVE


@lru_cache(maxsize=1)
def _environment() -> dict[str, str]:
    """devloop's environment: the submodule compiler, its stdlib, and LLVM."""
    compiler = zutai_cli.binary()
    environment = dict(os.environ)
    environment["PATH"] = os.pathsep.join([str(compiler.parent), environment.get("PATH", "")])
    environment["ZUTAI_STDLIB_ROOT"] = str(zutai_cli.STDLIB)
    environment["ZUTAI_RUNTIME_ARCHIVE"] = str(_runtime_archive())
    for variable, program in (("ZUTAI_LLC", "llc"), ("ZUTAI_CLANG", "clang")):
        if variable not in environment:
            resolved = shutil.which(program)
            if resolved is not None:
                environment[variable] = resolved
    return environment


def check_toolchain() -> list[str]:
    """Findings that would make any devloop result untrustworthy."""
    pinned, submodule = pinned_revision(), submodule_revision()
    if pinned != submodule:
        return [
            "devloop pins Zutai "
            f"{pinned} but deps/zutai is {submodule}: a helper identity recorded "
            "against one compiler cannot be validated with another; move the "
            "submodule or the devloop pin so they agree"
        ]
    return []


def run(*arguments: str, cwd: Path = ROOT) -> str:
    """Invoke the pinned devloop with this repository's policy environment."""
    finished = subprocess.run(
        [executable(), *arguments],
        cwd=cwd,
        capture_output=True,
        text=True,
        env=_environment(),
    )
    if finished.returncode:
        raise DevloopError((finished.stderr or finished.stdout).strip())
    return finished.stdout


def validate(identity: str, *, policy: Path = POLICY, cwd: Path = ROOT) -> dict[str, object]:
    """Validate one item's stored requirements body and its recorded semantics."""
    return json.loads(run("validate", identity, "--policy", str(policy), cwd=cwd))


def render(item: dict[str, object], cwd: Path = ROOT) -> str:
    """The readable Markdown a projection consumer would publish for an item."""
    finished = subprocess.run(
        [executable(), "render"],
        cwd=cwd,
        input=json.dumps(item),
        capture_output=True,
        text=True,
        env=_environment(),
    )
    if finished.returncode:
        raise DevloopError((finished.stderr or finished.stdout).strip())
    return finished.stdout


def main(arguments: list[str]) -> int:
    """Forward a devloop invocation into this repository's environment.

    `just devloop …` is how an operator admits, starts, gates, or completes an
    item here: the compiler, its standard library, and its runtime archive come
    from the same `deps/zutai` build every contract check uses, and the pin is
    asserted before anything runs.
    """
    findings = check_toolchain()
    if findings:
        for finding in findings:
            print(f"devloop: {finding}")
        return 1
    finished = subprocess.run(
        [executable(), *(arguments or ["--help"])], cwd=ROOT, env=_environment()
    )
    return finished.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
