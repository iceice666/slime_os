"""Offline real-CLI retirement controls, invoked by check-work-items.py."""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from unittest.mock import patch

from work_item_retirement import RetirementError, apply_candidates, select_candidates, verify_batch


def check_retirement_controls() -> list[str]:
    """Exercise observable storage transitions and hostile pending branches."""
    failures: list[str] = []
    git, myque = shutil.which("git"), shutil.which("myque")
    if git is None or myque is None:
        return ["retirement control requires pinned myque and git on PATH"]
    with tempfile.TemporaryDirectory(prefix="retirement-controls-") as temporary:
        root = Path(temporary) / "repository"
        root.mkdir()
        home = Path(temporary) / "home"
        home.mkdir()
        environment = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(home),
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "TZ": "UTC",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull,
            "GIT_AUTHOR_NAME": "Retirement control",
            "GIT_AUTHOR_EMAIL": "retirement-control@example.invalid",
            "GIT_COMMITTER_NAME": "Retirement control",
            "GIT_COMMITTER_EMAIL": "retirement-control@example.invalid",
            "GIT_AUTHOR_DATE": "2026-09-16T00:00:00Z",
            "GIT_COMMITTER_DATE": "2026-09-16T00:00:00Z",
        }

        def command(program: str, *arguments: str, data: bytes | None = None) -> bytes:
            result = subprocess.run(
                [program, *arguments], cwd=root, input=data, capture_output=True, env=environment
            )
            if result.returncode:
                raise RetirementError(
                    f"control setup {program} {arguments}: "
                    + (result.stderr or result.stdout).decode(errors="replace")
                )
            return result.stdout

        def observe(condition: bool, message: str) -> None:
            if not condition:
                failures.append("retirement control: " + message)

        def refuses(action, message: str) -> None:
            try:
                action()
            except RetirementError:
                return
            failures.append("retirement control: accepted " + message)

        def get(identity: str) -> dict:
            return json.loads(command(myque, "api", "get", identity))

        def put(identity: str, **fields) -> None:
            command(
                myque,
                "api",
                "put",
                identity,
                "--expected",
                get(identity)["revision"],
                data=json.dumps(fields).encode(),
            )

        def new(
            title: str,
            body: str,
            transition: str | None = "close",
            *,
            consumers: dict[str, str] | None = None,
        ) -> str:
            command(myque, "new", title, "--key", title)
            identity = next(
                json.loads(line)["id"]
                for line in command(
                    myque, "query", f"key = {title}", "--format", "json"
                ).splitlines()
            )
            put(identity, body=body, consumers=consumers or {})
            if transition:
                command(myque, transition, identity)
            return identity

        def path(identity: str) -> Path:
            return root / ".tasks" / "items" / f"{identity}.md"

        def commit(message: str) -> str:
            command(git, "add", "--all")
            command(git, "commit", "-m", message)
            return command(git, "rev-parse", "HEAD").decode().strip()

        def pending_commit(base: str) -> str:
            command(git, "add", "--all")
            tree = command(git, "write-tree").decode().strip()
            return (
                command(git, "commit-tree", tree, "-p", base, data=b"Control pending branch\n")
                .decode()
                .strip()
            )

        with patch.dict(os.environ, environment, clear=True):
            try:
                command(git, "init", "--initial-branch=main")
                (root / ".gitignore").write_text("/.tasks/write.lock\n")
                command(myque, "init")
                evidence_v1 = "\nObserved exact v1 results.  \n### Limits\nOffline only.\n- Tests passed\n---\n\n"
                first = new(
                    "V1",
                    "\n```md\n## Closure evidence\nNot evidence.\n```\n"
                    "## Closure evidence\n"
                    + evidence_v1
                    + "## Exit conditions\nNot observations.\n",
                )
                path(first).write_bytes(
                    path(first)
                    .read_bytes()
                    .replace(b"schema: work-item/v2", b"schema: work-item/v1", 1)
                )
                evidence_v2 = "\r\nObserved " + "long result " * 900 + "\r\nNo final newline"
                second = new("V2", "\r\n## Closure evidence\r\n" + evidence_v2)
                untouched = {}
                for title, transition in (
                    ("Open", None),
                    ("Active", "start"),
                    ("Blocked", "block"),
                    ("Deferred", "defer"),
                    ("Cancelled", "cancel"),
                ):
                    identity = new(title, "\n## Closure evidence\nMust remain live.\n", transition)
                    untouched[identity] = path(identity).read_bytes()
                dependent = new("Dependent", "\nWaiting on V1.\n", None)
                command(myque, "depend", dependent, first)
                untouched[dependent] = path(dependent).read_bytes()
                skipped = {}
                for title, body in (
                    ("Missing", "\n## Exit conditions\nFuture assertions are not results.\n"),
                    ("Empty", "\n## Closure evidence\n \t\n## Next\nOther section.\n"),
                    ("Ambiguous", "\n## Closure evidence\nOne.\n## Closure evidence\nTwo.\n"),
                    ("Fenced", "\n~~~~md\n## Closure evidence\nExample.\n~~~~\n"),
                    ("Commented", "\n<!--\n## Closure evidence\nExample.\n-->\n"),
                    ("RawHtml", "\n<pre>\n## Closure evidence\nExample.\n</pre>\n"),
                ):
                    identity = new(title, body)
                    skipped[identity] = path(identity).read_bytes()
                unknown = new(
                    "Unknown",
                    "\n## Closure evidence\nRecorded.\n",
                    consumers={"unknown": "unknown:\n  value: preserved\n"},
                )
                skipped[unknown] = path(unknown).read_bytes()
                already = new("Retired", "\nHistorical manual item.\n")
                commit("Initial control store")
                command(
                    myque, "retire", already, "--evidence", "Existing manual evidence", data=b"{}"
                )
                commit("Existing terminal item")
                already_bytes = (root / ".tasks" / "terminal" / f"{already}.json").read_bytes()
                before_refs = command(git, "show-ref")
                before_status = command(git, "status", "--porcelain=v1", "--untracked-files=all")
                candidates, reasons = select_candidates(root)
                observe(
                    {candidate.identity for candidate in candidates} == {first, second},
                    "selection did not isolate done eligible v1/v2 items",
                )
                observe(
                    {reason.split(" ", 1)[0] for reason in reasons} == set(skipped),
                    "missing, ambiguous, fenced, empty, or unknown-consumer skips were not reported",
                )
                observed_evidence = {
                    candidate.identity: candidate.evidence for candidate in candidates
                }
                observe(
                    observed_evidence == {first: evidence_v1, second: evidence_v2},
                    "prose evidence was trimmed, truncated, or included a fenced example/next section",
                )
                observe(
                    command(git, "show-ref") == before_refs
                    and command(git, "status", "--porcelain=v1", "--untracked-files=all")
                    == before_status,
                    "preview changed refs or canonical files",
                )
                original = path(first).read_bytes()
                put(first, body="\n## Closure evidence\nEdited after preview.\n")
                commit("Edit source after preview")
                refuses(lambda: apply_candidates(root, candidates), "changed source after preview")
                observe(not get(first)["retired"], "changed source was removed despite refusal")
                path(first).write_bytes(original)
                commit("Restore fixture source")
                command(myque, "reopen", first)
                commit("Reopen source after preview")
                refuses(lambda: apply_candidates(root, candidates), "reopened source after preview")
                observe(
                    get(first)["state"] == "open" and not get(first)["retired"],
                    "reopened item was retired",
                )
                path(first).write_bytes(original)
                base = commit("Fresh source for retirement")
                candidates, _ = select_candidates(root)
                original_bodies = {
                    candidate.identity: get(candidate.identity)["body"] for candidate in candidates
                }
                apply_candidates(root, candidates)
                head = commit("Retire eligible items")
                retained = verify_batch(root, base, head)
                observe(len(retained) == 2, "verified batch did not expose both retained snapshots")
                observe(
                    get(first)["schema"] == "work-item/v1"
                    and get(second)["schema"] == "work-item/v2",
                    "retirement changed envelope versions",
                )
                observe(
                    get(dependent)["ready"] and get(dependent)["dependenciesDone"],
                    "incoming dependency stopped resolving as done",
                )
                observe(
                    all(
                        path(identity).read_bytes() == content
                        for identity, content in (untouched | skipped).items()
                    ),
                    "nonselected item bytes changed",
                )
                observe(
                    (root / ".tasks" / "terminal" / f"{already}.json").read_bytes()
                    == already_bytes,
                    "previously retired item changed",
                )
                for candidate in candidates:
                    record = json.loads(
                        (root / ".tasks" / "terminal" / f"{candidate.identity}.json").read_bytes()
                    )
                    history = record["history"]
                    observe(
                        command(git, "show", history["commit"] + ":" + history["path"])
                        == candidate.original,
                        "retained snapshot does not contain exact original bytes",
                    )
                retry, _ = select_candidates(root)
                observe(not retry, "repeated preview selected already retired items")
                apply_candidates(root, retry)
                observe(
                    not command(git, "status", "--porcelain=v1"), "empty retry changed the store"
                )

                terminal = root / ".tasks" / "terminal" / f"{first}.json"
                original_terminal = terminal.read_bytes()

                def tamper(label: str, change) -> None:
                    record = json.loads(original_terminal)
                    change(record)
                    terminal.write_text(json.dumps(record))
                    revision = pending_commit(base)
                    refuses(lambda: verify_batch(root, base, revision), label)
                    terminal.write_bytes(original_terminal)
                    command(git, "add", "--all")

                tamper(
                    "tampered evidence", lambda record: record.update(evidence="Invented evidence")
                )
                tamper(
                    "tampered metadata",
                    lambda record: record.update(
                        metadata=record["metadata"].replace("# V1", "# Altered title")
                    ),
                )
                tamper("tampered digest", lambda record: record["history"].update(digest="0" * 64))
                tamper(
                    "tampered repository",
                    lambda record: record["history"].update(repository="0" * 40),
                )
                tamper(
                    "tampered original path",
                    lambda record: record.update(originalPath=".tasks/items/wrong.md"),
                )
                history = json.loads(original_terminal)["history"]
                snapshot_tree = (
                    command(git, "rev-parse", history["commit"] + "^{tree}").decode().strip()
                )
                counterfeit = (
                    command(
                        git,
                        "commit-tree",
                        snapshot_tree,
                        "-p",
                        head,
                        data=b"Wrong-base retained snapshot\n",
                    )
                    .decode()
                    .strip()
                )
                command(git, "update-ref", "refs/myque/retained/" + counterfeit, counterfeit)
                tamper(
                    "snapshot based on stale/wrong source",
                    lambda record: record["history"].update(commit=counterfeit),
                )
                wrong_blob = (
                    command(
                        git, "hash-object", "-w", "--stdin", data=b"Invented retained item bytes\n"
                    )
                    .decode()
                    .strip()
                )
                leaf = (
                    command(git, "mktree", data=f"100644 blob {wrong_blob}\t{first}.md\n".encode())
                    .decode()
                    .strip()
                )
                items_tree = (
                    command(git, "mktree", data=f"040000 tree {leaf}\titems\n".encode())
                    .decode()
                    .strip()
                )
                wrong_tree = (
                    command(git, "mktree", data=f"040000 tree {items_tree}\t.tasks\n".encode())
                    .decode()
                    .strip()
                )
                wrong_snapshot = (
                    command(
                        git, "commit-tree", wrong_tree, "-p", base, data=b"Forged retained bytes\n"
                    )
                    .decode()
                    .strip()
                )
                command(git, "update-ref", "refs/myque/retained/" + wrong_snapshot, wrong_snapshot)
                tamper(
                    "forged retained bytes",
                    lambda record: record["history"].update(commit=wrong_snapshot),
                )
                command(git, "update-ref", "-d", "refs/myque/retained/" + history["commit"])
                refuses(lambda: verify_batch(root, base, head), "unretained history object")
                command(
                    git, "update-ref", "refs/myque/retained/" + history["commit"], history["commit"]
                )
                advanced = (
                    command(
                        git,
                        "commit-tree",
                        command(git, "rev-parse", base + "^{tree}").decode().strip(),
                        "-p",
                        base,
                        data=b"Main advanced after planning\n",
                    )
                    .decode()
                    .strip()
                )
                refuses(
                    lambda: verify_batch(root, advanced, head),
                    "stale pending branch after main advanced",
                )
                extra = root / "unexpected.txt"
                extra.write_text("Not a storage transition\n")
                refuses(
                    lambda: verify_batch(root, base, pending_commit(base)),
                    "unrelated pending branch file",
                )
                extra.unlink()
                command(git, "add", "--all")
                verify_batch(root, base, head)
                for candidate in candidates:
                    command(myque, "reopen", candidate.identity)
                    restored = get(candidate.identity)
                    observe(
                        restored["id"] == candidate.identity
                        and restored["state"] == "open"
                        and not restored["retired"]
                        and restored["body"] == original_bodies[candidate.identity],
                        "reopen did not restore original identity and exact body",
                    )
                config = root / ".tasks" / "config.toml"
                config.write_text('[storage]\nitems = "../outside"\n')
                refuses(lambda: select_candidates(root), "unsafe configured storage")
            except (RetirementError, OSError, ValueError, KeyError) as error:
                failures.append(f"retirement control could not complete: {error}")
    return failures
