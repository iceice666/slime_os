#!/usr/bin/env python3
"""Propose verified MyQue retirement from canonical main, never close work."""

from __future__ import annotations

import argparse
import json
import hashlib
import os
import re
import subprocess
import tempfile
from pathlib import Path

from work_item_retirement import (
    RetirementError,
    apply_candidates,
    select_candidates,
    verify_batch,
)

REPOSITORY = "iceice666/slime_os"
PREFIX = "automation/myque-retire/"
CHECK = "Canonical work-item store"
ACTIONS_APP = 15368
OBJECT_ID = re.compile(r"[0-9a-f]{40}")


def command(root: Path, *args: str, input: str | None = None) -> str:
    result = subprocess.run(args, cwd=root, input=input, text=True, capture_output=True)
    if result.returncode:
        raise RetirementError(f"{args[0]} failed: {(result.stderr or result.stdout).strip()}")
    return result.stdout.strip()


def github(root: Path, path: str) -> object:
    return json.loads(command(root, "gh", "api", f"repos/{REPOSITORY}/{path}"))


def require_publication_permissions(root: Path, *, approve: bool = False) -> str:
    """Bind operator-verified bypass policy to the current public ruleset revision."""
    rules = github(root, "rules/branches/main")
    if not isinstance(rules, list):
        raise RetirementError("cannot read effective main rules")
    required = [rule for rule in rules if rule.get("type") == "required_status_checks"]
    guarded = any(
        rule.get("parameters", {}).get("strict_required_status_checks_policy") is True
        and any(
            check.get("context") == CHECK and check.get("integration_id") == ACTIONS_APP
            for check in rule.get("parameters", {}).get("required_status_checks", [])
        )
        for rule in required
    )
    if not guarded or not any(rule.get("type") == "pull_request" for rule in rules):
        raise RetirementError(
            f"publication requires a main PR ruleset and strict required '{CHECK}' "
            "from GitHub Actions; configure repository rules before applying"
        )
    identities = []
    for rule in rules:
        if rule.get("type") not in {"required_status_checks", "pull_request"}:
            continue
        if rule.get("ruleset_source_type") != "Repository":
            raise RetirementError("cannot verify bypass policy of non-repository ruleset")
        detail = github(root, f"rulesets/{rule['ruleset_id']}")
        if detail.get("enforcement") != "active" or not detail.get("updated_at"):
            raise RetirementError("retirement requires active versioned rulesets")
        if approve and detail.get("bypass_actors") != []:
            raise RetirementError("operator approval requires visible empty bypass actors")
        identities.append((str(rule["ruleset_id"]), detail["updated_at"]))
    digest = hashlib.sha256(json.dumps(sorted(set(identities))).encode()).hexdigest()
    if not approve and os.environ.get("MYQUE_RETIRE_RULES_APPROVAL") != digest:
        raise RetirementError(
            "ruleset approval missing or stale; an administrator must run --approve-rules"
        )
    return digest


def require_workflow_context() -> None:
    if (
        os.environ.get("GITHUB_ACTIONS") != "true"
        or os.environ.get("GITHUB_REPOSITORY") != REPOSITORY
        or os.environ.get("GITHUB_REF") != "refs/heads/main"
        or os.environ.get("GITHUB_EVENT_NAME") not in {"schedule", "workflow_dispatch"}
    ):
        raise RetirementError("--apply is reserved for the canonical main GitHub workflow")


def open_retirement_prs(root: Path) -> list[dict]:
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


def current_main(root: Path) -> str:
    output = command(root, "git", "ls-remote", "origin", "refs/heads/main")
    value = output.split()
    if len(value) != 2 or not OBJECT_ID.fullmatch(value[0]):
        raise RetirementError("cannot resolve canonical main")
    return value[0]


def require_current_main(root: Path, base: str) -> None:
    if current_main(root) != base:
        raise RetirementError("main advanced; discard this unpublished batch and retry")


def verify_commit(root: Path, base: str, head: str) -> list[str]:
    parents = command(root, "git", "rev-list", "--parents", "-n", "1", head).split()
    if parents != [head, base]:
        raise RetirementError("retirement must be one commit directly on current main")
    author = command(root, "git", "show", "-s", "--format=%an%n%ae%n%cn%n%ce", head)
    expected = "github-actions[bot]\n41898282+github-actions[bot]@users.noreply.github.com"
    if author != f"{expected}\n{expected}":
        raise RetirementError("unexpected retirement commit author or committer")
    histories = verify_batch(root, base, head)
    if not histories:
        raise RetirementError("empty retirement commit")
    return histories


