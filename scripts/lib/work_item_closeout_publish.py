#!/usr/bin/env python3
"""Propose completion of spec-driven items from canonical main, never grade them.

An implementation branch records gate evidence on its item but cannot close
it: `just devloop complete` refuses unless the inputs file, recipe closure and
scripts that graded the item are byte-for-byte what `origin/main` carries, and
a branch is not `main`. Once the branch merges, that condition holds on `main`
itself, and the only remaining work is to ask devloop the same question there
and commit the transition it makes. This module does exactly that, from a
fresh clone of canonical `main`, and proposes the resulting `.tasks`-only
change as a pull request for a human to merge.

Nothing here decides eligibility. `just devloop eligible` and `just devloop
complete` do, under `scripts/check/devloop-approval.py`; an item devloop
refuses is reported with its reason, not closed. Evidence identities name the
inputs digest, target and image a gate ran under, so the execution context is
recovered from the record rather than guessed.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

from work_item_retirement import RetirementError
from work_item_retirement_publish import (
    REPOSITORY,
    command,
    current_main,
    prepare_checkout,
    report,
)

PREFIX = "automation/myque-closeout/"
POLICY = ".devloop/policy.json"
INPUTS = ".devloop/inputs"
UUID = r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"

_DEVLOOP_BLOCK = re.compile(r"^devloop:\n((?:  .*\n|\n)*)", re.M)
_EPOCH = re.compile(r"^  epoch: (\S+)$", re.M)
_IDENTITY_FIELD = re.compile(r"^      (inputs|target|image): (\S+)$", re.M)


class CloseoutError(RetirementError):
    """A refused publication step; the message names the condition."""


@dataclass(frozen=True)
class Context:
    inputs: str  # path relative to the checkout
    target: str
    image: str


@dataclass(frozen=True)
class Candidate:
    identity: str
    title: str
    contexts: tuple[Context, ...]


def evidence_contexts(item_text: str) -> tuple[tuple[str, str, str], ...]:
    """Distinct (inputs digest, target, image) triples the item's evidence records.

    Only the `devloop:` frontmatter block is read; an unstarted epoch or an
    empty evidence list yields nothing. Order is first appearance.
    """
    block = _DEVLOOP_BLOCK.search(item_text)
    if block is None:
        return ()
    text = block.group(1)
    epoch = _EPOCH.search(text)
    if epoch is None or epoch.group(1) == "unstarted":
        return ()
    triples: list[tuple[str, str, str]] = []
    pending: dict[str, str] = {}
    for field, value in _IDENTITY_FIELD.findall(text):
        pending[field] = value
        if len(pending) == 3:
            triple = (pending["inputs"], pending["target"], pending["image"])
            if triple not in triples:
                triples.append(triple)
            pending = {}
    return tuple(triples)


def inputs_by_digest(root: Path) -> dict[str, str]:
    """Tracked `.devloop/inputs` files keyed by the digest devloop binds."""
    listed = command(root, "git", "ls-files", "--", INPUTS)
    table = {}
    for name in listed.splitlines():
        digest = hashlib.sha256((root / name).read_bytes()).hexdigest()
        table[digest] = name
    return table


def prepare(source: Path, destination: Path) -> str:
    """Clone canonical main with its submodules, and return the base commit.

    devloop's code identity covers `.devloop/policy.json` codePaths inside
    submodules, so a tree without them is refused for every item. Submodules are
    checked out at the commits main records, never at their remote heads.
    """
    base = prepare_checkout(source, destination)
    command(destination, "git", "submodule", "update", "--init", "--recursive")
    return base


def require_workflow_context() -> None:
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
        or os.environ.get("GITHUB_REF") != "refs/heads/main"
        or os.environ.get("GITHUB_EVENT_NAME") not in {"push", "workflow_dispatch"}
    ):
        raise CloseoutError("--apply is reserved for the canonical main GitHub workflow")


def select_candidates(root: Path) -> tuple[list[Candidate], list[str]]:
    """Live spec-driven items with recorded evidence and a resolvable context."""
    rows = command(
        root, "myque", "query", "state = open or state = active", "--format", "json"
    ).splitlines()
    inputs = inputs_by_digest(root)
    candidates: list[Candidate] = []
    skipped: list[str] = []
    for row in rows:
        summary = json.loads(row)
        identity = summary["id"]
        if not re.fullmatch(UUID, identity):
            raise CloseoutError(f"not a canonical UUID: {identity!r}")
        text = (root / ".tasks" / "items" / f"{identity}.md").read_text(encoding="utf-8")
        if "\ndevloop:\n" not in text:
            continue
        triples = evidence_contexts(text)
        if not triples:
            continue
        contexts = []
        for digest, target, image in triples:
            name = inputs.get(digest)
            if name is None:
                skipped.append(
                    f"{identity} ({summary['title']}): evidence names inputs digest "
                    f"{digest[:12]}… which no tracked {INPUTS} file carries"
                )
                continue
            contexts.append(Context(name, target, image))
        if contexts:
            candidates.append(Candidate(identity, summary["title"], tuple(contexts)))
    return candidates, skipped


def _devloop(root: Path, verb: str, candidate: Candidate, context: Context) -> tuple[bool, str]:
    finished = subprocess.run(
        [
            "just",
            "devloop",
            verb,
            candidate.identity,
            "--policy",
            POLICY,
            "--inputs",
            context.inputs,
            "--target",
            context.target,
            "--image",
            context.image,
        ],
        cwd=root,
        capture_output=True,
        text=True,
    )
    return finished.returncode == 0, _diagnostic(finished.stderr + finished.stdout)


def _diagnostic(output: str) -> str:
    """devloop's own refusal line, not `just`'s wrapper around it."""
    lines = [
        line.strip()
        for line in output.splitlines()
        if line.strip() and not line.startswith("error: recipe") and not line.startswith("python3 ")
    ]
    return lines[-1] if lines else "refused without a diagnostic"


def evaluate(root: Path, candidates: list[Candidate]) -> tuple[list[tuple[Candidate, Context]], list[str]]:
    """Ask devloop, per recorded context, which candidates it would close."""
    eligible: list[tuple[Candidate, Context]] = []
    refused: list[str] = []
    for candidate in candidates:
        reasons = []
        chosen = None
        for context in candidate.contexts:
            passed, detail = _devloop(root, "eligible", candidate, context)
            if passed:
                chosen = context
                break
            reasons.append(f"{context.target}/{context.image}: {detail}")
        if chosen is None:
            refused.append(f"{candidate.identity} ({candidate.title}): " + "; ".join(reasons))
        else:
            eligible.append((candidate, chosen))
    return eligible, refused


def complete(root: Path, eligible: list[tuple[Candidate, Context]]) -> None:
    for candidate, context in eligible:
        passed, detail = _devloop(root, "complete", candidate, context)
        if not passed:
            raise CloseoutError(f"{candidate.identity}: complete refused after eligible: {detail}")


def verify_commit(root: Path, base: str, head: str, identities: list[str]) -> None:
    """One bot commit on current main that changes only the closed items' files."""
    parents = command(root, "git", "rev-list", "--parents", "-n", "1", head).split()
    if parents != [head, base]:
        raise CloseoutError("closeout must be one commit directly on current main")
    author = command(root, "git", "show", "-s", "--format=%an%n%ae%n%cn%n%ce", head)
    expected = "github-actions[bot]\n41898282+github-actions[bot]@users.noreply.github.com"
    if author != f"{expected}\n{expected}":
        raise CloseoutError("unexpected closeout commit author or committer")
    changed = command(root, "git", "diff", "--name-status", base, head).splitlines()
    allowed = {f".tasks/items/{identity}.md" for identity in identities}
    for line in changed:
        status, _, path = line.partition("\t")
        if status != "M" or path not in allowed:
            raise CloseoutError(f"closeout commit touches {path!r} ({status}); only closed items may change")
    if len(changed) != len(allowed):
        raise CloseoutError("closeout commit does not change every closed item")


