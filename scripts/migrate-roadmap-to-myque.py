#!/usr/bin/env python3

"""Import the Markdown roadmap into MyQue's canonical ``.tasks/items/`` store.

One-shot. It allocates one UUIDv7 per roadmap declaration, writes a
``work-item/v1`` file per item, and commits the legacy map that lets historical
devlog references keep resolving without the roadmap headings staying
authoritative.

Three decisions are worth reading before changing anything here.

**Identity.** A fresh UUIDv7 is canonical. The old roadmap id is preserved as
MyQue's optional ``key`` only when it names exactly one item; ``B29`` and
``B30`` were each allocated twice, so those four items get no key and record
their legacy id in the body instead. Nothing downstream may resolve a
relationship through a key.

**Timestamps.** ``work-item/v1`` requires ``created``, and a terminal state
requires ``closed``. Closure dates are recovered from the repository's own
evidence (see :func:`roadmap_inventory.recover_closed`); 34 items — mostly
M-track work that predates the devlog — have no dated evidence anywhere. Those
get the migration date and an explicit body note saying the original date is
unrecorded. Inventing a plausible date would contradict the repository's rule
that a closure is only ever an observed fact.

**Ordering.** Each item's UUIDv7 timestamp field is backdated to its closure
date (or to the migration date when open), so creation order is chronological
and ``myque list`` reads like the roadmap did. This is why abbreviations must
be computed against the store rather than at a fixed width.
"""

from __future__ import annotations

import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parent / "lib"))

import argparse
import hashlib
import json
import re
import time
import uuid
from datetime import date

import roadmap_inventory as inventory
from harness import ROOT

TASKS = ROOT / ".tasks"
ITEMS = TASKS / "items"
LEGACY_MAP = TASKS / "legacy-roadmap-ids.json"

# The repository's timezone, matching the timestamps the devlog and the existing
# roadmap already use. `work-item/v1` requires an explicit offset.
OFFSET = "+08:00"

# Body sections MyQue recognises by convention. The roadmap keeps its
# architectural prose; only content that belongs to one item moves here.
DELIVERABLES = re.compile(r"^### (?:Deliverables|Scope)\s*$", re.MULTILINE)
EXIT_CONDITION = re.compile(r"^### Exit condition.*$", re.MULTILINE)
VERIFICATION = re.compile(r"^### (?:Planned verification target|Required checks|Verification.*)\s*$", re.MULTILINE)

# `**Evidence:**` and the devlog links in it.
EVIDENCE = re.compile(r"^\*\*Evidence[^:]*:\*\*(?P<text>.*(?:\n(?!\*\*|#|$).*)*)", re.MULTILINE)
WAS = re.compile(r"^\*\*Was:\*\*(?P<text>.*(?:\n(?!\*\*|#|$).*)*)", re.MULTILINE)
OBSERVED = re.compile(r"^\*\*Exit condition \(observed\):\*\*(?P<text>.*(?:\n(?!\*\*|#|$).*)*)", re.MULTILINE)
GATES = re.compile(r"`just ([a-z_0-9]+)`")

# The backlog records a resolution class alongside its status.
CLASS = re.compile(r"\*\*Class:\*\*\s*(?P<text>[^.]*\.)")


def uuid7(day: date, entropy: bytes) -> str:
    """A UUIDv7 whose timestamp field is midnight on ``day``.

    RFC 9562 §5.7: 48 bits of Unix milliseconds, then the version and variant
    markers over 74 bits of entropy. Backdating is what makes an imported
    store's creation order match the history it came from.
    """
    milliseconds = int(time.mktime(day.timetuple())) * 1000
    raw = bytearray(milliseconds.to_bytes(6, "big") + entropy[:10])
    raw[6] = 0x70 | (raw[6] & 0x0F)
    raw[8] = 0x80 | (raw[8] & 0x3F)
    return str(uuid.UUID(bytes=bytes(raw)))


def timestamp(day: date) -> str:
    return f"{day.isoformat()}T00:00:00{OFFSET}"


def section(body: str, pattern: re.Pattern[str]) -> str | None:
    """The text under the first heading matching ``pattern``, if present."""
    found = pattern.search(body)
    if found is None:
        return None
    rest = body[found.end() :]
    end = re.search(r"^#{2,4} ", rest, re.MULTILINE)
    text = (rest[: end.start()] if end else rest).strip()
    return text or None


