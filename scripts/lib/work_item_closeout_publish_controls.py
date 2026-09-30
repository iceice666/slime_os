"""Closeout publication controls.

`check_controls` is offline: the record parser and the commit guard. The
`--checkout` case (`just closeout_checkout_check`) clones canonical main and is
explicit because it needs the network.
"""

from __future__ import annotations

import inspect
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

import devloop_observations
import work_item_closeout_publish as closeout
import work_item_retirement_publish as retirement_publish
from harness import ROOT
from work_item_retirement import RetirementError

_ITEM = "01a0ec61-d599-73ec-9e42-1ee6966420fd"
_RECORD = """---
schema: work-item/v2
id: {item}
state: open
devloop:
  schema: dev-spec/v1
  epoch: {epoch}
  evidence:{evidence}
---

# Title
"""
_ENTRY = """
  - acceptance: {acceptance}
    gate: just-target
    identity:
      requirements: r
      helper: h
      code: c
      policy: p
      inputs: {inputs}
      target: {target}
      image: {image}
      epoch: e
    passed: true
"""


def _record(epoch: str, entries: list[tuple[str, str, str, str]]) -> str:
    evidence = "".join(
        _ENTRY.format(acceptance=a, inputs=i, target=t, image=m) for a, i, t, m in entries
    ) or " []"
    return _RECORD.format(item=_ITEM, epoch=epoch, evidence=evidence)


def _refuse(action, description: str) -> None:
    try:
        action()
    except RetirementError:
        return
    raise RetirementError(f"closeout control accepted {description}")


def check_controls() -> None:
    # Parser: distinct contexts in first-appearance order; nothing before start.
    two = _record("e", [("a", "d1", "t1", "i1"), ("b", "d1", "t1", "i1"), ("c", "d2", "t2", "i2")])
    if closeout.evidence_contexts(two) != (("d1", "t1", "i1"), ("d2", "t2", "i2")):
        raise RetirementError("closeout control: evidence contexts not recovered")
    if closeout.evidence_contexts(_record("unstarted", [("a", "d1", "t1", "i1")])):
        raise RetirementError("closeout control: unstarted epoch yielded a context")
    if closeout.evidence_contexts(_record("e", [])):
        raise RetirementError("closeout control: empty evidence yielded a context")
    if closeout.evidence_contexts("---\nstate: open\n---\n# Prose\n"):
        raise RetirementError("closeout control: prose item yielded a context")

    # --apply outside the canonical main workflow is refused.
    with patch.dict(os.environ, {"GITHUB_ACTIONS": "true", "GITHUB_EVENT_NAME": "pull_request"}):
        _refuse(closeout.require_workflow_context, "a pull_request workflow context")

    # Commit guard: one bot commit on base that modifies only the closed items.
    with (
        tempfile.TemporaryDirectory(prefix="closeout-controls-") as temporary,
        patch.dict(
            os.environ,
            {
                "PATH": os.environ.get("PATH", ""),
                "GIT_CONFIG_NOSYSTEM": "1",
                "GIT_CONFIG_GLOBAL": os.devnull,
                "TZ": "UTC",
            },
            clear=True,
        ),
    ):
        root = Path(temporary)
        items = root / ".tasks" / "items"
        items.mkdir(parents=True)

        def git(*args: str) -> str:
            return closeout.command(root, "git", *args)

        def commit(message: str) -> str:
            git("add", "-A")
            git("commit", "-q", "-m", message)
            return git("rev-parse", "HEAD")

        git("init", "-q", "-b", "main")
        git("config", "user.name", "github-actions[bot]")
        git("config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
        git("config", "commit.gpgsign", "false")
        (items / f"{_ITEM}.md").write_text("state: open\n")
        (root / "README.md").write_text("x\n")
        base = commit("base")

        (items / f"{_ITEM}.md").write_text("state: done\n")
        head = commit("close")
        closeout.verify_commit(root, base, head, [_ITEM])

        (root / "README.md").write_text("y\n")
        stacked = commit("second")
        _refuse(lambda: closeout.verify_commit(root, base, stacked, [_ITEM]), "two commits on base")

        git("checkout", "-q", base)
        (items / f"{_ITEM}.md").write_text("state: done\n")
        (root / "README.md").write_text("y\n")
        mixed = commit("mixed")
        _refuse(lambda: closeout.verify_commit(root, base, mixed, [_ITEM]), "a commit touching README")

        git("checkout", "-q", base)
        other = "01a0ec61-d599-73ec-9e42-1ee6966420fe"
        (items / f"{other}.md").write_text("state: done\n")
        added = commit("adds an item")
        _refuse(lambda: closeout.verify_commit(root, base, added, [other]), "an added item file")

        git("checkout", "-q", base)
        git("config", "user.name", "someone")
        (items / f"{_ITEM}.md").write_text("state: done\n")
        human = commit("close by a human")
        _refuse(lambda: closeout.verify_commit(root, base, human, [_ITEM]), "a non-bot author")

    # A devloop refusal is reported, never treated as eligible.
    def refused(*args, **kwargs):
        return subprocess.CompletedProcess(args, 1, "", "E_EXPIRED: evidence expired")

    candidate = closeout.Candidate(_ITEM, "t", (closeout.Context(".devloop/inputs/x.json", "t", "i"),))
    with patch.object(closeout.subprocess, "run", refused):
        eligible, reasons = closeout.evaluate(Path("."), [candidate])
    if eligible or len(reasons) != 1 or "E_EXPIRED" not in reasons[0]:
        raise RetirementError("closeout control: a devloop refusal was not reported")


def _uncovered(root: Path, code_paths: list[str]) -> list[str]:
    """The codePaths devloop would refuse in `root`: missing or a symlink."""
    return [name for name in code_paths if not (root / name).exists() or (root / name).is_symlink()]


def check_checkout() -> int:
    """The closeout checkout covers every codePaths entry; a bare one does not.

    Returns the negative controls refused.
    """
    code_paths = json.loads((ROOT / ".devloop" / "policy.json").read_text(encoding="utf-8"))["codePaths"]
    prepare = getattr(closeout, "prepare", None)
    if prepare is None:
        raise RetirementError("closeout checkout: work_item_closeout_publish has no prepare function")
    if "prepare(source, root)" not in inspect.getsource(closeout.run):
        raise RetirementError("closeout checkout: run() does not prepare its tree with prepare()")
    with tempfile.TemporaryDirectory(prefix="closeout-checkout-") as temporary:
        bare = Path(temporary) / "bare"
        retirement_publish.prepare_checkout(ROOT, bare)
        if not _uncovered(bare, code_paths):
            raise RetirementError("closeout checkout control: a checkout without submodules covered every codePaths entry")
        refused = 1
        full = Path(temporary) / "full"
        prepare(ROOT, full)
        missing = _uncovered(full, code_paths)
        if missing:
            raise RetirementError(f"closeout checkout: codePaths missing or symlinked: {missing}")
    print(f"closeout checkout: {len(code_paths)} codePaths covered; bare checkout refused")
    return refused


if __name__ == "__main__":
    if sys.argv[1:] != ["--checkout"]:
        raise SystemExit("usage: work_item_closeout_publish_controls.py --checkout")
    try:
        devloop_observations.record(negativeControlsRefused=check_checkout())
    except RetirementError as error:
        raise SystemExit(str(error)) from error
