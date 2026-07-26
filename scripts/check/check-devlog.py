#!/usr/bin/env python3

"""Validate the devlog's structure, front matter, index, and cross-links.

The devlog is a mandatory record (see ``AGENTS.md``) that no runtime test
touches, so its only guard against drift is this checker. It enforces the
layout and front-matter contract documented in ``devlog/README.md``:

* every entry is a ``YYYY-MM-DD-short-topic/`` folder holding ``index.md``;
* front matter carries the exact field set, in order, with ``Kind``/``Status``
  drawn from the declared vocabularies;
* ``Roadmap`` ids resolve to a real roadmap heading and ``Gates`` name real
  Justfile targets;
* required ``##`` sections are present for the entry's kind, in template order;
* the README index lists every entry once, with matching date and status;
* every devlog path referenced anywhere in the repository exists, and every
  evidence sibling is linked from its ``index.md``.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import re
import subprocess

from harness import ROOT

DEVLOG = ROOT / "devlog"
README = DEVLOG / "README.md"
TEMPLATE = DEVLOG / "TEMPLATE.md"
ROADMAP = ROOT / "roadmap"
JUSTFILE = ROOT / "Justfile"

ENTRY_NAME = re.compile(r"^(\d{4})-(\d{2})-(\d{2})-[a-z0-9]+(?:-[a-z0-9]+)*$")

FIELD_ORDER = ["Date", "Kind", "Status", "Scope", "Roadmap", "Gates", "Trigger", "Baseline"]

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


def front_matter(text: str) -> dict[str, str]:
    fields: dict[str, str] = {}
    for line in text.splitlines():
        if not line.startswith("|"):
            if fields:
                break
            continue
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != 2 or cells[0] in {"Field", "---"} or set(cells[0]) == {"-"}:
            continue
        fields[cells[0]] = cells[1]
    return fields


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


def roadmap_ids() -> set[str]:
    ids: set[str] = set()
    for path in ROADMAP.glob("*.md"):
        for heading in re.findall(r"^#{2,3} (\S+)", path.read_text(), re.M):
            ids.add(heading.rstrip(":").rstrip("—").strip())
    return ids


def just_targets() -> set[str]:
    return set(re.findall(r"^([a-z_0-9]+):", JUSTFILE.read_text(), re.M))


KNOWN_IDS = roadmap_ids()
KNOWN_TARGETS = just_targets()

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
    row = dict(zip(index_header, cells))
    link = re.match(r"^\[[^\]]+\]\(([^)]+)/index\.md\)$", row.get("Entry", ""))
    if not link:
        continue
    target = link.group(1)
    if target in index_rows:
        fail(f"README index lists {target} more than once")
    index_rows[target] = (row.get("Date", ""), row.get("Status", ""))

for column in ("Date", "Entry", "Kind", "Status", "Roadmap"):
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
    fields = front_matter(text)

    present = [field for field in FIELD_ORDER if field in fields]
    if present != FIELD_ORDER:
        missing = [field for field in FIELD_ORDER if field not in fields]
        if missing:
            fail(f"{name}: front matter missing {', '.join(missing)}")
        else:
            fail(f"{name}: front-matter fields out of order: {present}")
        continue

    keys = list(fields)
    if keys[: len(FIELD_ORDER)] != FIELD_ORDER:
        fail(f"{name}: front-matter fields out of order: {keys[: len(FIELD_ORDER)]}")

    if fields["Date"] != name[:10]:
        fail(f"{name}: Date {fields['Date']} does not match the folder date {name[:10]}")

    kind = fields["Kind"]
    if kind not in KINDS:
        fail(f"{name}: Kind {kind!r} is not one of {', '.join(KINDS)}")

    status = fields["Status"]
    if status not in STATUSES:
        fail(f"{name}: Status {status!r} is not one of {', '.join(STATUSES)}")

    if fields["Roadmap"] != "none":
        for identifier in (part.strip() for part in fields["Roadmap"].split(",")):
            if identifier not in KNOWN_IDS:
                fail(f"{name}: Roadmap id {identifier!r} matches no roadmap/backlog heading")

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

    for target in re.findall(r"\]\(([^)#]+)\)", text):
        if target.startswith(("http://", "https://")):
            continue
        if not (entry / target).exists():
            fail(f"{name}: dead relative link {target}")

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
    for reference in re.findall(r"devlog/[0-9A-Za-z._/-]+", path.read_text()):
        reference = reference.rstrip(".`,;:)")
        if "YYYY-MM-DD" in reference:
            continue
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

print(f"devlog check passed: {len(entries)} entries, {len(index_rows)} indexed")
