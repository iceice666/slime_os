#!/usr/bin/env python3

"""Validate maintained documentation without fetching or reading a history corpus.

Local links and canonical work-item references are checked in every docs page,
retained roadmap page and PR template. Literal Just invocations are checked in
current documentation only: roadmap chronology and work-item evidence do not
promise that their recorded commands still exist. History URLs must pin a full
commit, but their destinations are never opened. Just metadata is read locally.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re
import shlex
import tempfile
from urllib.parse import unquote

from harness import ROOT
from just_metadata import recipes
from markdown_anchors import anchors, controls as anchor_controls
from work_items import UUID, identities

REQUIRED_ACTIVE_DOCUMENTS = (
    "README.md",
    "AGENTS.md",
    "CONTRIBUTING.md",
    "docs/README.md",
    "roadmap/README.md",
    ".github/PULL_REQUEST_TEMPLATE/change.md",
    ".github/PULL_REQUEST_TEMPLATE/system-change.md",
)
MARKDOWN_LINK = re.compile(r"\]\(([^)\s]+)\)")
EXTERNAL_LINK_PREFIXES = ("http://", "https://", "mailto:")
HISTORY_BASE = "https://git.justaslime.dev/iceice666/slime_os-history/"
HISTORY_URL = re.compile(re.escape(HISTORY_BASE) + r"[^\s<>`\)\]]*")
HISTORY_COMMIT = re.compile(re.escape(HISTORY_BASE) + r"src/commit/[0-9a-f]{40}(?:/|$)")
HISTORY_LINK = re.compile(r"\[[^\]]*\]\(" + re.escape(HISTORY_BASE) + r"[^)\s]+\)")
UUID_REFERENCE = re.compile(
    r"(?<![0-9a-f])[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(?![0-9a-f])"
)
CODE_SPAN = re.compile(r"(`+)([^`]+)\1")
FENCE = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")
RECIPE_NAME = re.compile(r"[a-zA-Z_][a-zA-Z_0-9-]*")


def active_documents(root: _Path = ROOT) -> tuple[_Path, ...]:
    """Discover whole documentation families, including future architecture pages."""
    return tuple(
        dict.fromkeys(
            (
                *(root / relative for relative in REQUIRED_ACTIVE_DOCUMENTS),
                *sorted((root / "docs").rglob("*.md")),
                *sorted((root / "roadmap").rglob("*.md")),
                *sorted((root / "contracts").rglob("*.md")),
                *sorted((root / ".tasks" / "items").glob("*.md")),
                *sorted((root / ".github" / "PULL_REQUEST_TEMPLATE").rglob("*.md")),
                *sorted((root / ".github").glob("pull_request_template.md")),
                *sorted((root / ".github").glob("PULL_REQUEST_TEMPLATE.md")),
            )
        )
    )


def checks_commands(path: _Path, root: _Path = ROOT) -> bool:
    """Current indexes are checked; retained delivery narratives are link-only."""
    relative = path.relative_to(root)
    return (
        relative.parts[:2] != (".tasks", "items")
        and (relative.parts[0] != "roadmap" or relative.parts[1:] == ("README.md",))
        and relative.parts[:4] != ("contracts", "generation-manifest", "v1", "compositions")
    )


def document_reference_failures(
    path: _Path,
    text: str,
    *,
    item_root: _Path = ROOT / ".tasks" / "items",
    known_items: frozenset[str] | None = None,
) -> list[str]:
    """Validate local Markdown destinations, fragments, and work-item links."""
    found: list[str] = []
    known = identities() if known_items is None else known_items
    linked_work_items: set[str] = set()
    for target in dict.fromkeys(HISTORY_URL.findall(text)):
        if not HISTORY_COMMIT.match(target):
            found.append(f"{path}: history URL must use src/commit/<full 40-hex commit>: {target}")
    for target in dict.fromkeys(MARKDOWN_LINK.findall(text)):
        if target.startswith(EXTERNAL_LINK_PREFIXES):
            continue
        base, _, fragment = target.partition("#")
        destination = (path.parent / unquote(base)) if base else path
        resolved = destination.resolve()
        is_work_item = resolved.parent == item_root.resolve() and resolved.suffix == ".md"
        reference = resolved.stem
        if is_work_item:
            linked_work_items.add(reference)
            if not UUID.fullmatch(reference):
                found.append(f"{path}: work-item link {target} does not use a canonical UUID")
                continue
            if reference not in known:
                found.append(f"{path}: work-item link {target} names absent UUID {reference}")
                continue
        if not destination.exists():
            found.append(f"{path}: dead relative link {target}")
        elif fragment and (
            not destination.is_file() or unquote(fragment) not in anchors(destination)
        ):
            found.append(
                f"{path}: link {target} names no heading or explicit anchor in {destination}"
            )

    # An archive URL names historical content, not an item in the current store.
    local_text = HISTORY_URL.sub("", text)
    for reference in dict.fromkeys(UUID_REFERENCE.findall(local_text)):
        if reference not in linked_work_items and reference not in known:
            found.append(f"{path}: references absent work-item UUID {reference}")
    return found


def literal_commands(text: str) -> list[str]:
    """Read inline code and shell/plain fenced examples, not ordinary prose or URLs."""
    literals: list[str] = []
    fence = ""
    shell = False
    outside: list[str] = []
    for line in text.splitlines():
        match = FENCE.match(line)
        if match:
            delimiter, language = match.groups()
            if not fence:
                fence = delimiter
                shell = language.strip() in {"", "sh", "bash", "shell", "console"}
            elif delimiter[0] == fence[0] and len(delimiter) >= len(fence) and not language.strip():
                fence = ""
            continue
        if fence:
            if shell:
                literals.append(line)
        else:
            outside.append(line)
    literals.extend(match.group(2) for match in CODE_SPAN.finditer("\n".join(outside)))
    return literals


def command_reference_failures(path: _Path, text: str, known_recipes: dict[str, dict]) -> list[str]:
    """Check literal recipe names; patterns and Just's own options are not recipes."""
    found: list[str] = []
    for literal in literal_commands(HISTORY_LINK.sub("", text)):
        # Match an invocation, not a historical URL containing a command name.
        for command in re.split(r"&&|\|\||;", literal):
            command = " ".join(command.split()).removeprefix("$ ")
            if not command.startswith("just "):
                continue
            try:
                words = shlex.split(command, comments=True)[1:]
            except ValueError as error:
                found.append(f"{path}: malformed literal Just command {command!r}: {error}")
                continue
            index = 0
            while index < len(words):
                name = words[index]
                # Global flags and variable assignments precede recipes. Options
                # taking a value consume that value, not a recipe reference.
                if name.startswith("-") or "=" in name:
                    index += 1
                    if name in {
                        "--justfile",
                        "-f",
                        "--working-directory",
                        "-d",
                        "--shell",
                        "--shell-arg",
                    }:
                        index += 1
                    elif name == "--set":
                        index += 2
                    continue
                # Recipe-family globs and example placeholders are not literals.
                if not RECIPE_NAME.fullmatch(name):
                    break
                if name not in known_recipes:
                    found.append(f"{path}: literal just command names absent recipe {name!r}")
                    break
                parameters = known_recipes[name].get("parameters", [])
                index += 1
                for parameter in parameters:
                    if index >= len(words):
                        break
                    if parameter.get("kind", "singular") != "singular":
                        index = len(words)
                        break
                    if parameter.get("default") is not None and words[index] in known_recipes:
                        break
                    index += 1
    return list(dict.fromkeys(found))


