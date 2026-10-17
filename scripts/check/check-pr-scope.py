#!/usr/bin/env python3

"""Refuse a pull request whose diff mixes a new work item with product changes.

The input is a Git revision range, which is why this is its own checker
rather than a case in `check-work-items.py`: that gate validates the store
offline and must not depend on what a branch is being compared against.
The classes themselves live in `scripts/lib/pr_scope.py`; this file only
reads the diff, reports the classification, and runs the negative controls
that prove the rule can refuse.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import json
from pathlib import Path
import subprocess
import tempfile

from harness import ROOT
import pr_scope


DIFF_OPTIONS = ("--no-ext-diff", "--no-textconv", "--find-renames=50%", "--find-copies=50%", "--find-copies-harder", "-l0")
POLICY_PATH = ".devloop/policy.json"


def git(repo: Path, *args: str) -> str:
    finished = subprocess.run(
        ["git", *args], cwd=repo, capture_output=True,
    )
    if finished.returncode:
        diagnostic = finished.stderr.decode("utf-8", errors="backslashreplace").strip()
        raise ValueError(f"git {args!r} failed (exit {finished.returncode}): {diagnostic}")
    return finished.stdout.decode("utf-8", errors="surrogateescape")


def resolve(repo: Path, revision: str) -> str:
    return git(repo, "rev-parse", "--verify", "--end-of-options", f"{revision}^{{commit}}").strip()


def diff(base: str, head: str, repo: Path = ROOT) -> list[tuple[str, str]]:
    return pr_scope.parse_name_status(git(repo, "diff", "--name-status", "-z", *DIFF_OPTIONS, base, head, "--"))


def compare(base: str, head: str, repo: Path = ROOT) -> pr_scope.Verdict:
    """Bind policy and changed paths to the same immutable endpoint commits."""
    base_commit, head_commit = resolve(repo, base), resolve(repo, head)
    code = set()
    for revision in (base_commit, head_commit):
        try:
            code.update(pr_scope.code_paths_data(git(repo, "show", f"{revision}:{POLICY_PATH}")))
        except ValueError as error:
            raise ValueError(f"policy at {revision}:{POLICY_PATH}: {error}") from error
    return pr_scope.judge(diff(base_commit, head_commit, repo), tuple(sorted(code)))


def report(verdict: pr_scope.Verdict) -> None:
    for name in pr_scope.CLASSES:
        paths = verdict.classes[name]
        print(f"{name}: {len(paths)} path(s)")
        for path in sorted(paths):
            print(f"  {path!r}")
    if verdict.added_items:
        print("planning: lands work item(s) " + ", ".join(sorted(verdict.added_items)))
    elif verdict.needs_landed_item:
        print("implementation: a work item already on main must cover this change")
    else:
        print("process: no landed work item required")


def controls() -> None:
    """Synthetic diffs the rule must refuse or accept, without Git."""
    code = ("slime-root", "components", "contracts", "boot-contracts", "scripts/check/check-docs.py", "Justfile")
    item = ".tasks/items/01a0ec61-d599-73ec-9e42-1ee6966420fd.md"
    cases = [
        ([("A", item), ("M", "slime-root/src/main.rs")], True, "item plus product"),
        ([("A", item), ("R100", "components/a.rs")], True, "item plus renamed product"),
        ([("A", item), ("M", "scripts/check/check-docs.py")], False, "item plus grader"),
        ([("A", item), ("A", ".devloop/inputs/x.json"), ("M", "Justfile")], False, "item plus exam"),
        ([("A", item), ("M", "docs/plans/x.md")], False, "item plus docs"),
        ([("M", item), ("M", "slime-root/src/main.rs")], False, "modified item plus product"),
        ([("A", ".tasks/terminal/01a0ec61-d599-73ec-9e42-1ee6966420fd.json")], False, "retirement"),
        ([("M", ".github/workflows/ci.yml"), ("M", "CONTRIBUTING.md")], False, "process only"),
        (
            [("A", item), ("M", "scripts/check/x.py"), ("M", "contracts/system-image-closure/v2/closures/sel4.zti")],
            False,
            "item plus grader plus regenerated closures",
        ),
        ([("A", item), ("M", "contracts/system-image-closure/v2/check.zt")], True, "item plus closure contract source"),
        ([("A", item), ("A", "contracts/entropy/v1/schema.zt")], False, "item plus new schema"),
        ([("A", item), ("A", "contracts/bootstate/trace/v2/schema.zt")], False, "item plus nested new schema version"),
        ([("A", item), ("A", "contracts/entropy/README.md")], False, "item plus contract readme"),
        ([("A", item), ("M", "contracts/spawn/v2/schema.zt")], True, "item plus modified schema"),
        ([("A", item), ("R100", "contracts/entropy/v1/schema.zt")], True, "item plus renamed schema"),
        ([("A", item), ("A", "contracts/entropy/v1/schema.zt"), ("A", "contracts/entropy/v1/gen_rust.zt")], True, "item plus schema plus generator"),
        ([("A", item), ("A", "contracts/entropy/v1/schema.zt"), ("M", "boot-contracts/src/generated/entropy.rs")], True, "item plus schema plus bindings"),
        ([("A", item), ("A", "contracts/entropy/v1/schema.zt"), ("M", "contracts/system-spec/v1/systems/sel4-entropy.zti")], True, "item plus schema plus system spec"),
        ([("A", item), ("A", "contracts/entropy/schema.zt")], True, "item plus unversioned schema"),
    ]
    for changes, refused, description in cases:
        verdict = pr_scope.judge(changes, code)
        if bool(verdict.findings) != refused:
            raise SystemExit(f"pr-scope control failed: {description}")
    if not pr_scope.judge([("A", "contracts/entropy/v1/schema.zt")], code).needs_landed_item:
        raise SystemExit("pr-scope control failed: a new schema alone escaped the landed-item rule")
    if pr_scope.classify("contracts/entropy/v1/schema.zt", code, "A") != pr_scope.INTERFACE:
        raise SystemExit("pr-scope control failed: added schema is interface")
    if pr_scope.classify("contracts/entropy/v1/schema.zt", code, "M") != pr_scope.PRODUCT:
        raise SystemExit("pr-scope control failed: modified schema is product")
    hatch = pr_scope.judge([("M", ".github/workflows/ci.yml"), ("M", "docs/x.md")], code)
    if hatch.needs_landed_item:
        raise SystemExit("pr-scope control failed: process-only diff demanded an item")
    for path in ("slime-root/src/main.rs", "Justfile", "scripts/lib/pr_scope.py"):
        if not pr_scope.judge([("M", path)], code).needs_landed_item:
            raise SystemExit(f"pr-scope control failed: {path} escaped the landed-item rule")
    if pr_scope.classify("Justfile", code) != pr_scope.GRADER:
        raise SystemExit("pr-scope control failed: Justfile is grader, not product")
    if pr_scope.classify("scripts/x", ()) != pr_scope.GRADER:
        raise SystemExit("pr-scope control failed: scripts/ is grader regardless of codePaths")
    if pr_scope.judge([("M", "contracts/system-image-closure/v2/closures/sel4.zti")], code).needs_landed_item:
        raise SystemExit("pr-scope control failed: regenerated closures alone demanded an item")
    print("pr-scope controls passed: 20 diffs judged, 11 refused")


def revision_controls() -> None:
    """Exercise the production comparison against committed temporary Git trees."""
    item = ".tasks/items/01a0ec61-d599-73ec-9e42-1ee6966420fd.md"
    next_item = ".tasks/items/01a0ec61-d599-73ec-9e42-1ee6966420fe.md"
    protected = ".github/workflows/protected.yml"
    schema = "contracts/example/v1/schema.zt"
    odd_path = "protected/tab\tand\nnewline.txt"
    base_code = [protected, "protected", "contracts", "slime-root"]
    passed = 0
    with tempfile.TemporaryDirectory(prefix="pr-scope-controls-") as directory:
        repo = Path(directory)

        def put(path: str, content: str) -> None:
            target = repo / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(content)

        def policy(entries: list[str]) -> str:
            return json.dumps({"codePaths": entries}) + "\n"

        def commit(message: str) -> str:
            git(repo, "add", "--all")
            git(repo, "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "commit", "--quiet", "--allow-empty", "-m", message)
            return resolve(repo, "HEAD")

        git(repo, "init", "--quiet")
        git(repo, "config", "user.name", "PR scope control")
        git(repo, "config", "user.email", "pr-scope@example.invalid")
        # Fixtures deliberately disagree with detection defaults to prove the CLI pins them.
        git(repo, "config", "diff.renames", "false")
        git(repo, "config", "diff.renameLimit", "1")
        files = {
            POLICY_PATH: policy(base_code),
            item: "existing task identity\n",
            protected: "protected workflow content\n",
            "protected/source.txt": "protected source content\n",
            odd_path: "odd pathname content\n",
            "docs/source.txt": "unprotected source content\n",
            "docs/future.txt": "future protected content\n",
            "slime-root/main.rs": "product implementation\n",
            schema: "schema example unique content\n",
        }
        for path, content in files.items():
            put(path, content)
        initial = commit("fixture base")

        def check(
            description: str,
            edits: dict[str, str | None],
            *,
            refused: bool = True,
            expected_rows: tuple[tuple[str, str], ...] = (),
            base_policy: str | None = "unchanged",
            invalid: bool = False,
            checkout_policy: str | None = None,
            unavailable: str | None = None,
            planning: bool = True,
        ) -> None:
            nonlocal passed
            git(repo, "reset", "--hard", initial)
            git(repo, "clean", "-fd")
            if base_policy != "unchanged":
                if base_policy is None:
                    (repo / POLICY_PATH).unlink()
                else:
                    put(POLICY_PATH, base_policy)
            base = commit(description + " base")
            for path, content in edits.items():
                if content is None:
                    (repo / path).unlink()
                else:
                    put(path, content)
            if planning:
                put(next_item, "new planning item\n")
            head = commit(description + " head")
            if checkout_policy is not None:
                put(POLICY_PATH, checkout_policy)
            compared_base = "missing-base-revision" if unavailable == "base" else base
            compared_head = "missing-head-revision" if unavailable == "head" else head
            print(f"pr-scope fixture: {description}; base={base}; head={head}")
            print(f"  compare(base={compared_base!r}, head={compared_head!r}, repo=<temporary repository>)")
            print(f"  policy commands: git show {base}:{POLICY_PATH}; git show {head}:{POLICY_PATH}")
            print(f"  diff command: git diff --name-status -z {' '.join(DIFF_OPTIONS)} {base} {head} --")
            print(git(repo, "diff", "--no-color", *DIFF_OPTIONS, base, head, "--"), end="")
            try:
                verdict = compare(compared_base, compared_head, repo)
            except ValueError as error:
                if not invalid:
                    raise SystemExit(f"pr-scope fixture failed: {description}: {error}") from error
                print(f"  exit=1 refused invalid input: {error}")
            else:
                if invalid or bool(verdict.findings) != refused:
                    raise SystemExit(f"pr-scope fixture failed: {description}: unexpected verdict {verdict!r}")
                rows = diff(base, head, repo)
                for row in expected_rows:
                    if row not in rows:
                        raise SystemExit(f"pr-scope fixture failed: {description}: missing {row!r} in {rows!r}")
                print(f"  parsed changes: {rows!r}")
                print(f"  exit={int(bool(verdict.findings))}; verdict={'refused' if verdict.findings else 'accepted'}")
                report(verdict)
                for finding in verdict.findings:
                    print(f"  diagnostic: {finding}")
            passed += 1

        removed = policy([entry for entry in base_code if entry != protected])
        check("removed base protection plus modification", {POLICY_PATH: removed, protected: "changed workflow\n"})
        check("removed base protection plus deletion", {POLICY_PATH: removed, protected: None})
        check("removed base protection plus rename", {POLICY_PATH: removed, protected: None, "docs/moved.yml": files[protected]}, expected_rows=(("D", protected), ("R100", "docs/moved.yml")))
        check("head-only protection plus modification", {POLICY_PATH: policy([*base_code, "docs/future.txt"]), "docs/future.txt": "changed future\n"})
        for checkout in (policy([]), "{malformed checkout policy", policy(["docs"])):
            check("divergent checkout policy " + repr(checkout), {protected: "changed workflow\n"}, checkout_policy=checkout)
        check("outward protected rename", {"protected/source.txt": None, "docs/moved.txt": files["protected/source.txt"]}, expected_rows=(("D", "protected/source.txt"), ("R100", "docs/moved.txt")))
        check("inward protected rename", {"docs/source.txt": None, "protected/moved.txt": files["docs/source.txt"]}, expected_rows=(("D", "docs/source.txt"), ("R100", "protected/moved.txt")))
        check("unchanged protected source copy", {"docs/copied.txt": files["protected/source.txt"]}, expected_rows=(("M", "protected/source.txt"), ("C100", "docs/copied.txt")))
        check("schema rename into new version", {schema: None, "contracts/example/v2/schema.zt": files[schema]}, expected_rows=(("D", schema), ("R100", "contracts/example/v2/schema.zt")))
        check("unchanged schema copy into new version", {"contracts/example/v2/schema.zt": files[schema]}, expected_rows=(("M", schema), ("C100", "contracts/example/v2/schema.zt")))
        check("task rename plus product", {item: None, next_item: files[item], "slime-root/main.rs": "changed product\n"}, planning=False, expected_rows=(("R100", next_item),))
        check("unchanged task copy plus product", {next_item: files[item], "slime-root/main.rs": "changed product\n"}, planning=False, expected_rows=(("C100", next_item),))
        check("tab and newline rename path bytes", {odd_path: None, "docs/odd\tand\nname.txt": files[odd_path]}, expected_rows=(("D", odd_path), ("R100", "docs/odd\tand\nname.txt")))
        invalid_policies = (
            ("missing policy", None),
            ("malformed JSON", "{"),
            ("non-object policy", "[]"),
            ("missing codePaths", "{}"),
            ("string codePaths", '{"codePaths":"protected"}'),
            ("non-string codePaths entry", '{"codePaths":[1]}'),
        )
        for description, content in invalid_policies:
            check("base " + description, {POLICY_PATH: policy(base_code)}, base_policy=content, invalid=True)
            check("head " + description, {POLICY_PATH: content}, invalid=True)
        for endpoint in ("base", "head"):
            check("unavailable " + endpoint, {}, unavailable=endpoint, invalid=True)
        check("valid planning exam", {"scripts/check/example.py": "exam content\n"}, refused=False)
        check("valid new schema planning", {"contracts/new/v1/schema.zt": "genuinely new schema\n"}, refused=False)
        check("valid process only", {"docs/new.md": "new documentation\n"}, planning=False, refused=False)
        check("divergent checkout cannot add protection", {"docs/new.md": "new documentation\n"}, checkout_policy=policy(["docs"]), refused=False)
        check("valid derived only", {"contracts/system-image-closure/v2/closures/example.zti": "derived closure\n"}, planning=False, refused=False)
    print(f"pr-scope revision controls passed: {passed} actual Git comparisons")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", help="base revision of the pull request")
    parser.add_argument("--head", help="head revision of the pull request")
    parser.add_argument("--controls", action="store_true", help="run the negative controls only")
    args = parser.parse_args()
    controls()
    if args.controls:
        revision_controls()
        return 0
    if not args.base or not args.head:
        parser.error("--base and --head are required unless --controls")
    try:
        verdict = compare(args.base, args.head)
    except (ValueError, OSError) as error:
        print(f"pr-scope: refused: {error}", file=_sys.stderr)
        return 1
    report(verdict)
    for finding in verdict.findings:
        print(f"pr-scope: {finding}")
    return 1 if verdict.findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
