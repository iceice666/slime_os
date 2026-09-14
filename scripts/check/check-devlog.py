#!/usr/bin/env python3

"""Validate retained devlog records and active policy-document references.

Ordinary changes no longer require a devlog entry. The retained corpus remains
immutable until H3 archives it, so this checker continues to enforce the layout
and front-matter contract documented in ``devlog/README.md``:

* every entry is a ``YYYY-MM-DD-short-topic/`` folder holding ``index.md``;
* front matter carries the exact field set, in order, with ``Kind``/``Status``
  drawn from the declared vocabularies;
* ``Work items`` references resolve to a canonical work item in
  ``.tasks/items/``, and ``Gates`` name real Justfile targets;
* required ``##`` sections are present for the entry's kind, in template order;
* the README index lists every entry once, with matching date and status;
* every devlog path referenced anywhere in the repository exists, and every
  evidence sibling is linked from its ``index.md``.

The active policy documents are checked independently for local links,
``#fragment`` anchors, and canonical work-item links. This checker derives no
identity from roadmap headings or human keys. A work item *is* its UUID under
``.tasks/items/``, ``myque check`` validates that store, and
``scripts/lib/work_items.py`` answers only whether a UUID is in it.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re
import subprocess
import tempfile

from harness import ROOT
from just_metadata import targets as just_targets
from markdown_anchors import anchors, controls as anchor_controls
from work_items import UUID, identities

DEVLOG = ROOT / "devlog"
README = DEVLOG / "README.md"
TEMPLATE = DEVLOG / "TEMPLATE.md"
BACKLOG_INDEX = ROOT / "roadmap" / "00-backlog.md"

REQUIRED_ACTIVE_DOCUMENTS = (
    "AGENTS.md",
    "CONTRIBUTING.md",
    "README.md",
    "docs/README.md",
    "docs/decisions/README.md",
    "docs/plans/README.md",
    "docs/directions/README.md",
    "docs/getting-started/01-orientation.md",
    "docs/getting-started/04-first-change.md",
    "roadmap/README.md",
    "devlog/README.md",
    ".github/PULL_REQUEST_TEMPLATE/change.md",
    ".github/PULL_REQUEST_TEMPLATE/system-change.md",
)


def active_documents(root: _Path = ROOT) -> tuple[_Path, ...]:
    """Return policy entry points plus maintained documentation families."""
    return tuple(
        dict.fromkeys(
            (
                *(root / relative for relative in REQUIRED_ACTIVE_DOCUMENTS),
                *sorted((root / "docs" / "getting-started").rglob("*.md")),
                *sorted((root / ".github" / "PULL_REQUEST_TEMPLATE").rglob("*.md")),
                *sorted((root / "docs" / "decisions").rglob("*.md")),
                *sorted((root / "docs" / "plans").rglob("*.md")),
                *sorted((root / "docs" / "directions").rglob("*.md")),
                *sorted((root / "roadmap").rglob("*.md")),
            )
        )
    )


ACTIVE_DOCUMENTS = active_documents()
MARKDOWN_LINK = re.compile(r"\]\(([^)\s]+)\)")
FULL_MARKDOWN_LINK = re.compile(r"\[[^\]]*\]\([^)\s]+\)")
EXTERNAL_LINK_PREFIXES = ("http://", "https://", "mailto:")
DEVLOG_INDEX_LINK = re.compile(r"\d{4}-\d{2}-\d{2}-[a-z0-9]+(?:-[a-z0-9]+)*/index\.md")
UUID_REFERENCE = re.compile(
    r"(?<![0-9a-f])[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}(?![0-9a-f])"
)


ACTIVE_DOCUMENT_SET = frozenset(ACTIVE_DOCUMENTS)


def devlog_path_references(
    path: _Path,
    text: str,
    *,
    active_document_set: frozenset[_Path] = ACTIVE_DOCUMENT_SET,
) -> list[str]:
    """Find retained-corpus paths not already checked as active Markdown links."""
    if path in active_document_set:
        text = FULL_MARKDOWN_LINK.sub("", text)
    return list(
        dict.fromkeys(
            reference.rstrip(".`,;:)")
            for reference in re.findall(r"devlog/[0-9A-Za-z._/-]+", text)
            if "YYYY-MM-DD" not in reference
        )
    )


ENTRY_NAME = re.compile(r"^(\d{4})-(\d{2})-(\d{2})-[a-z0-9]+(?:-[a-z0-9]+)*$")

FIELD_ORDER = ["Date", "Kind", "Status", "Scope", "Work items", "Gates", "Trigger", "Baseline"]

KINDS = ["Defect", "Change", "Audit", "Decision"]

STATUSES = ["Investigating", "Root-caused", "Fixed", "Verified", "Monitoring", "Proposed"]

# Template order. Every entry's sections must be a subsequence of this list.
SECTIONS = [
    "Summary",
    "Observable symptom",
    "Investigation log",
    "Root cause",
    "Changes",
    "Regression guards",
    "Verification",
    "Decisions",
    "Open risks and follow-ups",
    "Artifacts and provenance",
    "Corrections",
]

# A kind declares the minimum evidence its claim needs. "Corrections" is never
# required: it only exists once a published entry has been corrected.
REQUIRED_SECTIONS = {
    "Defect": [s for s in SECTIONS if s != "Corrections"],
    "Change": [
        "Summary",
        "Changes",
        "Regression guards",
        "Verification",
        "Decisions",
        "Open risks and follow-ups",
        "Artifacts and provenance",
    ],
    "Audit": [
        "Summary",
        "Observable symptom",
        "Investigation log",
        "Changes",
        "Verification",
        "Open risks and follow-ups",
        "Artifacts and provenance",
    ],
    "Decision": [
        "Summary",
        "Changes",
        "Decisions",
        "Open risks and follow-ups",
        "Artifacts and provenance",
    ],
}

# A status that asserts an observed result must name the gate that observed it.
STATUS_REQUIRES_GATES = {"Fixed", "Verified", "Monitoring"}

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def front_matter_rows(text: str) -> list[tuple[str, str]]:
    """The front-matter table's two-cell rows, in file order, duplicates kept.

    Order and multiplicity are the point. Collapsing straight into a dict
    loses both: a trailing duplicate row overwrites the earlier value while
    adding no new key, so a second ``Work items | none`` would silently
    replace a real UUID and still satisfy a field-order check.
    """
    rows: list[tuple[str, str]] = []
    for line in text.splitlines():
        if not line.startswith("|"):
            if rows:
                break
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 2 or cells[0] in {"Field", "---"} or set(cells[0]) == {"-"}:
            continue
        rows.append((cells[0], cells[1]))
    return rows


def sections(text: str) -> list[str]:
    return [line[3:].strip() for line in text.splitlines() if line.startswith("## ")]


def ragged_rows(text: str) -> list[tuple[str, int, int]]:
    """Report table rows whose cell count disagrees with their header row.

    An unescaped ``|`` inside a cell silently splits the row and changes what
    the table says — including in the front-matter table parsed above.
    """
    ragged: list[tuple[str, int, int]] = []
    width = 0
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|") or not stripped.endswith("|"):
            width = 0
            continue
        cells = len(re.split(r"(?<!\\)\|", stripped.strip("|")))
        if width == 0:
            width = cells
        elif set(stripped) <= set("|-: "):
            continue
        elif cells != width:
            ragged.append((stripped, cells, width))
    return ragged


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
    for target in dict.fromkeys(MARKDOWN_LINK.findall(text)):
        if target.startswith(EXTERNAL_LINK_PREFIXES):
            continue
        base, _, fragment = target.partition("#")
        if path == README and DEVLOG_INDEX_LINK.fullmatch(base):
            continue
        destination = (path.parent / base) if base else path
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
        elif fragment and (not destination.is_file() or fragment not in anchors(destination)):
            found.append(
                f"{path}: link {target} names no heading or explicit anchor in {destination}"
            )

    if path != BACKLOG_INDEX:
        for reference in dict.fromkeys(UUID_REFERENCE.findall(text)):
            if reference not in linked_work_items and reference not in known:
                found.append(f"{path}: references absent work-item UUID {reference}")
    return found


def active_document_discovery_controls() -> list[str]:
    """Prove future maintained documentation enters the checked set."""
    failures: list[str] = []
    with tempfile.TemporaryDirectory() as temporary:
        root = _Path(temporary)
        for relative in REQUIRED_ACTIVE_DOCUMENTS:
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Required\n")
        expected = {
            root / "docs" / "getting-started" / "future.md",
            root / ".github" / "PULL_REQUEST_TEMPLATE" / "future.md",
            root / "docs" / "decisions" / "future.md",
            root / "docs" / "plans" / "future.md",
            root / "docs" / "directions" / "future.md",
            root / "roadmap" / "future.md",
        }
        for path in expected:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Future\n")
        discovered = set(active_documents(root))
        missing = sorted(path.relative_to(root) for path in expected - discovered)
        if missing:
            failures.append(f"future active documents were not discovered: {missing}")
    return failures


def active_document_controls() -> list[str]:
    """Prove invalid active paths, fragments, and work-item links are rejected."""
    failures: list[str] = []
    present = "00000000-0000-0000-0000-000000000001"
    absent = "00000000-0000-0000-0000-000000000002"
    with tempfile.TemporaryDirectory() as temporary:
        root = _Path(temporary)
        source = root / "policy.md"
        target = root / "target.md"
        item_root = root / ".tasks" / "items"
        target.write_text("# Existing heading\n")
        item_root.mkdir(parents=True)
        (item_root / f"{present}.md").write_text("# Present\n")

        cases = (
            (
                "valid references",
                "[doc](target.md#existing-heading)\n[item](.tasks/items/" + present + ".md)\n",
                (),
            ),
            ("dead path", "[doc](missing.md)\n", ("dead relative link",)),
            ("dead fragment", "[doc](target.md#missing)\n", ("names no heading",)),
            (
                "repeated dead fragment",
                "[doc](target.md#missing)\n[again](target.md#missing)\n",
                ("names no heading",),
            ),
            ("non-UUID item", "[item](.tasks/items/not-an-id.md)\n", ("canonical UUID",)),
            ("absent UUID link", "[item](.tasks/items/" + absent + ".md)\n", ("absent UUID",)),
            ("absent plain UUID", absent + "\n", ("absent work-item UUID",)),
            (
                "repeated absent plain UUID",
                f"{absent}\n{absent}\n",
                ("absent work-item UUID",),
            ),
        )
        for name, document, expected_signals in cases:
            found = document_reference_failures(
                source,
                document,
                item_root=item_root,
                known_items=frozenset({present}),
            )
            if len(found) != len(expected_signals) or any(
                signal not in failure
                for signal, failure in zip(expected_signals, found, strict=True)
            ):
                failures.append(f"{name}: expected {expected_signals}, got {found}")

        missing_devlog_path = "dev" + "log/missing/index.md"
        devlog_link = f"[`{missing_devlog_path}`]({missing_devlog_path})\n"
        link_failures = document_reference_failures(
            source,
            devlog_link,
            item_root=item_root,
            known_items=frozenset({present}),
        )
        path_references = devlog_path_references(
            source,
            devlog_link,
            active_document_set=frozenset({source}),
        )
        if len(link_failures) != 1 or path_references:
            failures.append(
                "devlog link: expected one Markdown failure and no duplicate path "
                f"reference, got {link_failures} and {path_references}"
            )

        bare_reference = f"{missing_devlog_path}\n"
        if devlog_path_references(
            source,
            bare_reference,
            active_document_set=frozenset({source}),
        ) != [missing_devlog_path]:
            failures.append("bare devlog path: expected one retained-corpus reference")

        non_active_link = f"[`{missing_devlog_path}`]({missing_devlog_path})\n"
        if devlog_path_references(
            source,
            non_active_link,
            active_document_set=frozenset(),
        ) != [missing_devlog_path]:
            failures.append("non-active devlog link: expected one path reference")
    return failures


def declared_just_targets() -> set[str]:
    """Every recipe in the fully imported repository Justfile."""
    return set(just_targets())


KNOWN_TARGETS = declared_just_targets()

# The anchor computation decides whether an inbound link is live, so its own
# rules are proven before they are trusted: a phantom anchor accepts a dead
# link and a missed one rejects a valid link, and neither shows a symptom at
# the destination.
for failure in anchor_controls():
    fail(f"anchor control: {failure}")

for failure in active_document_discovery_controls():
    fail(f"active document discovery control: {failure}")

for failure in active_document_controls():
    fail(f"active document control: {failure}")

for document in ACTIVE_DOCUMENTS:
    if not document.is_file():
        fail(f"active document is missing: {document.relative_to(ROOT)}")
        continue
    for failure in document_reference_failures(document, document.read_text()):
        fail(failure.replace(f"{ROOT}/", ""))

entries = sorted(path for path in DEVLOG.iterdir() if path.is_dir())
if not entries:
    raise SystemExit("devlog contains no entries")

for stray in sorted(DEVLOG.glob("*.md")):
    if stray.name not in {"README.md", "TEMPLATE.md"}:
        fail(f"{stray.name}: flat entry file; every entry is a folder with an index.md")

# The index table is parsed by column name, not position, so reordering its
# columns is a formatting choice rather than a checker change.
index_rows: dict[str, tuple[str, str]] = {}
index_header: list[str] = []
for line in README.read_text().splitlines():
    if not line.startswith("| "):
        continue
    cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
    if cells[:2] == ["Date", "Entry"]:
        index_header = cells
        continue
    if not index_header or len(cells) != len(index_header):
        continue
    row = dict(zip(index_header, cells, strict=True))
    link = re.match(r"^\[[^\]]+\]\(([^)]+)/index\.md\)$", row.get("Entry", ""))
    if not link:
        continue
    target = link.group(1)
    if target in index_rows:
        fail(f"README index lists {target} more than once")
    index_rows[target] = (row.get("Date", ""), row.get("Status", ""))

for column in ("Date", "Entry", "Kind", "Status", "Keys"):
    if column not in index_header:
        fail(f"README index table is missing the {column!r} column")

for entry in entries:
    name = entry.name
    index = entry / "index.md"
    if not index.is_file():
        fail(f"{name}: missing index.md")
        continue

    if not ENTRY_NAME.match(name):
        fail(f"{name}: folder name is not YYYY-MM-DD-short-topic in lowercase kebab-case")

    text = index.read_text()
    rows = front_matter_rows(text)
    declared = [field for field, _ in rows]
    duplicated = sorted({field for field in declared if declared.count(field) > 1})
    if duplicated:
        fail(
            f"{name}: front matter repeats {', '.join(duplicated)}; a repeated row "
            "silently overrides the first one's value"
        )
        continue
    if declared != FIELD_ORDER:
        missing = [field for field in FIELD_ORDER if field not in declared]
        extra = [field for field in declared if field not in FIELD_ORDER]
        if missing:
            fail(f"{name}: front matter missing {', '.join(missing)}")
        if extra:
            fail(f"{name}: front matter carries unknown field(s) {', '.join(extra)}")
        if not missing and not extra:
            fail(f"{name}: front-matter fields out of order: {declared}")
        continue
    fields = dict(rows)

    if fields["Date"] != name[:10]:
        fail(f"{name}: Date {fields['Date']} does not match the folder date {name[:10]}")

    kind = fields["Kind"]
    if kind not in KINDS:
        fail(f"{name}: Kind {kind!r} is not one of {', '.join(KINDS)}")

    status = fields["Status"]
    if status not in STATUSES:
        fail(f"{name}: Status {status!r} is not one of {', '.join(STATUSES)}")

    if fields["Work items"] != "none":
        known = identities()
        for reference in (part.strip() for part in fields["Work items"].split(",")):
            if reference in known:
                continue
            if UUID.match(reference):
                fail(
                    f"{name}: Work items names {reference!r}, which is not a work item "
                    "in .tasks/items/"
                )
            else:
                fail(
                    f"{name}: Work items names {reference!r}, which is not a UUID. A human "
                    "key is a display alias and resolves nothing; cite the item's UUID"
                )

    gates = re.findall(r"`just ([a-z_0-9]+)`", fields["Gates"])
    if fields["Gates"] == "none":
        if status in STATUS_REQUIRES_GATES:
            fail(f"{name}: Status {status} claims an observed result but Gates is none")
    elif not gates:
        fail(f"{name}: Gates must be `just <target>` entries or none, got {fields['Gates']!r}")
    for gate in gates:
        if gate not in KNOWN_TARGETS:
            fail(f"{name}: Gates names {gate!r}, which is not a Justfile target")

    found = sections(text)
    unknown = [section for section in found if section not in SECTIONS]
    if unknown:
        fail(f"{name}: unknown section(s) {unknown}; extend TEMPLATE.md first")
    ordered = [SECTIONS.index(section) for section in found if section in SECTIONS]
    if ordered != sorted(ordered):
        fail(f"{name}: sections are out of template order: {found}")
    if len(set(found)) != len(found):
        fail(f"{name}: duplicate section heading in {found}")
    for required in REQUIRED_SECTIONS.get(kind, []):
        if required not in found:
            fail(f"{name}: Kind {kind} requires a '## {required}' section")

    for row, cells, width in ragged_rows(text):
        fail(
            f"{name}: table row has {cells} cells where its header has {width}; "
            f"escape a literal '|' as '\\|' -> {row}"
        )

    for sibling in sorted(entry.iterdir()):
        if sibling.name == "index.md":
            continue
        if sibling.name not in text:
            fail(f"{name}: evidence file {sibling.name} is not referenced from index.md")

    # A fragment is validated, not stripped. 77 merged entries link at a
    # specific heading, so a reworded one silently breaks an inbound URL: the
    # file still exists and the old anchor lands the reader at the top of the
    # page. Checking the file alone would claim a guarantee it does not give.
    # `markdown_anchors` owns which fragments a file offers, and its own rules
    # are proven by the controls run above.
    for target in re.findall(r"\]\(([^)\s]+)\)", text):
        if target.startswith(("http://", "https://", "mailto:")):
            continue
        base, _, fragment = target.partition("#")
        destination = (entry / base) if base else index
        if not destination.exists():
            fail(f"{name}: dead relative link {target}")
            continue
        if not fragment:
            continue
        if fragment not in anchors(destination):
            fail(
                f"{name}: link {target} names no heading or explicit anchor in "
                f"{destination.relative_to(ROOT)}"
            )

    if name not in index_rows:
        fail(f"{name}: not registered in devlog/README.md")
    else:
        listed_date, listed_status = index_rows[name]
        if listed_date != fields["Date"]:
            fail(f"{name}: README index date {listed_date} != entry Date {fields['Date']}")
        if listed_status != status:
            fail(f"{name}: README index status {listed_status!r} != entry Status {status!r}")

for target in index_rows:
    if not (DEVLOG / target / "index.md").is_file():
        fail(f"README index links {target}/index.md, which does not exist")

# Include untracked-but-not-ignored files: a brand-new entry is untracked until
# it is committed, and its links must be checked before that, not after.
listed = subprocess.run(
    ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
    cwd=ROOT,
    check=True,
    text=True,
    capture_output=True,
).stdout.split("\0")
for relative in dict.fromkeys(listed):
    if not relative or not relative.endswith((".md", ".py")):
        continue
    path = ROOT / relative
    if not path.is_file():
        continue
    for reference in devlog_path_references(path, path.read_text()):
        if not (ROOT / reference).exists():
            fail(f"{relative}: references {reference}, which does not exist")

template_sections = sections(TEMPLATE.read_text())
if [section for section in template_sections if section in SECTIONS] != [
    section for section in SECTIONS if section in template_sections
]:
    fail("TEMPLATE.md section order disagrees with the checker's SECTIONS order")
for kind, required in REQUIRED_SECTIONS.items():
    for section in required:
        if section not in template_sections:
            fail(f"TEMPLATE.md is missing '## {section}', required by Kind {kind}")

if failures:
    for failure in failures:
        print(f"devlog: {failure}")
    raise SystemExit(f"devlog check failed with {len(failures)} problem(s)")

print(
    f"devlog check passed: {len(entries)} entries, {len(index_rows)} indexed; "
    f"{len(ACTIVE_DOCUMENTS)} active documents"
)