def publish_history(root: Path, histories: list[str]) -> None:
    """Retained objects reach the remote before any canonical deletion does."""
    for commit in sorted(set(histories)):
        if not OBJECT_ID.fullmatch(commit):
            raise RetirementError("retained commit is not a full object identity")
        ref = f"refs/myque/retained/{commit}"
        command(root, "git", "push", "origin", f"{commit}:{ref}")
        if command(root, "git", "ls-remote", "origin", ref) != f"{commit}\t{ref}":
            raise RetirementError(f"remote did not retain {commit}")


def pr_body(root: Path, base: str, head: str, histories: list[str], skipped: list[str]) -> str:
    changed = command(root, "git", "diff", "--name-only", "--diff-filter=A", base, head)
    rows = []
    for name in changed.splitlines():
        record = json.loads(command(root, "git", "show", f"{head}:{name}"))
        identity = Path(name).stem
        api = json.loads(command(root, "myque", "api", "get", identity))
        # JSON escaping prevents item-controlled line breaks from forging report sections.
        rows.append(f"- `{identity}`: {json.dumps(api['title'], ensure_ascii=False)}")
        rows.append(f"  - Evidence: {json.dumps(record['evidence'], ensure_ascii=False)}")
        rows.append(f"  - Retained commit: `{record['history']['commit']}`")
    return (
        "\n".join(
            [
                "## Why",
                "",
                "Retire already-completed work without changing completion state.",
                "",
                "## What changed",
                "",
                *rows,
                "",
                "## Invariants / risk",
                "",
                f"- Source: `{base}`; retirement commit: `{head}`.",
                "- Full item bytes are verified in remote retained refs before this PR is published.",
                "- No completion, migration, cancellation, or automatic merge is performed.",
                "- A main change requires a newly generated batch, not rebasing this commit.",
                "",
                "## Verification",
                "",
                "- Direct: `just tasks_check` passed before and after retirement.",
                "- Direct: exact source/history bytes, metadata, graph, and restricted diff verified.",
                f"- Direct: {len(set(histories))} retained refs verified on origin.",
                "- GitHub CI remains subject to normal checks and workflow approval requirements.",
                "",
                "## Skipped",
                "",
                *(json.dumps(reason, ensure_ascii=False) for reason in skipped),
                "",
                "## Related",
                "",
                "Storage maintenance only; this PR does not implement or complete the listed work.",
            ]
        )
        + "\n"
    )


def report(text: str) -> None:
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as stream:
            stream.write(text + "\n")


def prepare_checkout(source: Path, destination: Path) -> str:
    url = command(source, "git", "remote", "get-url", "origin")
    if url not in {
        f"git@github.com:{REPOSITORY}.git",
        f"https://github.com/{REPOSITORY}.git",
        f"https://github.com/{REPOSITORY}",
    }:
        raise RetirementError("origin is not the canonical repository")
    command(source, "git", "clone", "--no-checkout", "--", url, str(destination))
    command(destination, "git", "fetch", "origin", "refs/myque/retained/*:refs/myque/retained/*")
    base = command(destination, "git", "rev-parse", "refs/remotes/origin/main")
    command(destination, "git", "checkout", "--detach", base)
    command(destination, "git", "config", "user.name", "github-actions[bot]")
    command(
        destination,
        "git",
        "config",
        "user.email",
        "41898282+github-actions[bot]@users.noreply.github.com",
    )
    command(destination, "git", "config", "commit.gpgsign", "false")
    return base