def paragraph(body: str, pattern: re.Pattern[str]) -> str | None:
    found = pattern.search(body)
    if found is None:
        return None
    return " ".join(found.group("text").split()) or None


def compose_body(declaration: inventory.Declaration, title: str, note: str | None) -> str:
    """Build the item's Markdown body from its roadmap section.

    Only content that belongs to *this* item moves. Track-wide architecture
    prose stays in ``roadmap/``, which remains readable documentation.
    """
    lines = [f"# {title}", ""]

    if declaration.kind == "followup":
        # A deferred follow-up is one prose sentence under a shared heading,
        # with no Status/Deliverables/Exit structure to extract.
        lines += ["## Context", "", declaration.body, ""]
        lines += [
            "## Notes",
            "",
            f"Migrated from `roadmap/{declaration.source}`'s "
            f"`## Deferred follow-ups`, line {declaration.line}. The roadmap "
            "never assigned this work an id, so it carries no key; the item it "
            "was deferred from is recorded as a dependency.",
            "",
        ]
        return "\n".join(lines)

    context = [paragraph(declaration.body, inventory.STATUS)]
    context.append(paragraph(declaration.body, WAS))
    grade = CLASS.search(declaration.body)
    if grade is not None:
        context.append(f"Class: {' '.join(grade.group('text').split())}")
    if declaration.ambiguous:
        # The heading is the only thing that separates the two allocations, so
        # it is what the body records.
        allocation = "first" if declaration.line < AMBIGUOUS_SECOND[declaration.identifier] else "second"
        context.append(
            f"Legacy roadmap identifier: `{declaration.identifier}` ({allocation} allocation). "
            f"That id was allocated twice, so it is not carried as a key."
        )
    if note is not None:
        context.append(note)
    body_context = [line for line in context if line]
    if body_context:
        lines += ["## Context", ""] + [f"{line}\n" for line in body_context]

    scope = section(declaration.body, DELIVERABLES)
    if scope:
        lines += ["## Scope", "", scope, ""]

    exit_text = section(declaration.body, EXIT_CONDITION) or paragraph(declaration.body, OBSERVED)
    if exit_text:
        lines += ["## Exit conditions", "", exit_text, ""]

    checks = section(declaration.body, VERIFICATION)
    gates = sorted(set(GATES.findall(declaration.body)))
    verification: list[str] = []
    if checks:
        verification.append(checks)
    if gates:
        verification.append("Gates: " + ", ".join(f"`just {gate}`" for gate in gates))
    if verification:
        lines += ["## Verification", ""] + [f"{part}\n" for part in verification]

    evidence = paragraph(declaration.body, EVIDENCE)
    if evidence:
        lines += ["## Evidence", "", evidence, ""]

    lines += [
        "## Notes",
        "",
        f"Migrated from `roadmap/{declaration.source}` line {declaration.line}: "
        f"`{declaration.heading.lstrip('# ').strip()}`.",
        "",
    ]
    return "\n".join(lines)


def title_of(declaration: inventory.Declaration) -> str:
    """A one-line title: the heading text with its id and separator removed."""
    if declaration.kind == "followup":
        # A follow-up's prose is its own description. The title is its first
        # clause with the "X left" lead-in dropped, since the owning item is
        # already recorded as a dependency.
        first = re.split(r"(?<=[a-z])\.\s|\s+—\s+|;\s|,\s+so\s", declaration.body, maxsplit=1)[0]
        first = re.sub(r"^[A-Z]+[0-9.]*\s+left\s+", "", first).strip().rstrip(".")
        return (first[:88].rstrip() + "…") if len(first) > 89 else first
    text = declaration.heading.lstrip("#").strip()
    text = re.sub(rf"^{re.escape(declaration.identifier)}(?:\s+(?:—|--)\s+|:\s+)", "", text)
    # The backlog appends its resolution to the heading; the state field
    # carries that, so it does not belong in a title.
    text = re.sub(r"\s*[—-]+\s*\*\*resolved[^*]*\*\*\s*$", "", text, flags=re.IGNORECASE)
    return text.strip().rstrip(".")