def controls() -> list[str]:
    """Prove broken references fail and historical-only commands remain harmless."""
    failures = [f"anchor control: {failure}" for failure in anchor_controls()]
    present = "00000000-0000-0000-0000-000000000001"
    absent = "00000000-0000-0000-0000-000000000002"
    with tempfile.TemporaryDirectory() as temporary:
        root = _Path(temporary)
        for relative in REQUIRED_ACTIVE_DOCUMENTS:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Required\n")
        source = root / "README.md"
        target = root / "target.md"
        target.write_text("# Existing heading\n")
        item_root = root / ".tasks" / "items"
        item_root.mkdir(parents=True)
        (item_root / f"{present}.md").write_text("# Present\n")
        cases = (
            ("valid", f"[doc](target.md#existing-heading) [item](.tasks/items/{present}.md)", ()),
            ("missing path", "[doc](missing.md)", ("dead relative link",)),
            ("missing fragment", "[doc](target.md#missing)", ("names no heading",)),
            ("noncanonical UUID", "[item](.tasks/items/not-an-id.md)", ("canonical UUID",)),
            ("absent UUID", f"[item](.tasks/items/{absent}.md)", ("absent UUID",)),
            ("absent plain UUID", absent, ("absent work-item UUID",)),
            (
                "immutable history",
                f"[just retired_check]({HISTORY_BASE}src/commit/{'a' * 40}/devlog/missing.md)",
                (),
            ),
            (
                "mutable history",
                f"[history]({HISTORY_BASE}src/branch/main/devlog/missing.md)",
                ("full 40-hex commit",),
            ),
        )
        for name, document, signals in cases:
            found = document_reference_failures(
                source, document, item_root=item_root, known_items=frozenset({present})
            )
            if len(found) != len(signals) or any(
                signal not in failure for signal, failure in zip(signals, found, strict=True)
            ):
                failures.append(f"reference control {name}: expected {signals}, got {found}")
        future = root / "docs" / "architecture" / "future.md"
        future.parent.mkdir(parents=True)
        future.write_text("# Future\n")
        future_contract = root / "contracts" / "future" / "README.md"
        future_contract.parent.mkdir(parents=True)
        future_contract.write_text("# Contract\n")
        discovered = active_documents(root)
        if future not in discovered:
            failures.append("discovery control: new architecture page was not discovered")
        if future_contract not in discovered or (item_root / f"{present}.md") not in discovered:
            failures.append(
                "discovery control: contract docs or work-item evidence was not discovered"
            )
        if any(not path.is_file() for path in discovered) or (root / "devlog").exists():
            failures.append("discovery control: docs-only tree requires missing files or devlog")
        command_cases = (
            (source, "`just current_check`", False),
            (source, "`just removed_check`", True),
            (source, "```sh\njust removed_check\n```", True),
            (source, "`just current_check removed_check`", True),
            (source, "`just\nremoved_check`", True),
            (source, "`just --quiet removed_check`", True),
            (source, f"[`just retired_check`]({HISTORY_BASE}src/commit/{'a' * 40}/old.md)", False),
            (
                source,
                f"[`just retired_check`]({HISTORY_BASE}src/commit/{'a' * 40}/old.md) `just removed_check`",
                True,
            ),
            (root / "roadmap" / "history.md", "`just retired_check`", False),
            (root / "roadmap" / "README.md", "`just removed_check`", True),
            (item_root / f"{present}.md", "`just retired_check`", False),
        )
        for path, document, expected in command_cases:
            found = (
                command_reference_failures(path, document, {"current_check": {}})
                if checks_commands(path, root)
                else []
            )
            if bool(found) != expected:
                failures.append(
                    f"command control {document!r}: expected failure={expected}, got {found}"
                )
    return failures


def main() -> int:
    failures = controls()
    known_recipes = recipes()
    known_items = identities()
    documents = active_documents()
    for path in documents:
        if not path.is_file():
            failures.append(f"{path}: maintained document is missing")
            continue
        text = path.read_text()
        failures.extend(document_reference_failures(path, text, known_items=known_items))
        if checks_commands(path):
            failures.extend(command_reference_failures(path, text, known_recipes))
    for failure in failures:
        print(f"docs: {failure.replace(f'{ROOT}/', '')}")
    if failures:
        print(f"docs check failed with {len(failures)} problem(s)")
        return 1
    print(
        f"docs: checked {len(documents)} maintained documents; local links, fragments, UUIDs and current Just commands pass"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
