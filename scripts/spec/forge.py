#!/usr/bin/env python3
"""Everything the validator needs from a code host, behind five functions.

    scripts/spec/forge.py inputs [--forge github|local]      # JSON: base, head, author, approvers
    scripts/spec/forge.py review-changes [--forge github]    # V4 metric: last 10 grader approvals
    scripts/spec/forge.py open-closeout <branch> <title> <body-file> [--forge github|local]

The five functions are `base_sha`, `author`, `approvers` (logins whose
latest review approves the current head commit), `open_closeout` (open a
pull request from a pushed branch and ask the host to merge it once its
checks pass), and `review_events` (recent reviews, for the approval
metric). No other file in scripts/spec knows which host it runs on.

`github` reads the pull_request event GitHub Actions writes to
$GITHUB_EVENT_PATH and the REST API at $GITHUB_API_URL with $GITHUB_TOKEN
(reads) or $CLOSEOUT_TOKEN (the closeout bot's own token: a pull request
opened with the workflow's built-in token triggers no checks, so V6 would
never run on it). `local` reads $SPEC_FORGE_BASE, $SPEC_FORGE_AUTHOR and
$SPEC_FORGE_APPROVERS, and opens nothing; the e2e fixture uses it.
`gitlab` is M6.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import urllib.request
from pathlib import Path


class Forge:
    def base_sha(self) -> str:
        raise NotImplementedError

    def author(self) -> str:
        raise NotImplementedError

    def approvers(self, head: str) -> set[str]:
        raise NotImplementedError

    def open_closeout(self, branch: str, title: str, body: str) -> str:
        raise NotImplementedError

    def review_events(self, limit: int) -> list[dict]:
        """Recent merged pull requests: `{"number", "approvers", "changesRequestedBy", "graderPaths"}`."""
        raise NotImplementedError


class Local(Forge):
    def base_sha(self) -> str:
        return os.environ.get("SPEC_FORGE_BASE", "")

    def author(self) -> str:
        return os.environ.get("SPEC_FORGE_AUTHOR", "")

    def approvers(self, head: str) -> set[str]:
        return {a.strip() for a in os.environ.get("SPEC_FORGE_APPROVERS", "").split(",") if a.strip()}

    def open_closeout(self, branch: str, title: str, body: str) -> str:
        return f"local:{branch}"

    def review_events(self, limit: int) -> list[dict]:
        return []


GRADER_HINTS = ("scripts/check/", "scripts/lib/", "scripts/spec/", "just/", "Justfile", "specs/")


class GitHub(Forge):
    def __init__(self) -> None:
        self.api = os.environ.get("GITHUB_API_URL", "https://api.github.com")
        self.repo = os.environ.get("GITHUB_REPOSITORY", "")
        path = os.environ.get("GITHUB_EVENT_PATH")
        self.event = json.loads(Path(path).read_text()) if path and Path(path).is_file() else {}
        self.pull = self.event.get("pull_request", {})

    def request(self, method: str, path: str, data: dict | None = None, token_env: str = "GITHUB_TOKEN") -> object:
        token = os.environ.get(token_env, "")
        request = urllib.request.Request(
            f"{self.api}{path}",
            method=method,
            data=json.dumps(data).encode() if data is not None else None,
            headers={"Accept": "application/vnd.github+json", "Authorization": f"Bearer {token}", "X-GitHub-Api-Version": "2022-11-28"},
        )
        with urllib.request.urlopen(request, timeout=60) as response:
            body = response.read()
        return json.loads(body) if body else {}

    def base_sha(self) -> str:
        return self.pull.get("base", {}).get("sha", "")

    def author(self) -> str:
        return self.pull.get("user", {}).get("login", "")

    def reviews(self, number: int) -> list[dict]:
        found: list[dict] = []
        page = 1
        while True:
            batch = self.request("GET", f"/repos/{self.repo}/pulls/{number}/reviews?per_page=100&page={page}")
            found.extend(batch)
            if len(batch) < 100:
                return found
            page += 1

    def approvers(self, head: str) -> set[str]:
        number = self.pull.get("number")
        if not number:
            return set()
        latest: dict[str, dict] = {}
        for review in self.reviews(number):
            if review.get("state") in {"APPROVED", "CHANGES_REQUESTED", "DISMISSED"}:
                latest[review["user"]["login"]] = review
        return {login for login, review in latest.items() if review["state"] == "APPROVED" and review.get("commit_id") == head}

    def open_closeout(self, branch: str, title: str, body: str) -> str:
        default = self.event.get("repository", {}).get("default_branch", "main")
        pull = self.request("POST", f"/repos/{self.repo}/pulls", {"title": title, "head": branch, "base": default, "body": body}, token_env="CLOSEOUT_TOKEN")
        query = "mutation($id: ID!) { enablePullRequestAutoMerge(input: {pullRequestId: $id, mergeMethod: SQUASH}) { clientMutationId } }"
        try:
            self.request("POST", "/graphql", {"query": query, "variables": {"id": pull["node_id"]}}, token_env="CLOSEOUT_TOKEN")
        except OSError as error:
            print(f"forge: auto-merge not enabled on #{pull['number']}: {error}", file=sys.stderr)
        return pull["html_url"]

    def review_events(self, limit: int) -> list[dict]:
        events = []
        for pull in self.request("GET", f"/repos/{self.repo}/pulls?state=closed&per_page=100"):
            if not pull.get("merged_at"):
                continue
            files = self.request("GET", f"/repos/{self.repo}/pulls/{pull['number']}/files?per_page=100")
            grader = [f["filename"] for f in files if f["filename"].startswith(GRADER_HINTS)]
            if not grader:
                continue
            reviews = self.reviews(pull["number"])
            approvers = {r["user"]["login"] for r in reviews if r["state"] == "APPROVED"}
            if not approvers:
                continue
            asked = {r["user"]["login"] for r in reviews if r["state"] in {"CHANGES_REQUESTED", "COMMENTED"}}
            events.append({"number": pull["number"], "approvers": sorted(approvers), "changesRequestedBy": sorted(asked & approvers), "graderPaths": grader})
            if len(events) == limit:
                break
        return events


class GitLab(Forge):
    """M6: the GitLab implementation of the same five functions."""


FORGES = {"local": Local, "github": GitHub, "gitlab": GitLab}


def get(name: str | None) -> Forge:
    chosen = name or ("github" if os.environ.get("GITHUB_ACTIONS") else "local")
    return FORGES[chosen]()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=("inputs", "review-changes", "open-closeout"))
    parser.add_argument("rest", nargs="*")
    parser.add_argument("--forge", choices=sorted(FORGES))
    parser.add_argument("--head", default="")
    args = parser.parse_args()
    forge = get(args.forge)
    if args.command == "inputs":
        head = args.head or os.environ.get("GITHUB_SHA", "")
        if isinstance(forge, GitHub):
            head = forge.pull.get("head", {}).get("sha", head)
        print(json.dumps({"base": forge.base_sha(), "head": head, "author": forge.author(), "approvers": sorted(forge.approvers(head))}))
        return 0
    if args.command == "review-changes":
        events = forge.review_events(10)
        asked = sum(1 for e in events if e["changesRequestedBy"])
        print(json.dumps({"approvals": len(events), "withRequestedChange": asked, "events": events}, indent=1))
        return 0
    branch, title, body_file = args.rest
    print(forge.open_closeout(branch, title, Path(body_file).read_text()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
