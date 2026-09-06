"""The roadmap's work-item declarations, parsed from ``roadmap/*.md``.

This module exists for one migration: it reads the legacy Markdown roadmap so
that :mod:`scripts.migrate_roadmap_to_myque` can allocate one MyQue work item
per declaration. Once ``.tasks/items/`` is canonical, nothing else may derive
identity from these headings — the committed legacy map does that instead.

A *declaration* is a level-2 or level-3 heading whose first token is a roadmap
id followed by an em dash or a colon. That is deliberately narrower than "any
heading": the roadmap also uses headings for track prose (``## Boundaries``),
per-item subsections (``### Deliverables``), and range placeholders
(``### P5.4.2 … P5.4.n``), none of which are work items.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

from harness import ROOT

ROADMAP = ROOT / "roadmap"

# A roadmap id: a letter prefix, a number, then dotted segments that may be a
# number (`C8.13`), a number with a disambiguating letter (`M5.2a`), a single
# uppercase letter (`P3.D`, `P6.A`), or a word (`P5.4.final`).
ID = r"[A-Z]{1,2}[0-9]+(?:\.(?:[0-9]+[a-z]?|[A-Z]|[a-z]+))*"

# The heading form that *declares* an item, as opposed to merely mentioning an
# id. The separator is required: `### M5 acceptance and verification stack` is a
# subsection of M5, not a second declaration of it.
DECLARATION = re.compile(rf"^(?P<hashes>#{{2,3}}) (?P<id>{ID})(?:\s+(?:—|--)\s+|:\s+)")

# `**Status:**` and its continuation lines, up to the next bold lead-in,
# heading, or blank line. Status prose wraps, and the closure date is often on
# the second line.
STATUS = re.compile(r"^\*\*Status:\*\*(?P<text>.*(?:\n(?!\*\*|#|$).*)*)", re.MULTILINE)

# `**Depends on:**` is prose, but it is the roadmap's only per-item dependency
# declaration and it covers three times as many items as the README's diagram.
DEPENDS = re.compile(r"^\*\*Depends on:\*\*(?P<text>.*(?:\n(?!\*\*|#|$).*)*)", re.MULTILINE)

DATE = re.compile(r"\b(20[0-9]{2}-[0-9]{2}-[0-9]{2})\b")

# A devlog folder reference. The folder's date prefix is when the work was
# recorded, which for a closed item is the best available closure evidence.
DEVLOG_LINK = re.compile(r"devlog/(20[0-9]{2}-[0-9]{2}-[0-9]{2})-[0-9a-z-]+")

# An inclusive numeric range of ids, e.g. `B39–B49` or `M1-M4`.
ID_RANGE = re.compile(r"\b([A-Z]{1,2})([0-9]+)\s*[–-]\s*(?:[A-Z]{1,2})?([0-9]+)\b")

ID_MENTION = re.compile(rf"\b({ID})\b")

# One hop of a Mermaid chain in `roadmap/README.md`, with its own arrow style
# and optional label. Parsed per hop rather than per line, because a chain such
# as `RP0 --> RP1 --> RP2 -.->|deferred| RP3` mixes styles on one line.
DIAGRAM_HOP = re.compile(r"(?P<arrow>-\.->|-->)(?:\|(?P<label>[^|]*)\|)?")

# `B29` and `B30` were each allocated twice before the backlog froze heading
# text. Their ids cannot identify an item, so the migration keys them by source
# heading and gives them no MyQue `key`.
COLLIDING_IDS = frozenset({"B29", "B30"})

# Roadmap state vocabulary, mapped to `work-item/v1` states. Order matters:
# "superseded" outranks "complete" because a superseded item's prose often
# describes what completed before it was replaced.
STATE_RULES: tuple[tuple[str, str], ...] = (
    (r"\bsuperseded\b", "cancelled"),
    (r"\b(complete|resolved|verified|delivered|done)\b", "done"),
    (r"\bblocked\b", "blocked"),
    (r"\bdefer", "deferred"),
    (r"\bin progress\b", "active"),
    (r"\b(not started|planned|later)\b", "open"),
)

# Track file to tag. The backlog is a defect log rather than a track, so its
# items carry `backlog` and take their kind from that.
TRACK_TAGS: dict[str, str] = {
    "00-backlog.md": "backlog",
    "01-foundations.md": "foundations",
    "02-core-runtime.md": "core-runtime",
    "03-ros2-compatibility.md": "ros2",
    "04-platform-hardware.md": "hardware",
    "05-foreign-workloads.md": "foreign-workloads",
    "06-authority-trust.md": "authority",
    "07-architecture-portability.md": "architecture",
    "08-native-development.md": "native-development",
    "09-rpi5-ros2-demo.md": "rpi5",
    "10-component-platform.md": "component-platform",
    "11-io-substrate.md": "io",
}


@dataclass
class Declaration:
    """One roadmap work-item declaration and the section it heads."""

    identifier: str
    source: str
    """The track file name, e.g. ``00-backlog.md``."""
    line: int
    heading: str
    """The heading line verbatim. This disambiguates a colliding id."""
    level: int
    body: str
    """The section text, from its heading to the next declaration."""
    state: str = "open"
    status_text: str = ""
    closed: str | None = None
    closed_source: str = "none"
    """How ``closed`` was recovered: ``status``, ``devlog-link``, ``body``, or
    ``none``. ``none`` means no evidence in the repository dates this closure."""
    depends_text: str | None = None
    tags: list[str] = field(default_factory=list)
    kind: str = "milestone"
    keyless: bool = False
    """Whether this item deliberately carries no ``key``. True for the
    backlog's deferred follow-ups, which are real work the roadmap never
    assigned an id to, and for the two ids allocated twice."""

    @property
    def terminal(self) -> bool:
        return self.state in {"done", "cancelled"}

    @property
    def ambiguous(self) -> bool:
        """Whether this id names more than one historical item."""
        return self.identifier in COLLIDING_IDS


def classify_state(status_text: str) -> str:
    """Map a ``**Status:**`` paragraph onto a ``work-item/v1`` state."""
    collapsed = " ".join(status_text.lower().split())
    for pattern, state in STATE_RULES:
        if re.search(pattern, collapsed):
            return state
    # Every current declaration matches a rule. A new one that does not is a
    # migration input the author has to classify, not something to guess at.
    raise ValueError(f"unclassifiable status: {status_text.strip()[:120]!r}")


def recover_closed(declaration: Declaration) -> tuple[str | None, str]:
    """Recover a closure date from the strongest available evidence.

    The ladder is deliberate: a date the author wrote on the status line is a
    claim about when the item closed, a linked devlog folder is when the
    closing work was recorded, and a bare date elsewhere in the section is
    weaker still. Nothing here invents a date — an item with no evidence
    reports ``none`` so the caller decides explicitly.
    """
    dated = DATE.search(declaration.status_text)
    if dated is not None:
        return dated.group(1), "status"
    links = DEVLOG_LINK.findall(declaration.body)
    if links:
        # The latest linked entry: an item that reopened and closed again
        # links both, and the closure is the last event.
        return max(links), "devlog-link"
    fallback = DATE.search(declaration.body)
    if fallback is not None:
        return fallback.group(1), "body"
    return None, "none"


def parse_track(name: str, text: str) -> list[Declaration]:
    """Every declaration in one track file, in source order."""
    lines = text.splitlines()
    starts = [
        (index, match)
        for index, line in enumerate(lines)
        if (match := DECLARATION.match(line)) is not None
    ]
    declarations: list[Declaration] = []
    for position, (index, match) in enumerate(starts):
        end = starts[position + 1][0] if position + 1 < len(starts) else len(lines)
        declaration = Declaration(
            identifier=match.group("id"),
            source=name,
            line=index + 1,
            heading=lines[index],
            level=len(match.group("hashes")),
            body="\n".join(lines[index:end]),
        )
        status = STATUS.search(declaration.body)
        if status is None:
            raise ValueError(f"{name}:{declaration.line}: {declaration.identifier} declares no Status")
        declaration.status_text = status.group("text").strip()
        declaration.state = classify_state(declaration.status_text)
        declaration.closed, declaration.closed_source = recover_closed(declaration)
        depends = DEPENDS.search(declaration.body)
        declaration.depends_text = depends.group("text").strip() if depends else None
        tag = TRACK_TAGS.get(name)
        declaration.tags = [tag] if tag else []
        # The backlog is a defect log; every other track declares gates with
        # exit conditions, which are milestones whether or not they nest.
        declaration.kind = "bug" if name == "00-backlog.md" else "milestone"
        declaration.keyless = declaration.ambiguous
        if declaration.ambiguous:
            declaration.tags.append("legacy")
        declarations.append(declaration)
    return declarations


def deferred_followups(text: str) -> list[Declaration]:
    """The backlog's ``## Deferred follow-ups`` bullets, as keyless items.

    These are real, unclosed work the roadmap never gave an id to: each is a
    half a resolved entry deliberately left open, tracked as prose under one
    shared heading. They have to become work items — otherwise the canonical
    store silently loses the only open work the backlog admits to — but there
    is no id to carry as a ``key``, and inventing one (``B61a``) would be the
    artificial naming the migration is meant to avoid. The owning item is
    recorded as a dependency instead, which is the relation the prose states.

    A bullet under a ``**Resolved``/``**Closed`` lead-in is history, not open
    work, so only bullets before the first such lead-in are read.
    """
    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith("## Deferred follow-ups"))
        end = next(i for i, line in enumerate(lines[start + 1 :], start + 1) if line.startswith("## "))
    except StopIteration:
        return []

    found: list[Declaration] = []
    for index in range(start + 1, end):
        line = lines[index]
        if line.startswith(("**Resolved", "**Closed")):
            break
        if not line.startswith("- "):
            continue
        # A bullet wraps until the next bullet, blank line, or lead-in.
        block = [line]
        for continuation in lines[index + 1 : end]:
            if not continuation.startswith("  ") or continuation.startswith("  - "):
                break
            block.append(continuation)
        prose = " ".join(" ".join(block).split()).removeprefix("- ")
        owner = ID_MENTION.match(prose)
        found.append(
            Declaration(
                identifier=f"deferred-followup:{index + 1}",
                source="00-backlog.md",
                line=index + 1,
                heading=line,
                level=3,
                body=prose,
                state="deferred",
                status_text=prose,
                depends_text=owner.group(1) if owner else None,
                tags=["backlog", "followup"],
                kind="followup",
                keyless=True,
            )
        )
    return found


def declarations() -> list[Declaration]:
    """Every work item the roadmap declares, in file order.

    That is the id-bearing headings plus the backlog's keyless deferred
    follow-ups, which are the only open work the backlog currently admits to.
    """
    found: list[Declaration] = []
    for path in sorted(ROADMAP.glob("*.md")):
        if path.name not in TRACK_TAGS:
            # `README.md` is a view: it summarises tracks and draws the
            # dependency diagram, and allocates no identity.
            continue
        text = path.read_text()
        found.extend(parse_track(path.name, text))
        if path.name == "00-backlog.md":
            found.extend(deferred_followups(text))
    return found


def parent_id(identifier: str, known: set[str]) -> str | None:
    """The nearest ancestor id, from the id's own structure.

    ``C8.13.3`` belongs to ``C8.13`` and ``M5.2a`` to ``M5``. Heading depth
    agrees with this everywhere in the current roadmap, but the id is the more
    precise signal: ``C8.13.3`` and ``C8.13`` are both level-3 headings.
    """
    head = identifier
    while "." in head:
        head = head.rsplit(".", 1)[0]
        if head in known:
            return head
    suffixed = re.fullmatch(r"([A-Z]+[0-9]+)[a-z]", identifier)
    if suffixed is not None and suffixed.group(1) in known:
        return suffixed.group(1)
    return None


def dependency_ids(declaration: Declaration, known: set[str]) -> list[str]:
    """The ids a ``**Depends on:**`` paragraph names, in order.

    Only the paragraph's first sentence is read. The rest is commentary, and
    it mentions ids that are explicitly *not* prerequisites — P3 depends on P5
    alone, then explains that "P3.D independently supplies the physical board
    loop that P3.E consumes", which as an edge would make P3 depend on its own
    children and close a cycle. Ranges such as ``B39–B49`` expand over the ids
    that exist, so a gap in the numbering does not invent one, and a colliding
    id is dropped rather than guessed at.
    """
    text = declaration.depends_text
    if text is None or re.match(r"^\s*None\.?\s*$", text, re.IGNORECASE):
        return []
    # A roadmap id's dots are always followed by another segment, so a period
    # before whitespace or end-of-text ends the sentence rather than an id.
    sentence = re.split(r"\.(?=\s|$)", text.strip(), maxsplit=1)[0]
    found: list[str] = []
    for prefix, low, high in ID_RANGE.findall(sentence):
        for number in range(int(low), int(high) + 1):
            candidate = f"{prefix}{number}"
            if candidate in known:
                found.append(candidate)
    found.extend(token for token in ID_MENTION.findall(sentence) if token in known)
    return [
        identifier
        for identifier in dict.fromkeys(found)
        if identifier != declaration.identifier and identifier not in COLLIDING_IDS
    ]


def diagram_dependencies(known: set[str]) -> list[tuple[str, str]]:
    """Hard dependency hops from the track diagram in ``roadmap/README.md``.

    The diagram is the roadmap's only *structured* dependency statement, and it
    reaches items whose prose declares no ``**Depends on:**`` line at all. It is
    read narrowly, because it is a drawing and encodes more than prerequisites:

    * Only solid ``-->`` hops. A dotted ``-.->`` hop is conditional or
      advisory — ``X1 -.->|only if chosen| RP6`` is a choice, not a
      prerequisite, and importing it as one would assert a decision nobody
      made.
    * Only hops between two real work items. Six endpoints (``Backlog``,
      ``Foundations``, ``Duo``, ``RV64``, ``H1V1``, ``Framework``) are prose
      aggregates with no item behind them.

    The caller keeps prose authoritative wherever prose speaks: an item whose
    ``**Depends on:**`` line names a *descendant* of a diagram predecessor is
    stating something strictly more precise, and the parent relation already
    connects the two.
    """
    text = (ROADMAP / "README.md").read_text()
    if "```mermaid" not in text:
        return []
    block = text.split("```mermaid", 1)[1].split("```", 1)[0]

    hops: list[tuple[str, str]] = []
    for line in block.splitlines():
        stripped = line.strip()
        # Node declarations carry a label in brackets; skip them so a label's
        # own text can never be read as an edge.
        if "[" in stripped or DIAGRAM_HOP.search(stripped) is None:
            continue
        pieces = DIAGRAM_HOP.split(stripped)
        # `re.split` with two groups yields node, arrow, label, node, ...
        nodes = [pieces[0].strip()]
        arrows: list[str] = []
        index = 1
        while index + 2 <= len(pieces) - 1:
            arrows.append(pieces[index])
            nodes.append(pieces[index + 2].strip())
            index += 3
        # `nodes` is one longer than `arrows` by construction — n hops join
        # n+1 nodes — so pairing deliberately stops at the last hop.
        for arrow, source, target in zip(arrows, nodes, nodes[1:], strict=False):
            if arrow != "-->":
                continue
            if source in known and target in known and source != target:
                hops.append((source, target))
    return list(dict.fromkeys(hops))