def open_closeout_prs(root: Path) -> list[dict]:
    pages = json.loads(
        command(
            root,
            "gh",
            "api",
            "--paginate",
            "--slurp",
            f"repos/{REPOSITORY}/pulls?state=open&base=main&per_page=100",
        )
    )
    return [
        pr
        for page in pages
        for pr in page
        if pr["head"]["ref"].startswith(PREFIX)
        and pr["head"].get("repo", {}).get("full_name") == REPOSITORY
    ]


def close_stale_prs(root: Path, current: str) -> None:
    """A closeout proposed against a superseded main is regenerated, not rebased."""
    for pr in open_closeout_prs(root):
        branch = pr["head"]["ref"]
        if branch == PREFIX + current:
            continue
        command(
            root,
            "gh",
            "pr",
            "close",
            str(pr["number"]),
            "--repo",
            REPOSITORY,
            "--delete-branch",
            "--comment",
            "main advanced past `" + branch.removeprefix(PREFIX) + "`; superseded by a fresh closeout.",
        )
        report(f"Closed stale closeout PR #{pr['number']} ({branch})")


def pr_body(base: str, head: str, eligible: list[tuple[Candidate, Context]], refused: list[str], skipped: list[str]) -> str:
    rows = []
    for candidate, context in eligible:
        rows.append(f"- `{candidate.identity}`: {json.dumps(candidate.title, ensure_ascii=False)}")
        rows.append(f"  - Context: `{context.inputs}` on `{context.target}` / `{context.image}`")
    return (
        "\n".join(
            [
                "## Why",
                "",
                "Close spec-driven items whose recorded evidence `just devloop complete` accepted on canonical main.",
                "",
                "## What changed",
                "",
                *rows,
                "",
                "## Invariants / risk",
                "",
                f"- Source: `{base}`; closeout commit: `{head}`.",
                "- Each transition was made by `just devloop complete` under `devloop-approval.py`; nothing here decided eligibility.",
                "- Only the closed items' `.tasks/items` files change; no evidence, inputs or checker is written.",
                "- A main change requires a newly generated closeout, not rebasing this commit.",
                "",
                "## Verification",
                "",
                "- Direct: `just tasks_check` passed before and after completion.",
                "- GitHub CI remains subject to normal checks.",
                "",
                "## Not closed",
                "",
                *(f"- {json.dumps(reason, ensure_ascii=False)}" for reason in refused + skipped),
                "",
                "## Related",
                "",
                "Storage maintenance only; merging records completion devloop already judged.",
            ]
        )
        + "\n"
    )


