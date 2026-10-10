#!/usr/bin/env python3

"""Validate the canonical work-item store and the repository's policy over it.

Two responsibilities, deliberately in one checker rather than two:

* **Store validity** is MyQue's own. This runs ``myque check``, which owns
  identity (UUID form, UUIDv7, duplicates, filename/id agreement), alias
  uniqueness, dangling references, parent and dependency cycles,
  state/``closed`` consistency, and schema conformance. Reimplementing any of
  that here would fork the schema, which the migration explicitly does not do.

* **Repository policy** is Slime OS's own and MyQue is generic, so it cannot
  live upstream: backlog-first ordering, and the choice that a spec-driven
  item's body is one fenced `zti` requirements block. The body rule is
  enforced by *devloop*, the project that owns that format — this checker only
  decides which items are subject to it and reports what devloop refused.

Adding this checker rather than extending an existing one is deliberate: the
work-item store is a new mechanism with its own execution boundary (an external
binary) and its own inputs, which is the case ``AGENTS.md`` reserves a
top-level checker for.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import contextlib
import datetime
import hashlib
import importlib.util
import io
import json
import re
import shutil
import subprocess
import tempfile
import tomllib
from unittest.mock import patch

from harness import ROOT
import devloop
import devloop_observations
import work_items
from work_items import ITEMS, identities, items, open_backlog
from work_item_retirement import RetirementError
from work_item_retirement_controls import check_retirement_controls
from work_item_retirement_publish_controls import (
    check_controls as check_retirement_publish_controls,
)
from work_item_closeout_publish_controls import (
    check_controls as check_closeout_publish_controls,
)


# Items whose identity is at or after this instant must carry a devloop record.
# The value is announced rather than derived from the identity of the item that
# proposed the rule: work created between a plan and its enforcement must stay
# valid, and a cutoff at that item's own timestamp would have retroactively
# refused a prose item that landed less than three minutes later. Moving it
# later is safe; moving it earlier retroactively refuses landed work.
# docs/decisions/mandatory-spec-driven-work-items.md owns the rationale.
SPEC_DRIVEN_FROM_MS = 1790812800000  # 2026-10-01T00:00:00Z

failures: list[str] = []


def identity_created_ms(identity: str) -> int:
    """The creation instant a UUIDv7 identity carries, in milliseconds.

    MyQue allocates UUIDv7, and ``myque check`` refuses any other form, so the
    leading 48 bits are the authority for when an item came into existence. The
    ``created`` frontmatter is not used: it is an independent field that can
    drift from the identity every durable reference resolves through.
    """
    return int(identity.replace("-", "")[:12], 16)


def fail(message: str) -> None:
    failures.append(message)


def run_myque_check(root: _Path = ROOT) -> list[str]:
    """Delegate store validation to MyQue, which owns the schema."""
    binary = shutil.which("myque")
    if binary is None:
        raise SystemExit(
            "myque is not on PATH: enter the repository dev shell (`nix develop`), "
            "which provides it, or run `nix run github:mozufu/myque -- check`"
        )
    finished = subprocess.run([binary, "check"], cwd=root, capture_output=True, text=True)
    if finished.returncode == 0:
        return []
    # `myque check` distinguishes its failures by status: 1 is findings, 2 is a
    # missing tracker or a usage error. Reporting them identically would send a
    # reader looking for a broken item when the store is simply absent.
    label = "reported findings" if finished.returncode == 1 else "could not read the store"
    details = [
        line.strip() for line in (finished.stdout + finished.stderr).splitlines() if line.strip()
    ]
    return [f"myque check {label}: {line}" for line in details or [f"exit {finished.returncode}"]]


def check_backlog_first() -> list[str]:
    """Backlog defects are cleared or explicitly deferred before track work.

    ``AGENTS.md``: "Resolve, defer, or block every open one before starting a
    new track milestone; a green verification suite is a precondition for
    milestone work, not a milestone itself." ``deferred`` and ``blocked`` are
    the explicit escapes the rule allows, so they satisfy it rather than
    breaking it.
    """
    blocking = open_backlog()
    active_milestones = [
        item for item in items() if item["kind"] == "milestone" and item["state"] == "active"
    ]
    if blocking and active_milestones:
        names = ", ".join(str(item["key"] or item["id"]) for item in blocking)
        opened = ", ".join(str(item["key"] or item["id"]) for item in active_milestones)
        return [
            f"backlog-first: {opened} is active while backlog items are neither "
            f"resolved nor explicitly deferred: {names}"
        ]
    return []


def spec_driven() -> list[dict[str, object]]:
    """Items that carry a devloop record, and are therefore spec-driven.

    The record is what admits an item to the convention. An item without one is
    ordinary work — legacy `work-item/v1` prose and plain-body v2 items both
    stay valid — so this deliberately does not guess from the body.
    """
    return [item for item in items() if "devloop" in item["consumers"]]


def check_devloop_bodies() -> list[str]:
    """Every spec-driven item's body and recorded semantics, per devloop.

    Slime OS chose `fenced-zti/v1`; devloop owns what that means and whether a
    payload, digest, profile, or helper identity is acceptable. A retired item
    has no body to validate — its requirements live in retained history — so
    only active records are subject.
    """
    subject = [item for item in spec_driven() if not item["retired"]]
    if not subject:
        return []
    findings = devloop.check_toolchain()
    if findings:
        return findings
    if not devloop.POLICY.is_file():
        return [
            f"{devloop.POLICY.relative_to(ROOT)} is missing: a spec-driven item cannot be "
            "validated without this repository's gate policy"
        ]
    for item in subject:
        identity = str(item["id"])
        try:
            devloop.validate(identity)
        except devloop.DevloopError as error:
            findings.extend(
                f"devloop {identity}: {line}" for line in str(error).splitlines() if line.strip()
            )
    return findings


def check_spec_driven_required(cutoff: int = SPEC_DRIVEN_FROM_MS) -> list[str]:
    """Every item created at or after the cutoff carries a devloop record.

    Enforcement is deliberately unconditional: no exemption by ``kind``,
    ``tag``, or state. An exemption any item could claim by choosing a kind
    would make the rule advisory, which is the policy that already produced zero
    adoption.

    The subject is decided by identity alone, so the rule resolves offline, and
    a retired item stays subject — MyQue preserves the consumer namespace in the
    terminal record, so retirement neither satisfies nor escapes this.
    """
    findings = []
    for item in items():
        identity = str(item["id"])
        if identity_created_ms(identity) < cutoff or "devloop" in item["consumers"]:
            continue
        where = ".tasks/terminal" if item["retired"] else ".tasks/items"
        findings.append(
            f"{identity} ({where}) carries no devloop record: items created on or after "
            f"{_instant(cutoff)} must be admitted with `just devloop admit`"
        )
    return findings


def _instant(milliseconds: int) -> str:
    """The cutoff as an operator reads it, so a refusal names a date not an int."""
    return (
        datetime.datetime.fromtimestamp(milliseconds / 1000, datetime.timezone.utc)
        .isoformat()
        .replace("+00:00", "Z")
    )


def check_terminal_records() -> list[str]:
    """A retired item must still say, readably, which terminal state it reached."""
    return work_items.corrupt_terminal_records()


def check_controls() -> None:
    """Exercise the real store and shared policy reader without historical files."""
    binary = shutil.which("myque")
    if binary is None:
        run_myque_check()  # Report the same prerequisite as the real-store path.
        return

    with tempfile.TemporaryDirectory(prefix="work-item-controls-") as temporary:
        root = _Path(temporary)

        def command(*arguments: str) -> None:
            result = subprocess.run([binary, *arguments], cwd=root, capture_output=True, text=True)
            if result.returncode:
                raise SystemExit(
                    f"work-item control setup failed ({' '.join(arguments)}): "
                    f"{result.stdout}{result.stderr}"
                )

        with patch.object(shutil, "which", return_value=None):
            try:
                run_myque_check(root)
            except SystemExit:
                pass
            else:
                fail("control: missing MyQue executable was accepted")

        if not any("no tracker found" in finding for finding in run_myque_check(root)):
            fail("control: missing store did not report the tracker prerequisite")
        command("init")
        command("new", "Backlog control", "--kind", "bug", "--key", "BACKLOG", "--tag", "backlog")
        command("new", "Milestone control", "--kind", "milestone", "--key", "MILESTONE")
        command("start", "MILESTONE")

        with patch.object(work_items, "ITEMS", root / ".tasks" / "items"):
            try:
                for transition, rejected in (
                    (None, True),
                    ("start", True),
                    ("defer", False),
                    ("block", False),
                    ("close", False),
                ):
                    if transition is not None:
                        command(transition, "BACKLOG")
                    findings = run_myque_check(root)
                    if findings:
                        fail(f"control: valid {transition} store rejected: {findings}")
                    items.cache_clear()
                    if bool(check_backlog_first()) != rejected:
                        fail(
                            f"control: backlog-first misclassified {transition} backlog with active milestone"
                        )
                command("reopen", "BACKLOG")
                command("reopen", "MILESTONE")
                items.cache_clear()
                if check_backlog_first():
                    fail("control: open backlog without active milestone was rejected")
            finally:
                items.cache_clear()

        command("new", "Dependency control", "--key", "DEPENDENCY")
        command("depend", "BACKLOG", "DEPENDENCY")
        if run_myque_check(root):
            fail("control: valid dependency graph was rejected")
        fixture_items = root / ".tasks" / "items"
        dependency = next(
            path for path in fixture_items.glob("*.md") if "key: DEPENDENCY\n" in path.read_text()
        )
        original = dependency.read_bytes()
        dependency.unlink()
        if not any(
            f"dangling depends reference to {dependency.stem}" in finding
            for finding in run_myque_check(root)
        ):
            fail("control: dangling dependency did not identify the missing target")
        # MyQue writes the envelope version it currently publishes, so the
        # refusal is exercised by replacing whatever version it wrote.
        dependency.write_bytes(
            re.sub(rb"schema: work-item/v\d+", b"schema: invalid/v1", original, count=1)
        )
        if not any(
            "unknown schema version: invalid/v1" in finding for finding in run_myque_check(root)
        ):
            fail("control: invalid store schema did not report the schema refusal")


def check_profile_controls() -> None:
    """Prove the body convention refuses its negative cases, offline.

    These run devloop's own refusals over the admitted item rather than
    asserting message text here: the profile belongs to devloop, so a control
    that re-encoded the rule would drift from it. None of them reach native
    validation, so the controls cost no compilation.

    A retired item has no body to mutate, so the controls need an active
    spec-driven item and report nothing when the store holds none. The summary
    line says how many were validated, so an empty run is visible rather than
    silently green.
    """
    active = [item for item in spec_driven() if not item["retired"]]
    if not active:
        return
    admitted = active[0]
    binary = shutil.which("myque")
    if binary is None:
        return
    finished = subprocess.run(
        [binary, "api", "get", str(admitted["id"])], cwd=ROOT, capture_output=True, text=True
    )
    if finished.returncode:
        fail(f"control: cannot read the admitted item: {finished.stderr.strip()}")
        return
    item = json.loads(finished.stdout)

    def refuses(name: str, mutate) -> None:
        payload = json.loads(finished.stdout)
        mutate(payload)
        try:
            devloop.render(payload)
        except devloop.DevloopError:
            return
        fail(f"control: devloop accepted {name}")

    if devloop.render(item).strip() == "":
        fail("control: devloop rendered the admitted item as nothing")
    refuses("a duplicated requirements block", lambda p: p.update(body=p["body"] + p["body"]))
    refuses("a missing requirements block", lambda p: p.update(body="\nOrdinary prose.\n"))
    refuses(
        "editable requirements prose beside the block",
        lambda p: p.update(body=p["body"] + "\nAlso required: something else.\n"),
    )
    refuses(
        "an unknown body profile",
        lambda p: p["consumers"].update(
            devloop=p["consumers"]["devloop"].replace("fenced-zti/v1", "fenced-zti/v9")
        ),
    )
    refuses(
        "a payload that no longer matches its digest",
        lambda p: p.update(body=p["body"].replace('problem = "', 'problem = "tampered ', 1)),
    )


def _identity_at(milliseconds: int, suffix: int) -> str:
    """A syntactically valid UUIDv7 that claims a given creation instant."""
    stamp = f"{milliseconds:012x}"
    return f"{stamp[:8]}-{stamp[8:]}-7000-8000-{suffix:012d}"


def _fixture_item(identity: str, *, spec_driven: bool) -> str:
    record = (
        "devloop:\n  recordSchema: devloop-consumer/v1\n  schema: dev-spec/v1\n"
        "  bodyProfile: fenced-zti/v1\n"
        if spec_driven
        else ""
    )
    return (
        f"---\nschema: work-item/v2\nid: {identity}\nkind: task\nstate: open\n"
        f"created: 2026-10-02T10:00:00Z\n{record}---\n\n# Cutoff control\n"
    )


def check_cutoff_controls() -> None:
    """The cutoff refuses only what it claims to, on both sides of the instant.

    Fixtures are derived from ``SPEC_DRIVEN_FROM_MS`` rather than hard-coded, so
    moving the cutoff moves the controls with it instead of leaving them
    asserting a date the rule no longer uses.
    """
    with tempfile.TemporaryDirectory(prefix="cutoff-controls-") as temporary:
        fixtures = _Path(temporary) / "items"
        fixtures.mkdir()
        cases = {
            "before": (_identity_at(SPEC_DRIVEN_FROM_MS - 1, 1), False),
            "after": (_identity_at(SPEC_DRIVEN_FROM_MS, 2), False),
            "after-admitted": (_identity_at(SPEC_DRIVEN_FROM_MS + 86_400_000, 3), True),
        }
        for name, (identity, spec_driven) in cases.items():
            (fixtures / f"{identity}.md").write_text(
                _fixture_item(identity, spec_driven=spec_driven)
            )
            if name == "before" and identity_created_ms(identity) >= SPEC_DRIVEN_FROM_MS:
                fail("control: the pre-cutoff fixture is not actually before the cutoff")

        with (
            patch.object(work_items, "ITEMS", fixtures),
            patch.object(work_items, "TERMINAL", _Path(temporary) / "absent"),
        ):
            work_items.cache_clear()
            try:
                findings = check_spec_driven_required()
            finally:
                work_items.cache_clear()

        refused = {
            identity for identity, _ in cases.values() if any(identity in f for f in findings)
        }
        if cases["after"][0] not in refused:
            fail("control: a post-cutoff item without a devloop record was accepted")
        if cases["before"][0] in refused:
            fail("control: a pre-cutoff prose item was refused")
        if cases["after-admitted"][0] in refused:
            fail("control: a post-cutoff item carrying a devloop record was refused")
        if len(findings) != 1:
            fail(f"control: the cutoff reported {len(findings)} findings, expected exactly 1")


# Tracked files a gate's outcome does not depend on, so they stay outside
# devloop's code identity by decision rather than by omission. Everything else
# `git ls-files` reports must be covered by `codePaths`: a checker, recipe,
# pin, or fixture that is not covered can change without staling evidence
# recorded against it. Prefixes end in `/`; other entries are exact paths.
CODE_IDENTITY_EXEMPT = (
    ".agents/",  # agent skills: instructions, not inputs to a gate
    ".claude/",
    ".devloop/",  # policy and inputs are bound by their own digests
    ".github/",  # CI orchestration runs the gates; it is not one
    ".tasks/",  # the store is the subject of evidence, not its input
    "assets/",
    "docs/",  # `docs_check` validates docs; that gate is `work-item-store`
    "roadmap/",
    ".envrc",
    ".gitignore",
    ".gitmodules",
    "AGENTS.md",
    "CLAUDE.md",
    "CONTRIBUTING.md",
    "README.md",
)


# Submodules whose content reaches a gate only through the commit
# `sel4/pins.toml` records and `just sel4_pin_check` asserts; that file is
# under `codePaths`, so re-pinning them stales evidence without walking a
# kernel tree into every identity. The pin is verified here, not assumed.
CODE_IDENTITY_PINNED = ("deps/sel4", "deps/rust-sel4-bcm2712-rpi5", "deps/rust-sel4-cv1800b-duo")
PINS = ROOT / "sel4" / "pins.toml"


def _tracked_files() -> list[tuple[str, str, str]]:
    """Every path Git tracks at the superproject: mode, object id, path."""
    finished = subprocess.run(
        ["git", "ls-files", "--stage"], cwd=ROOT, capture_output=True, text=True
    )
    if finished.returncode:
        raise RuntimeError(finished.stderr.strip() or "git ls-files failed")
    rows = []
    for line in finished.stdout.splitlines():
        meta, _, path = line.partition("\t")
        mode, object_id, _ = meta.split()
        rows.append((mode, object_id, path))
    return rows


def _pinned_commits() -> set[str]:
    with PINS.open("rb") as handle:
        pins = tomllib.load(handle)
    return {
        section["commit"]
        for section in pins.values()
        if isinstance(section, dict) and isinstance(section.get("commit"), str)
    }


def code_paths_coverage(
    code_paths: list[str],
    tracked: list[tuple[str, str, str]],
    exempt: tuple[str, ...] = CODE_IDENTITY_EXEMPT,
    pinned: tuple[str, ...] = CODE_IDENTITY_PINNED,
    pinned_commits: set[str] = frozenset(),
) -> list[str]:
    """Findings for tracked files outside both `codePaths` and the exemptions.

    A submodule gitlink is covered when a `codePaths` entry lies inside it —
    the superproject's tree does not enumerate the submodule's files, and the
    entries pin which of them the identity walks — or when its recorded commit
    is one `sel4/pins.toml` carries.
    """

    def under(path: str, prefix: str) -> bool:
        return path == prefix or path.startswith(prefix.rstrip("/") + "/")

    findings = []
    for mode, object_id, path in tracked:
        if mode == "160000":
            if any(under(entry, path) for entry in code_paths):
                continue
            if path in pinned:
                if object_id not in pinned_commits:
                    findings.append(
                        f"submodule {path} is at {object_id}, which sel4/pins.toml does not "
                        "pin: its content is outside devloop's code identity"
                    )
                continue
            findings.append(
                f"submodule {path} contributes nothing to devloop's code identity: "
                "list the directories a gate reads under codePaths, pin it in "
                "sel4/pins.toml and CODE_IDENTITY_PINNED, or exempt it"
            )
            continue
        if any(under(path, entry) for entry in code_paths):
            continue
        if any(under(path, e) if e.endswith("/") else path == e for e in exempt):
            continue
        findings.append(
            f"{path} is tracked but outside .devloop/policy.json codePaths: a change to it "
            "would not stale recorded evidence; cover it or exempt it in "
            "CODE_IDENTITY_EXEMPT with a reason"
        )
    return findings


def missing_code_paths(
    code_paths: list[str], tracked: list[tuple[str, str, str]], root: _Path
) -> list[str]:
    """`codePaths` entries that name nothing.

    An entry inside a submodule that is not checked out is not judged: the
    work-item job clones without submodules, so absence there says nothing
    about the entry. A checked-out submodule is judged like any other tree.
    """
    submodules = [path for mode, _, path in tracked if mode == "160000"]
    findings = []
    for entry in code_paths:
        if (root / entry).exists():
            continue
        owner = next((s for s in submodules if entry.startswith(s + "/")), None)
        if owner is not None and not (root / owner / ".git").exists():
            continue
        findings.append(f"codePaths names {entry}, which does not exist")
    return findings


def check_code_paths_coverage() -> list[str]:
    """Every tracked input a gate can read is inside devloop's code identity."""
    try:
        code_paths = json.loads((ROOT / ".devloop" / "policy.json").read_text())["codePaths"]
        tracked = _tracked_files()
        pinned_commits = _pinned_commits()
    except (OSError, ValueError, KeyError, RuntimeError) as error:
        return [f"cannot read codePaths coverage inputs: {error}"]
    findings = code_paths_coverage(code_paths, tracked, pinned_commits=pinned_commits)
    findings.extend(missing_code_paths(code_paths, tracked, ROOT))
    # devloop walks the filesystem, so an ignored file under a covered
    # directory would enter the identity and no other checkout could
    # reproduce it.
    finished = subprocess.run(
        ["git", "ls-files", "--others", "--ignored", "--exclude-standard", "--", *code_paths],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    for ignored in finished.stdout.split():
        findings.append(
            f"{ignored} is gitignored but under a codePaths entry, so devloop's code "
            "identity would record bytes no clone reproduces: remove it or narrow the entry"
        )
    return findings


def check_code_paths_controls() -> None:
    """The coverage rule catches the gaps it exists for and nothing else."""
    tracked = [
        ("100644", "b1", "scripts/check/check-new.py"),
        ("100644", "b2", "docs/plans/x.md"),
        ("100644", "b3", "slime-root/src/main.rs"),
        ("160000", "c1", "deps/other"),
        ("160000", "c2", "deps/zutai"),
        ("160000", "c3", "deps/sel4"),
        ("160000", "c4", "deps/rust-sel4-cv1800b-duo"),
        ("100644", "b4", "README.md"),
    ]
    paths = ["slime-root", "scripts/check/check-old.py", "deps/zutai/crates"]
    findings = code_paths_coverage(paths, tracked, pinned_commits={"c3"})
    flagged = {f.split(" ", 1)[0] for f in findings} | {
        f.split(" ")[1] for f in findings if f.startswith("submodule ")
    }
    if "scripts/check/check-new.py" not in flagged:
        fail("control: an uncovered checker was not reported")
    if "deps/other" not in flagged:
        fail("control: a submodule with no covered entry was not reported")
    if "deps/rust-sel4-cv1800b-duo" not in flagged:
        fail("control: a pinned submodule whose commit is not in sel4/pins.toml passed")
    covered_paths = (
        "docs/plans/x.md",
        "slime-root/src/main.rs",
        "deps/zutai",
        "deps/sel4",
        "README.md",
    )
    for covered in covered_paths:
        if covered in flagged:
            fail(f"control: {covered} was reported although covered, pinned, or exempt")
    if len(findings) != 3:
        fail(f"control: coverage reported {len(findings)} findings, expected 3")

    # Existence: an entry in an absent submodule is not judged; one in a
    # checked-out submodule, or outside any submodule, is.
    with tempfile.TemporaryDirectory(prefix="code-paths-controls-") as temporary:
        root = _Path(temporary)
        (root / "slime-root").mkdir()
        (root / "deps" / "zutai").mkdir(parents=True)
        (root / "deps" / "sel4").mkdir(parents=True)
        (root / "deps" / "sel4" / ".git").write_text("gitdir: elsewhere\n")
        entries = ["slime-root", "gone", "deps/zutai/crates", "deps/sel4/src"]
        missing = missing_code_paths(entries, tracked, root)
        if missing != [
            "codePaths names gone, which does not exist",
            "codePaths names deps/sel4/src, which does not exist",
        ]:
            fail(f"control: codePaths existence reported {missing}")


def _script_module(filename: str):
    """A sibling script loaded by path, because its filename is not an identifier."""
    location = _Path(__file__).resolve().parent / filename
    spec = importlib.util.spec_from_file_location(filename.replace("-", "_")[:-3], location)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _gate_module():
    return _script_module("devloop-gate.py")


def check_approval_controls() -> None:
    """Completion approval refuses a branch that rewrote its own exam.

    Offline: recipes and blobs are fixtures, so the controls pin what the rule
    compares — the inputs file, the recipe definitions in the dependency
    closure, the scripts they invoke and the `scripts/lib` modules those
    import — rather than whether Git happens to be reachable.
    """
    module = _script_module("devloop-approval.py")
    recipe = lambda body, deps=(): {  # noqa: E731
        "body": [[body]],
        "dependencies": [{"recipe": d, "arguments": [], "star": None} for d in deps],
        "parameters": [],
        "shebang": False,
    }
    canonical = {
        "plane_check": recipe("python3 scripts/check/check-sel4-qos-plane.py", ["pin_check"]),
        "pin_check": recipe("python3 scripts/check/check-sel4-pins.py"),
        "unrelated": recipe("echo unrelated"),
    }
    tree = json.loads(json.dumps(canonical))
    tree["added_later"] = recipe("echo new")

    if module.recipe_closure("plane_check", tree) != ["pin_check", "plane_check"]:
        fail("control: the recipe closure did not follow dependencies")
    if module.drifted_recipes("plane_check", tree, canonical):
        fail("control: an unchanged recipe closure was reported as drifted")
    tree["pin_check"]["body"] = [["true"]]
    if module.drifted_recipes("plane_check", tree, canonical) != [
        "recipe pin_check differs from origin/main"
    ]:
        fail("control: a weakened dependency recipe was not reported")
    tree["pin_check"] = canonical["pin_check"]
    tree["plane_check"]["dependencies"] = []
    if not module.drifted_recipes("plane_check", tree, canonical):
        fail("control: a dropped dependency was not reported")
    try:
        module.drifted_recipes("added_later", tree, {})
    except KeyError:
        fail("control: a recipe present in the tree raised instead of being reported")
    if module.drifted_recipes("added_later", tree, canonical) != [
        "recipe added_later is not on origin/main"
    ]:
        fail("control: a recipe absent from canonical main was not reported")

    scripts = module.recipe_scripts("plane_check", canonical)
    expected = {"scripts/check/check-sel4-qos-plane.py", "scripts/check/check-sel4-pins.py"}
    if scripts != expected:
        fail(f"control: recipe scripts resolved to {sorted(scripts)}")
    closure = module.lib_imports({"scripts/check/check-sel4-qos-plane.py"})
    # sel4_gate_markers is a direct import; system_image_closure is reached only
    # through closure_image, so it proves the walk is transitive.
    for expected_module in ("sel4_gate_markers", "closure_image", "system_image_closure"):
        if f"scripts/lib/{expected_module}.py" not in closure:
            fail(f"control: {expected_module} was not followed from the checker's imports")

    blobs = {"a.py": "1", "b.py": "2"}
    at_main = lambda p: {"a.py": "1", "b.py": "9"}.get(p)  # noqa: E731
    in_tree = lambda p: blobs.get(p)  # noqa: E731
    findings = module.drifted({"a.py", "b.py", "c.py"}, at_main, in_tree)
    if findings != ["b.py differs from origin/main", "c.py is not on origin/main"]:
        fail(f"control: blob drift reported {findings}")

    if module.named_recipes({"justTarget": "x"}) != ["x"]:
        fail("control: justTarget was not resolved as the named recipe")
    if module.named_recipes({"recipes": ["tasks_check", "docs_check"]}) != [
        "tasks_check",
        "docs_check",
    ]:
        fail("control: a fixed recipe list was not resolved")
    if module.named_recipes({"scenario": "none"}) != []:
        fail("control: inputs naming no recipe were not treated as exempt")
    for bad in ({"recipes": "x"}, ["x"], {"recipes": [1]}):
        try:
            module.named_recipes(bad)
        except ValueError:
            continue
        fail(f"control: malformed inputs {bad!r} were accepted")

    # The inputs digest must resolve to a tracked file; an untracked path with
    # the same bytes is not the landed decision.
    with tempfile.TemporaryDirectory(prefix="approval-controls-") as temporary:
        with patch.object(module, "ROOT", _Path(temporary)):
            (_Path(temporary) / "landed.json").write_text('{"justTarget": "x"}')
            digest = hashlib.sha256(b'{"justTarget": "x"}').hexdigest()
            if module.inputs_named(digest, ["landed.json"]) != "landed.json":
                fail("control: a tracked inputs file was not found by digest")
            if module.inputs_named(digest, []) is not None:
                fail("control: an untracked inputs file resolved by digest")
        with (
            patch.object(module, "tracked_inputs", lambda: []),
        ):
            reasons = module.exam_landed({"identity": {"inputs": digest}})
            if len(reasons) != 1 or ".devloop/inputs/" not in reasons[0]:
                fail("control: untracked execution inputs were approved")


def check_gate_controls() -> None:
    """`just-target` separates a check that failed from a gate that could not run.

    Conflating them would let a typo in an execution input be recorded as
    evidence that the repository is broken, or a real regression be dismissed as
    a harness problem. No target is executed here: both the recipe runner and
    the declared-target list are replaced, so the controls stay offline.
    """
    module = _gate_module()
    with tempfile.TemporaryDirectory(prefix="gate-controls-") as temporary:
        inputs = _Path(temporary) / "inputs.json"
        # devloop's execution identity, spelled out independently of the
        # adapter so a run key that drops a field is caught here.
        fields = ("requirements", "helper", "code", "policy", "inputs", "target", "image", "epoch")
        identity = {field: f"fixture-{field}" for field in fields}

        def request(payload: object, **changed: str) -> dict:
            inputs.write_text(json.dumps(payload))
            return {
                "inputs": str(inputs),
                "acceptance": {"id": "A1"},
                "execution": {"identity": identity | changed, "now": 0},
            }

        runs: list[str] = []
        outcome = [False]

        def recipe(target: str, environment: dict | None = None) -> tuple[bool, str]:
            runs.append(target)
            return outcome[0], "fixture output"

        fingerprints = iter(())

        def passed(observations: list[dict]) -> list[bool]:
            return [o["value"]["boolValue"] for o in observations]

        fixture = {"justTarget": "fixture_target"}
        # The adapter narrates what it ran to stderr. That is wanted under
        # devloop and misleading here, where a deliberately failing fixture
        # would print `failed` inside a passing `just tasks_check`.
        with (
            patch.object(module, "declared_targets", lambda: {"fixture_target"}),
            patch.object(module, "recipe", recipe),
            patch.object(module, "RUNS", _Path(temporary) / "runs"),
            patch.object(module, "code_fingerprint", lambda: next(fingerprints, "fixture-code")),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            observations = module.just_target(request(fixture))
            if passed(observations) != [False]:
                fail("control: a failing target was not reported as passed=false")
            if [o["id"] for o in observations] != ["passed"]:
                fail("control: the generic gate reported observations policy does not declare")

            # A second acceptance under the same identity is answered by the
            # first run, failure included: repeating a request is not a retry.
            outcome[0] = True
            if passed(module.just_target(request(fixture))) != [False] or len(runs) != 1:
                fail("control: a repeat request reran the recipe or retried a failure")

            # Any identity field that differs is a different claim.
            for field in fields:
                before = len(runs)
                module.just_target(request(fixture, **{field: "changed"}))
                if len(runs) != before + 1:
                    fail(f"control: a run was reused across a different {field}")

            # A run whose code closure changed underneath it is not kept.
            fingerprints = iter(("start", "end"))
            module.just_target(request(fixture, inputs="drifted"))
            fingerprints = iter(())
            before = len(runs)
            module.just_target(request(fixture, inputs="drifted"))
            if len(runs) != before + 1:
                fail("control: a run was kept although the code closure changed during it")

            # Nor is a run reused once its window has passed.
            with patch.object(module.time, "time", lambda: 10.0**12):
                before = len(runs)
                module.just_target(request(fixture))
                if len(runs) != before + 1:
                    fail("control: a run was reused after its reuse window")

            try:
                inputs.write_text(json.dumps(fixture))
                module.just_target({"inputs": str(inputs), "acceptance": {"id": "A1"}})
            except module.CannotRun:
                pass
            else:
                fail("control: the generic gate ran with no execution identity to bind")

        with patch.object(module, "declared_targets", lambda: {"fixture_target"}):
            for name, payload in {
                "an undeclared target": {"justTarget": "not_a_recipe"},
                "no justTarget": {"unrelated": True},
                "a non-object inputs file": ["fixture_target"],
            }.items():
                try:
                    module.just_target(request(payload))
                except module.CannotRun:
                    continue
                fail(f"control: the generic gate ran with {name}")


def check_observation_gate_controls() -> None:
    """`just-observations` reports only what policy declares, from this run only.

    The report is what lets a predicate name a count instead of a pass, so the
    controls pin where a count can come from: the checker that the recipe ran,
    under the gate's environment, typed as declared. A leftover report, an
    undeclared id, a mistyped value, or a checker vouching for `passed` are
    each refused as a gate that could not run, never recorded as evidence.
    """
    module = _gate_module()
    declared = {
        "passed": "bool",
        "transcriptDigest": "text",
        "casesObserved": "int",
        "negativeControlsRefused": "int",
    }
    with tempfile.TemporaryDirectory(prefix="observation-controls-") as temporary:
        inputs = _Path(temporary) / "inputs.json"
        inputs.write_text(json.dumps({"justTarget": "fixture_target"}))
        reports = _Path(temporary) / "reports"
        fields = ("requirements", "helper", "code", "policy", "inputs", "target", "image", "epoch")
        identity = {field: f"fixture-{field}" for field in fields}

        def request(**changed: str) -> dict:
            return {
                "inputs": str(inputs),
                "acceptance": {"id": "A1"},
                "execution": {"identity": identity | changed, "now": 0},
            }

        report: dict[str, object] = {}
        outcome = [True]
        seen_environment: list[dict | None] = []

        def recipe(target: str, environment: dict | None = None) -> tuple[bool, str]:
            seen_environment.append(environment)
            if report:
                with patch.dict(module.os.environ, environment or {}):
                    devloop_observations.record(**report)
            return outcome[0], "fixture output"

        def values(observations: list[dict]) -> dict[str, object]:
            out = {}
            for o in observations:
                kind = o["value"]["kind"]
                out[o["id"]] = o["value"][
                    {"bool": "boolValue", "int": "intValue", "text": "textValue"}[kind]
                ]
            return out

        with (
            patch.object(module, "declared_targets", lambda: {"fixture_target"}),
            patch.object(module, "declared_observations", lambda gate: dict(declared)),
            patch.object(module, "recipe", recipe),
            patch.object(module, "RUNS", _Path(temporary) / "runs"),
            patch.object(module, "code_fingerprint", lambda: "fixture-code"),
            patch.object(devloop_observations, "REPORTS", reports),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            # A passing recipe that reported nothing is not wired up: unresolved.
            try:
                module.just_observations(request())
            except module.CannotRun:
                pass
            else:
                fail("control: a passing recipe with no report was accepted")
            if seen_environment[-1].get(devloop_observations.ENVIRONMENT) != "fixture_target":
                fail("control: the recipe was not told which target it answers for")

            # A failing recipe may report nothing; passed is false and that is evidence.
            outcome[0] = False
            observed = values(module.just_observations(request(epoch="fail")))
            silent = {"passed", "transcriptDigest"}
            if observed.get("passed") is not False or set(observed) != silent:
                fail(f"control: a failing silent recipe reported {observed}")

            # A report is typed as declared and joined to the adapter's own fields.
            outcome[0] = True
            report.update({"casesObserved": 7, "negativeControlsRefused": 3})
            observed = values(module.just_observations(request(epoch="report")))
            expected = {"passed": True, "casesObserved": 7, "negativeControlsRefused": 3}
            if {k: observed.get(k) for k in expected} != expected:
                fail(f"control: a typed report was not passed through: {observed}")
            if observed.get("transcriptDigest") != hashlib.sha256(b"fixture output").hexdigest():
                fail("control: the transcript digest does not bind the recipe output")

            # The same identity is answered from the kept run, report included.
            runs = len(seen_environment)
            report.clear()
            report.update({"casesObserved": 999})
            again = values(module.just_observations(request(epoch="report")))
            if len(seen_environment) != runs or again.get("casesObserved") != 7:
                fail("control: a reused run did not answer with its own report")

            # A leftover report from another run cannot answer for this one.
            gate_environment = {devloop_observations.ENVIRONMENT: "fixture_target"}
            with patch.dict(module.os.environ, gate_environment):
                devloop_observations.record(casesObserved=123)
            report.clear()
            outcome[0] = True
            try:
                module.just_observations(request(epoch="stale"))
            except module.CannotRun:
                pass
            else:
                fail("control: a stale report answered for a recipe that reported nothing")

            for name, bad in {
                "an undeclared observation": {"undeclared": 1},
                "a mistyped observation": {"casesObserved": True},
                "a checker vouching for passed": {"passed": True},
                "a checker supplying the transcript digest": {"transcriptDigest": "x"},
            }.items():
                report.clear()
                report.update(bad)
                try:
                    module.just_observations(request(epoch=name))
                except module.CannotRun:
                    continue
                fail(f"control: the observation gate recorded {name}")

        # A different gate's run of the same recipe is not this gate's evidence.
        if module.run_key(request(), "just-target", "fixture_target") == module.run_key(
            request(), "just-observations", "fixture_target"
        ):
            fail("control: a just-target run would answer a just-observations acceptance")

    # Outside the gate the helper is inert, so a checker reports unconditionally.
    with patch.dict(module.os.environ, {}, clear=True):
        if devloop_observations.record(casesObserved=1) is not None:
            fail("control: the observations helper wrote a report outside the gate")


def check_terminal_controls() -> None:
    """A corrupt or non-terminal record is reported, never counted as done."""
    with tempfile.TemporaryDirectory(prefix="terminal-controls-") as temporary:
        terminal = _Path(temporary) / "terminal"
        terminal.mkdir()
        identity = "00000000-0000-7000-8000-00000000000a"
        cases = {
            "not JSON": "{",
            "wrong schema": json.dumps({"schema": "terminal-item/v9", "metadata": ""}),
            "no metadata": json.dumps({"schema": "terminal-item/v1"}),
            "still open": json.dumps(
                {
                    "schema": "terminal-item/v1",
                    "metadata": f"---\nschema: work-item/v2\nid: {identity}\nkind: task\n"
                    "state: open\ncreated: 2026-09-15T10:00:00Z\n---\n\n# Control\n",
                }
            ),
        }
        for name, content in cases.items():
            (terminal / f"{identity}.json").write_text(content)
            with patch.object(work_items, "TERMINAL", terminal):
                work_items.cache_clear()
                try:
                    if not work_items.corrupt_terminal_records():
                        fail(f"control: terminal record that is {name} was accepted")
                    if work_items.done(identity):
                        fail(f"control: terminal record that is {name} was counted as done")
                    if identity in work_items.identities():
                        fail(f"control: terminal record that is {name} claimed an identity")
                finally:
                    work_items.cache_clear()

        # A well-formed cancelled record resolves as identity, but is not done.
        (terminal / f"{identity}.json").write_text(
            json.dumps(
                {
                    "schema": "terminal-item/v1",
                    "metadata": f"---\nschema: work-item/v2\nid: {identity}\nkind: task\n"
                    "state: cancelled\ncreated: 2026-09-15T10:00:00Z\n"
                    "closed: 2026-09-16T10:00:00Z\n---\n\n# Control\n",
                }
            )
        )
        with patch.object(work_items, "TERMINAL", terminal):
            work_items.cache_clear()
            try:
                if work_items.corrupt_terminal_records():
                    fail("control: a valid cancelled terminal record was reported as corrupt")
                if identity not in work_items.identities():
                    fail("control: a retired identity did not resolve")
                if work_items.done(identity):
                    fail("control: a cancelled item was counted as done")
            finally:
                work_items.cache_clear()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--devloop-diagnostics", action="store_true")
    parser.add_argument("--diagnostic-controls", action="store_true")
    parser.add_argument("--observation-capacity", action="store_true")
    parser.add_argument("--capacity-controls", action="store_true")
    arguments = parser.parse_args()
    if arguments.observation_capacity or arguments.capacity_controls:
        from devloop_observation_capacity_exam import main as capacity_main
        from devloop_diagnostics_exam import ExamError

        try:
            return capacity_main(controls_only=arguments.capacity_controls)
        except ExamError as error:
            raise SystemExit(f"observation capacity exam failed: {error}") from error
    if arguments.devloop_diagnostics or arguments.diagnostic_controls:
        from devloop_diagnostics_exam import ExamError, main as diagnostics_main

        try:
            return diagnostics_main(controls_only=arguments.diagnostic_controls)
        except ExamError as error:
            raise SystemExit(f"devloop diagnostics exam failed: {error}") from error
    if not ITEMS.is_dir():
        raise SystemExit(
            f"{ITEMS.relative_to(ROOT)} does not exist: it is the repository's only "
            "work-item authority and nothing reconstructs it"
        )
    check_controls()
    check_cutoff_controls()
    check_gate_controls()
    check_observation_gate_controls()
    check_approval_controls()
    check_code_paths_controls()
    check_terminal_controls()
    failures.extend(check_retirement_controls())
    try:
        check_retirement_publish_controls()
    except RetirementError as error:
        fail(f"retirement publication control: {error}")
    try:
        check_closeout_publish_controls()
    except RetirementError as error:
        fail(f"closeout publication control: {error}")
    failures.extend(run_myque_check())
    failures.extend(check_backlog_first())
    failures.extend(check_spec_driven_required())
    failures.extend(check_terminal_records())
    failures.extend(check_devloop_bodies())
    failures.extend(check_code_paths_coverage())
    check_profile_controls()

    if failures:
        for failure in failures:
            print(f"work items: {failure}")
        raise SystemExit(f"work-item check failed with {len(failures)} problem(s)")

    total = len(identities())
    validated = len([item for item in spec_driven() if not item["retired"]])
    print(f"work-item check passed: {total} items, {validated} validated through devloop")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