def frontmatter(item: dict) -> str:
    lines = ["---", "schema: work-item/v1", f"id: {item['id']}"]
    if item.get("key"):
        lines.append(f"key: {item['key']}")
    lines += [f"kind: {item['kind']}", f"state: {item['state']}", f"created: {item['created']}"]
    if item.get("closed"):
        lines.append(f"closed: {item['closed']}")
    if item.get("tags"):
        lines.append("tags:")
        lines += [f"  - {tag}" for tag in item["tags"]]
    if item.get("parent"):
        lines.append(f"parent: {item['parent']}")
    if item.get("depends"):
        lines.append("depends:")
        lines += [f"  - {dependency}" for dependency in item["depends"]]
    lines.append("---")
    return "\n".join(lines) + "\n"


# The line number of the *second* allocation of each colliding id, so the body
# can say which one an item is. Filled in by `main`.
AMBIGUOUS_SECOND: dict[str, int] = {}

# Historical devlog entries that name a colliding id, and which allocation the
# repository's own evidence shows they meant. This is the one place the
# migration must not guess: an id that names two items cannot be resolved by
# the id, so each reference is resolved by hand from its entry, or recorded as
# undetermined.
#
# `B29` is referenced by no devlog entry at all. `B30` is referenced by exactly
# one, whose title reproduces the second allocation's heading verbatim
# ("`release_trust_check` was red, unregistered, and half-blind") and whose
# date matches that entry's resolution, so its target is a fact rather than an
# inference.
AMBIGUOUS_REFERENCES: dict[str, list[dict[str, str]]] = {
    "B29": [],
    "B30": [
        {
            "entry": "devlog/2026-08-07-b30-release-trust-gate",
            "allocation": "second",
            "evidence": (
                "The entry's title is the second allocation's heading text, and its "
                "`just release_trust_check` gate is that defect's gate."
            ),
        }
    ],
}