def run(source: Path, apply: bool) -> None:
    if apply:
        require_workflow_context()
    with tempfile.TemporaryDirectory(prefix="myque-closeout-") as temporary:
        root = Path(temporary) / "checkout"
        base = prepare(source, root)
        command(root, "just", "tasks_check")
        candidates, skipped = select_candidates(root)
        eligible, refused = evaluate(root, candidates)
        report(f"Source: `{base}`; with evidence: {len(candidates)}; eligible: {len(eligible)}")
        for candidate, context in eligible:
            report(f"Eligible `{candidate.identity}`: {json.dumps(candidate.title)} via {context.inputs}")
        for reason in refused:
            report(f"Refused: {json.dumps(reason)}")
        for reason in skipped:
            report(f"Skipped: {json.dumps(reason)}")
        if not apply:
            if eligible:
                complete(root, eligible)
                command(root, "just", "tasks_check")
            report("Preview only; canonical store and remote refs are unchanged.")
            return
        close_stale_prs(root, base)
        existing = open_closeout_prs(root)
        if existing:
            report("Existing closeout PR; not updating: " + ", ".join(pr["html_url"] for pr in existing))
            return
        if not eligible:
            report("No eligible items; nothing to publish.")
            return
        if current_main(root) != base:
            raise CloseoutError("main advanced; discard this closeout and retry")
        complete(root, eligible)
        command(root, "just", "tasks_check")
        identities = [candidate.identity for candidate, _ in eligible]
        command(root, "git", "add", "--", *(f".tasks/items/{identity}.md" for identity in identities))
        command(
            root,
            "git",
            "commit",
            "-m",
            f"chore(repo/tasks): close {len(identities)} item(s) on recorded evidence",
        )
        head = command(root, "git", "rev-parse", "HEAD")
        verify_commit(root, base, head, identities)
        body = pr_body(base, head, eligible, refused, skipped)
        if len(body) > 60000:
            raise CloseoutError("closeout report exceeds GitHub PR body limit; publish manually")
        branch = PREFIX + base
        if current_main(root) != base:
            raise CloseoutError("main advanced; discard this closeout and retry")
        # Never force-push: an unexpected existing ref must stop publication.
        command(root, "git", "push", "origin", f"{head}:refs/heads/{branch}")
        if open_closeout_prs(root):
            report("Closeout PR already published; leaving it unchanged.")
            return
        url = command(
            root,
            "gh",
            "pr",
            "create",
            "--repo",
            REPOSITORY,
            "--base",
            "main",
            "--head",
            branch,
            "--title",
            f"chore(repo/tasks): close {len(identities)} item(s) on recorded evidence",
            "--body-file",
            "-",
            input=body,
        )
        report(f"Closeout PR: {url}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="publish a closeout PR from the main workflow")
    args = parser.parse_args()
    try:
        run(Path.cwd(), args.apply)
    except (RetirementError, OSError, ValueError, KeyError) as error:
        print(f"closeout refused: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