def run(source: Path, apply: bool) -> None:
    if apply:
        require_workflow_context()
    with tempfile.TemporaryDirectory(prefix="myque-retirement-") as temporary:
        root = Path(temporary) / "checkout"
        base = prepare_checkout(source, root)
        command(root, "just", "tasks_check")
        candidates, skipped = select_candidates(root)
        report(f"Source: `{base}`; eligible: {len(candidates)}; skipped: {len(skipped)}")
        for candidate in candidates:
            report(f"Candidate `{candidate.identity}`: {json.dumps(candidate.title)}")
        for reason in skipped:
            report(f"Skipped: {json.dumps(reason)}")
        if not apply:
            if candidates:
                apply_candidates(root, candidates)
                command(root, "just", "tasks_check")
            report("Preview only; canonical store and remote refs are unchanged.")
            return
        existing = open_retirement_prs(root)
        if existing:
            report(
                "Existing retirement PR; not updating: "
                + ", ".join(pr["html_url"] for pr in existing)
            )
            return
        if not candidates:
            report("No eligible completed items; nothing to publish.")
            return
        require_publication_permissions(root)
        branch = PREFIX + base
        branches = github(root, f"git/matching-refs/heads/{PREFIX}")
        pending = []
        for entry in branches:
            name = entry["ref"].removeprefix("refs/heads/")
            previous = github(root, f"pulls?state=closed&head=iceice666:{name}&base=main")
            if any(pr.get("merged_at") for pr in previous):
                continue
            if previous:
                raise RetirementError(
                    "retirement PR was closed without merging; review before retrying"
                )
            pending.append(entry)
        branches = pending
        if len(branches) > 1 or any(entry["ref"] != f"refs/heads/{branch}" for entry in branches):
            raise RetirementError(
                "unexpected or stale retirement branch; review it before retrying"
            )
        if branches:
            head = branches[0]["object"]["sha"]
            if not OBJECT_ID.fullmatch(head):
                raise RetirementError("unexpected remote branch object")
            command(root, "git", "fetch", "origin", f"refs/heads/{branch}")
            histories = verify_commit(root, base, head)
            command(root, "git", "checkout", "--detach", head)
            command(root, "just", "tasks_check")
        else:
            require_current_main(root, base)
            apply_candidates(root, candidates)
            command(root, "just", "tasks_check")
            paths = [
                path
                for candidate in candidates
                for path in (
                    f".tasks/items/{candidate.identity}.md",
                    f".tasks/terminal/{candidate.identity}.json",
                )
            ]
            command(root, "git", "add", "--", *paths)
            command(root, "git", "commit", "-m", "chore(repo/tasks): retire completed items")
            head = command(root, "git", "rev-parse", "HEAD")
            histories = verify_commit(root, base, head)
        body = pr_body(root, base, head, histories, skipped)
        if len(body) > 60000:
            raise RetirementError(
                "retirement report exceeds GitHub PR body limit; publish manually"
            )
        require_current_main(root, base)
        publish_history(root, histories)
        require_current_main(root, base)
        require_publication_permissions(root)
        # Never force-push: an unexpected existing ref must stop publication.
        command(root, "git", "push", "origin", f"{head}:refs/heads/{branch}")
        require_current_main(root, base)
        if open_retirement_prs(root):
            report("Retirement PR already published; leaving it unchanged.")
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
            "chore(repo/tasks): retire completed items",
            "--body-file",
            "-",
            input=body,
        )
        report(f"Retirement PR: {url}")


def check_pr(root: Path) -> None:
    """Run trusted base code on a pending retirement commit, not its programs."""
    branch = os.environ.get("RETIREMENT_HEAD_REF", "")
    if not branch.startswith(PREFIX):
        return
    base = branch.removeprefix(PREFIX)
    head = os.environ.get("RETIREMENT_HEAD_SHA", "")
    if not OBJECT_ID.fullmatch(base) or not OBJECT_ID.fullmatch(head):
        raise RetirementError("malformed retirement branch identity")
    require_current_main(root, base)
    command(
        root,
        "git",
        "fetch",
        "origin",
        "refs/heads/main",
        f"refs/heads/{branch}",
        "refs/myque/retained/*:refs/myque/retained/*",
    )
    histories = verify_commit(root, base, head)
    for commit in histories:
        ref = f"refs/myque/retained/{commit}"
        if command(root, "git", "ls-remote", "origin", ref) != f"{commit}\t{ref}":
            raise RetirementError("pending PR refers to history not retained on origin")
    require_current_main(root, base)
    print(f"Retirement verified against current main {base}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--apply", action="store_true", help="publish a guarded retirement PR")
    mode.add_argument("--check-pr", action="store_true", help="verify a pending retirement PR")
    mode.add_argument(
        "--approve-rules",
        action="store_true",
        help="inspect bypass actors and print operator approval digest",
    )
    args = parser.parse_args()
    try:
        root = Path.cwd()
        if args.approve_rules:
            print(require_publication_permissions(root, approve=True))
        elif args.check_pr:
            check_pr(root)
        else:
            run(root, args.apply)
    except (RetirementError, OSError, ValueError, KeyError) as error:
        print(f"retirement refused: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
