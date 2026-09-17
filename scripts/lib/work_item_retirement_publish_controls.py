"""Offline retirement publication controls using a real disposable Git remote."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path
from unittest.mock import patch

import work_item_retirement_publish as publisher
from work_item_retirement import RetirementError, apply_candidates, select_candidates


def check_controls() -> None:
    with (
        tempfile.TemporaryDirectory(prefix="retirement-publication-controls-") as temporary,
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
        directory = Path(temporary)
        remote, root, recovered = (directory / name for name in ("remote.git", "work", "recovered"))
        root.mkdir()

        def run(*args: str, cwd: Path = root, data: str | None = None) -> str:
            return publisher.command(cwd, *args, input=data)

        def refuse(action, description: str) -> None:
            try:
                action()
            except RetirementError:
                return
            raise RetirementError(f"control accepted {description}")

        run("git", "init", "--bare", str(remote))
        run("git", "init", "-b", "main")
        run("git", "config", "user.name", "github-actions[bot]")
        run("git", "config", "user.email", "41898282+github-actions[bot]@users.noreply.github.com")
        run("git", "config", "commit.gpgsign", "false")
        run("git", "remote", "add", "origin", str(remote))
        run("myque", "init")
        (root / ".gitignore").write_text(".tasks/write.lock\n")
        created = json.loads(
            run(
                "myque",
                "api",
                "create",
                "--admission",
                "retirement-remote-control",
                data=json.dumps(
                    {
                        "title": "Retirement remote recovery",
                        "kind": "task",
                        "consumers": {},
                        "body": "\n## Closure evidence\n\nRecorded control result.\n",
                    }
                ),
            )
        )
        identity = created["id"]
        run("myque", "close", identity)
        original = (root / ".tasks/items" / f"{identity}.md").read_bytes()
        run("git", "add", ".")
        run("git", "commit", "-m", "Record source")
        base = run("git", "rev-parse", "HEAD")
        run("git", "push", "origin", "main")
        run("git", "symbolic-ref", "HEAD", "refs/heads/main", cwd=remote)
        candidates, skipped = select_candidates(root)
        if len(candidates) != 1 or skipped:
            raise RetirementError("control candidate selection failed")
        apply_candidates(root, candidates)
        run("git", "add", ".tasks")
        run("git", "commit", "-m", "Retire source")
        head = run("git", "rev-parse", "HEAD")
        histories = publisher.verify_commit(root, base, head)
        branch = publisher.PREFIX + base
        hook = remote / "hooks/pre-receive"
        hook.write_text("#!/bin/sh\nexit 1\n")
        hook.chmod(0o755)
        refuse(lambda: publisher.publish_history(root, histories), "rejected history push")
        if run("git", "ls-remote", "origin", "refs/myque/retained/*", f"refs/heads/{branch}"):
            raise RetirementError("failed history push published remote state")
        hook.unlink()
        publisher.publish_history(root, histories)
        # Retrying after history-only success must keep exactly the same objects.
        publisher.publish_history(root, histories)
        run("git", "push", "origin", f"{head}:refs/heads/{branch}")
        run("git", "clone", str(remote), str(recovered))
        run("git", "fetch", "origin", "refs/myque/retained/*:refs/myque/retained/*", cwd=recovered)
        if publisher.verify_commit(recovered, base, head) != histories:
            raise RetirementError("interrupted publication cannot be verified in a fresh clone")
        run("git", "checkout", "--detach", head, cwd=recovered)
        restored = subprocess.check_output(
            ["git", "show", f"{histories[0]}:.tasks/items/{identity}.md"],
            cwd=recovered,
        )
        if restored != original:
            raise RetirementError("remote history content differs from original")
        run("myque", "reopen", identity, cwd=recovered)
        api = json.loads(run("myque", "api", "get", identity, cwd=recovered))
        if api["retired"] or api["state"] != "open" or api["body"] != created["body"]:
            raise RetirementError("fresh clone could not recover retired body")
        # Real remote main advances after planning: stale results are refused.
        run("git", "checkout", "main")
        run("git", "reset", "--hard", base)
        run("myque", "reopen", identity)
        run("git", "add", ".tasks")
        run("git", "commit", "-m", "Reopen candidate")
        run("git", "push", "origin", "main")
        refuse(lambda: publisher.require_current_main(root, base), "stale remote main")
        refuse(
            lambda: publisher.verify_commit(root, run("git", "rev-parse", "HEAD"), head),
            "stale retirement base",
        )

    with patch.dict(os.environ, {"GITHUB_ACTIONS": "false"}):
        refuse(publisher.require_workflow_context, "local apply context")

    rules = [
        {"type": "pull_request", "ruleset_id": 1, "ruleset_source_type": "Repository"},
        {
            "type": "required_status_checks",
            "ruleset_id": 1,
            "ruleset_source_type": "Repository",
            "parameters": {
                "strict_required_status_checks_policy": True,
                "required_status_checks": [
                    {"context": publisher.CHECK, "integration_id": publisher.ACTIONS_APP}
                ],
            },
        },
    ]
    detail = {"enforcement": "active", "bypass_actors": [], "updated_at": "2026-09-16T00:00:00Z"}

    def remote_policy(_root, path):
        return rules if path == "rules/branches/main" else detail

    with patch.object(publisher, "github", side_effect=remote_policy):
        approval = publisher.require_publication_permissions(Path.cwd(), approve=True)
        detail["updated_at"] = "2026-09-16T08:00:00+08:00"
        if publisher.require_publication_permissions(Path.cwd(), approve=True) != approval:
            raise RetirementError("equivalent ruleset timestamp offsets changed approval")
        detail["updated_at"] = "2026-09-16T00:00:00Z"
        with patch.dict(os.environ, {"MYQUE_RETIRE_RULES_APPROVAL": approval}):
            del detail["bypass_actors"]
            publisher.require_publication_permissions(Path.cwd())
            refuse(
                lambda: publisher.require_publication_permissions(Path.cwd(), approve=True),
                "hidden bypass actors at approval",
            )
            detail["updated_at"] = "2026-09-17T00:00:00Z"
            refuse(
                lambda: publisher.require_publication_permissions(Path.cwd()),
                "stale ruleset approval",
            )
            detail["updated_at"] = "2026-09-16T00:00:00Z"
        detail["bypass_actors"] = []
        for timestamp, description in (
            ("2026-09-16T00:00:00", "ruleset timestamp without timezone"),
            ("not-a-timestamp", "invalid ruleset timestamp"),
        ):
            detail["updated_at"] = timestamp
            refuse(
                lambda: publisher.require_publication_permissions(Path.cwd(), approve=True),
                description,
            )
        detail["updated_at"] = "2026-09-16T00:00:00Z"
        detail["bypass_actors"] = [{"actor_type": "Integration", "actor_id": publisher.ACTIONS_APP}]
        refuse(
            lambda: publisher.require_publication_permissions(Path.cwd(), approve=True),
            "ruleset bypass",
        )
        detail["bypass_actors"] = []
        rules[1]["parameters"]["strict_required_status_checks_policy"] = False
        refuse(
            lambda: publisher.require_publication_permissions(Path.cwd()),
            "non-strict required check",
        )
        rules[1]["parameters"]["strict_required_status_checks_policy"] = True
        rules[1]["parameters"]["required_status_checks"][0]["integration_id"] = 123
        refuse(
            lambda: publisher.require_publication_permissions(Path.cwd()), "wrong check provider"
        )