def allocations_of(items: list[dict], identifier: str) -> list[dict[str, str]]:
    """Every item a colliding id names, in roadmap source order."""
    return [
        {
            "uuid": item["id"],
            "source": f"roadmap/{item['declaration'].source}:{item['declaration'].line}",
            "heading": item["declaration"].heading,
        }
        for item in sorted(items, key=lambda i: i["declaration"].line)
        if item["declaration"].identifier == identifier
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--migration-date",
        default=date.today().isoformat(),
        help="the date recorded for items with no dated closure evidence",
    )
    parser.add_argument("--force", action="store_true", help="overwrite an existing .tasks/items")
    arguments = parser.parse_args()
    migration_day = date.fromisoformat(arguments.migration_date)

    declarations = inventory.declarations()
    # Only real roadmap ids resolve a parent or a dependency. A deferred
    # follow-up's synthetic identifier is a source coordinate, not an id.
    known = {d.identifier for d in declarations if not d.keyless}

    for identifier in inventory.COLLIDING_IDS:
        lines = sorted(d.line for d in declarations if d.identifier == identifier)
        if len(lines) != 2:
            raise SystemExit(f"{identifier}: expected exactly 2 allocations, found {len(lines)}")
        AMBIGUOUS_SECOND[identifier] = lines[1]

    if ITEMS.exists() and any(ITEMS.glob("*.md")) and not arguments.force:
        raise SystemExit(f"{ITEMS} already holds items; pass --force to replace them")
    ITEMS.mkdir(parents=True, exist_ok=True)
    if arguments.force:
        for stale in ITEMS.glob("*.md"):
            stale.unlink()

    # Deterministic entropy: the same roadmap produces the same store, so the
    # import is auditable and re-runnable. Identity still never depends on
    # scanning existing ids — each id is a function of its own source heading.
    items: list[dict] = []
    for declaration in declarations:
        closing = declaration.closed
        undated = declaration.terminal and closing is None
        if undated:
            closing = migration_day.isoformat()
        closed_day = date.fromisoformat(closing) if closing else migration_day
        seed = f"{declaration.source}:{declaration.line}:{declaration.heading}"
        entropy = hashlib.sha256(seed.encode()).digest()
        note = None
        if undated:
            note = (
                "The original closure date is unrecorded: no status line, section text, "
                "or linked devlog entry dates it. The `closed` timestamp is the migration "
                f"date ({migration_day.isoformat()}), not an observed closure."
            )
        items.append(
            {
                "id": uuid7(closed_day, entropy),
                "key": None if declaration.keyless else declaration.identifier,
                "kind": declaration.kind,
                "state": declaration.state,
                # `created` precedes `closed`; the roadmap does not record when
                # work started, so the closure date stands for both.
                "created": timestamp(closed_day),
                "closed": timestamp(closed_day) if declaration.terminal else None,
                "tags": list(declaration.tags),
                "declaration": declaration,
                "title": title_of(declaration),
                "note": note,
                "undated": undated,
            }
        )

    by_identifier: dict[str, str] = {}
    for item in items:
        declaration = item["declaration"]
        if not declaration.keyless:
            by_identifier[declaration.identifier] = item["id"]

    # Diagram hops only reach items whose prose declares no dependency at all.
    # Where a `**Depends on:**` line exists it wins, even when it names fewer
    # or different ids: `RP0` depends on "P0 and P1" while the diagram draws
    # `RP0 --> RP1`, and `C9`'s line names `C9.2`/`C9.4` where the diagram
    # draws the coarser `C9 --> IO0`. The prose is the more precise claim, and
    # the parent relation already connects a milestone to its slices.
    prose_silent = {
        declaration.identifier
        for declaration in declarations
        if declaration.depends_text is None and not declaration.keyless
    }
    from_diagram: dict[str, list[str]] = {}
    for source, target in inventory.diagram_dependencies(known):
        if target in prose_silent:
            from_diagram.setdefault(target, []).append(source)

    for item in items:
        declaration = item["declaration"]
        parent = inventory.parent_id(declaration.identifier, known)
        item["parent"] = by_identifier.get(parent) if parent else None
        declared = inventory.dependency_ids(declaration, known)
        drawn = from_diagram.get(declaration.identifier, []) if not declaration.keyless else []
        item["depends"] = [
            by_identifier[identifier]
            for identifier in dict.fromkeys(declared + drawn)
            if identifier in by_identifier
        ]
        item["diagram_edges"] = len([d for d in drawn if d in by_identifier])

    for item in items:
        declaration = item["declaration"]
        text = frontmatter(item) + "\n" + compose_body(declaration, item["title"], item["note"])
        (ITEMS / f"{item['id']}.md").write_text(text)

    legacy = {
        "note": (
            "Legacy roadmap id to canonical MyQue UUID. Written once by "
            "scripts/migrate-roadmap-to-myque.py. This is committed data, not a parse of "
            "roadmap/*.md: it lets historical devlog entries keep resolving without the "
            "roadmap headings remaining authoritative. New references use UUIDs."
        ),
        "migrated": migration_day.isoformat(),
        "ids": {
            identifier: uuid_text
            for identifier, uuid_text in sorted(by_identifier.items())
        },
        "ambiguous": {
            identifier: {
                "allocations": allocations_of(items, identifier),
                "devlog_references": [
                    dict(
                        reference,
                        uuid=allocations_of(items, identifier)[
                            0 if reference["allocation"] == "first" else 1
                        ]["uuid"],
                    )
                    for reference in AMBIGUOUS_REFERENCES[identifier]
                ],
            }
            for identifier in sorted(inventory.COLLIDING_IDS)
        },
    }
    LEGACY_MAP.write_text(json.dumps(legacy, indent=2, ensure_ascii=False) + "\n")

    undated_count = sum(1 for item in items if item["undated"])
    print(f"wrote {len(items)} work items to {ITEMS.relative_to(ROOT)}")
    print(f"  keys: {len(by_identifier)} unique, {len(items) - len(by_identifier)} ambiguous (no key)")
    print(f"  parents: {sum(1 for i in items if i['parent'])}")
    drawn_total = sum(int(i["diagram_edges"]) for i in items)
    print(
        f"  dependencies: {sum(len(i['depends']) for i in items)} "
        f"({drawn_total} from the track diagram, the rest from `**Depends on:**`)"
    )
    print(f"  closure dates: {undated_count} carry the migration date for want of evidence")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
