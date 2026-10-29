"""Judge what the Zenoh service client adapter does when an attach fails after both loans arrived.

The adapter (`components/lib/src/zenoh_service_client.rs`) holds a session at the service once the service has queued
both loans and its attach answer. An attach that then fails must end that session, or the service keeps it and a
retry sees stale control state. The Rust test `the_abort_exam` plays the service with a scripted stand-in runtime and
prints one `[zenoh-abort] failed-attach ...` line for each failure. This module owns what those lines must be and
nothing here reads the Rust:

* which control requests the adapter sent, by operation, in order;
* how many loans were back when each request went out, so the loans return before the abort;
* how many scripted answers were unread when each request went out. The service sends its attach answer with a send
  that blocks until it is received, so the adapter reads that answer before it sends the abort;
* the loans returned and the answers left unread when the call ended;
* the bytes of the abort request, which `struct` writes again from the schema's layout.

Where the service may not hold a session, the adapter sends no abort. That is a refused attach, and an attach whose
answer never came, since the service may be blocked sending it.

Every field of every line is changed in turn by the controls, so a judge that stopped comparing one field fails here.
"""

from __future__ import annotations

import re

import zenoh_client_io_exam as io_exam
from harness import ROOT  # noqa: F401  (kept for the path the other judges share)

MARKER = "failed-attach"
# (name, error, sent, returns-at-send, unread-at-send, returns, slots, imports, provisioning loans unread, unread,
#  whether an abort request was sent)
# `slots` are the capability slots of the loans returned, sorted. The stand-in gives the ring slot 1 and the data slot 2,
# so a loan returned twice and another leaked prints `1,1`, which a count of two returns would not show.
# `imports` is how many loans the adapter imported. Every import is returned once, so it equals the returns, and a
# refused attach imports none. The line carries every counter the stand-in keeps, so none can be left off it.
# The scripted answers are the attach answer and then the abort's acknowledgement, so two are unread when the attach
# goes out. Reading the attach answer first leaves one unread, the acknowledgement, when the abort goes out.
CASES = (
    ("unformatted-ring", "Malformed", "attach,abort", "0,2", "2,1", 2, "1,2", 2, 0, 0, True),
    ("one-buffer-for-both", "Malformed", "attach,abort", "0,2", "2,1", 2, "1,2", 2, 0, 0, True),
    ("no-rows", "Malformed", "attach,abort", "0,2", "2,1", 2, "1,2", 2, 0, 0, True),
    ("too-many-rows", "Malformed", "attach,abort", "0,2", "2,1", 2, "1,2", 2, 0, 0, True),
    # The abort is not acknowledged: the attach error is still returned, and the loans are still back first.
    ("abort-not-acknowledged", "Malformed", "attach,abort", "0,2", "1,0", 2, "1,2", 2, 0, 0, True),
    # The acknowledgement is for another operation: it is read and ignored, and the attach error is returned.
    ("abort-wrongly-acknowledged", "Malformed", "attach,abort", "0,2", "2,1", 2, "1,2", 2, 0, 0, True),
    # No attach answer: the service may be blocked sending it, so no abort is sent. Both loans are returned.
    ("no-attach-answer", "Lost", "attach", "0", "0", 2, "1,2", 2, 0, 0, False),
    # The service refused the attach, so it holds no session. No loan was imported.
    ("attach-refused-by-the-service", "Setup", "attach", "0", "1", 0, "none", 0, 0, 0, False),
)
OK_LINE = "ok a-failed-attach-ends-the-service-session"


class AbortError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise AbortError(message)


def good_lines(text: str | None = None) -> list[str]:
    """The honest transcript, in the order the Rust test prints it, for the layouts in `text`."""
    abort = io_exam.request_bytes(io_exam.OP["abort"], text=text).hex()
    out = []
    for name, error, sent, returns_at_send, unread_at_send, returns, slots, imports, provision_unread, unread, aborted in CASES:
        request = abort if aborted else "none"
        out.append(
            f"{MARKER} {name} {error} sent {sent} returns-at-send {returns_at_send} unread-at-send {unread_at_send} "
            f"returns {returns} slots {slots} imports {imports} provision-unread {provision_unread} unread {unread} abort-request {request}"
        )
    out.append(OK_LINE)
    return out


def _shapes() -> set[int]:
    return {len(line.split()) for line in good_lines()}


def judge(lines: list[str], text: str | None = None) -> tuple[int, int]:
    """Check a transcript; returns (cases, refusals). Every `failed-attach` line names an error the adapter returned,
    so each counts as a refusal."""
    wanted = good_lines(text)
    for line in lines:
        _need(line.split()[:1] in ([MARKER], ["ok"]), f"abort: a record that is not an abort exam line: {line!r}")
        _need(len(line.split()) in _shapes(), f"abort: a line with {len(line.split())} fields: {line}")
    for index, (seen, want) in enumerate(zip(lines, wanted, strict=False)):
        _need(seen == want, f"abort line {index + 1} is {seen!r}, the exam requires {want!r}")
    if len(lines) < len(wanted):
        raise AbortError(f"abort: the transcript stops after {len(lines)} of {len(wanted)} lines; the next is {wanted[len(lines)]!r}")
    if len(lines) > len(wanted):
        raise AbortError(f"abort: {len(lines) - len(wanted)} lines beyond the exam, the first {lines[len(wanted)]!r}")
    refusals = sum(1 for line in lines if line.startswith(MARKER + " "))
    return len(lines), refusals


def _change(token: str) -> str:
    if re.fullmatch(r"[0-9a-f]{16,}", token):
        return token[:-1] + ("0" if token[-1] != "0" else "1")
    return token + "x"


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not an abort exam line", [*good, "clientio ok every-request-is-bounded"]),
        ("a line with an extra field", [*good[:-1], good[-1] + " extra"]),
        ("a line repeated", [*good, good[-1]]),
        ("the last two lines swapped", [*good[:-2], good[-1], good[-2]]),
        ("the first two lines swapped", [good[1], good[0], *good[2:]]),
    ]
    for index, line in enumerate(good):
        out.append((f"line {index + 1} ({line[:40]}) missing", good[:index] + good[index + 1 :]))
    # Every field of every line, changed alone.
    for index, line in enumerate(good):
        fields = line.split()
        for position, token in enumerate(fields):
            changed = [*fields[:position], _change(token), *fields[position + 1 :]]
            out.append((f"line {index + 1} field {position + 1} ({token[:24]}) changed", [*good[:index], " ".join(changed), *good[index + 1 :]]))
    # The failures a wrong adapter would print, by name: (line index, label, the line it would print instead).
    abort = good[0].rsplit(" ", 1)[1]
    named = [
        (0, "the abort sent before the attach answer was read", good[0].replace("unread-at-send 2,1", "unread-at-send 2,2")),
        (0, "the loans returned after the abort", good[0].replace("returns-at-send 0,2", "returns-at-send 0,0")),
        (0, "no abort sent after a failed setup", good[0].replace("sent attach,abort", "sent attach").replace("returns-at-send 0,2", "returns-at-send 0").replace("unread-at-send 2,1", "unread-at-send 2").replace("abort-request " + abort, "abort-request none")),
        (0, "a loan left unreturned", good[0].replace("returns 2 slots 1,2", "returns 1 slots 1")),
        (0, "one loan returned twice and the other leaked", good[0].replace("slots 1,2", "slots 1,1")),
        (7, "a slot imported on a refused attach and never returned", good[7].replace("imports 0", "imports 1")),
        (0, "a loan imported and never returned", good[0].replace("imports 2", "imports 3")),
        (0, "a provisioning loan left unread", good[0].replace("provision-unread 0", "provision-unread 1")),
        (0, "the abort acknowledgement left unread", good[0].replace("provision-unread 0 unread 0", "provision-unread 0 unread 1")),
        (6, "an abort sent when no attach answer came", good[6].replace("sent attach ", "sent attach,abort ").replace("returns-at-send 0 ", "returns-at-send 0,2 ").replace("unread-at-send 0 ", "unread-at-send 0,0 ").replace("abort-request none", "abort-request " + abort)),
        (7, "an abort sent after the service refused the attach", good[7].replace("sent attach ", "sent attach,abort ").replace("abort-request none", "abort-request " + abort)),
    ]
    for index, label, mutated in named:
        out.append((label, [*good[:index], mutated, *good[index + 1 :]]))
    return out


def transcript_controls() -> int:
    good = good_lines()
    judge(good)
    refused = 0
    seen: dict[tuple[str, ...], str] = {}
    for label, lines in transcript_mutations():
        if lines == good:
            raise AbortError(f"control: {label} did not change the transcript")
        # Two controls that corrupt the transcript the same way are one control, and the pin counts distinct ones.
        if tuple(lines) in seen:
            raise AbortError(f"control: {label} repeats {seen[tuple(lines)]}")
        seen[tuple(lines)] = label
        try:
            judge(lines)
        except AbortError:
            refused += 1
            continue
        raise AbortError(f"control: the abort judge accepted {label}")
    return refused


def reference_controls() -> int:
    """The abort request's bytes come from the schema's layout. An abort carries sequence 0 and row 0, so only edits to the fields before the row change its bytes. For each corrupted layout, the transcript of an
    adapter of the real layout is refused, because the judge then requires the bytes of the corrupted one."""
    text = io_exam.client.SCHEMA.read_text(encoding="utf-8")
    honest = good_lines()
    refused = 0
    edits = [
        ("the request op widened", '{ name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "row"', '{ name = "op"; width = 2; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "row"'),
        ("the request op and flags swapped", '{ name = "op"; width = 1; signed = false; byteArray = false; };\n  { name = "flags"; width = 1; signed = false; byteArray = false; };', '{ name = "flags"; width = 1; signed = false; byteArray = false; };\n  { name = "op"; width = 1; signed = false; byteArray = false; };'),
        ("the request version widened", '{ name = "version"; width = 2; signed = false; byteArray = false; };\n  { name = "op"', '{ name = "version"; width = 4; signed = false; byteArray = false; };\n  { name = "op"'),
    ]
    for label, old, new in edits:
        block = re.search(r"^requestLayout :: List WireField = \{\n(.*?)^\};", text, re.M | re.S)
        if block is None or block.group(0).count(old) != 1:
            raise AbortError(f"control: {label} anchor is not in requestLayout exactly once")
        mutated = text.replace(block.group(0), block.group(0).replace(old, new, 1))
        try:
            judge(honest, mutated)
        except (AbortError, io_exam.ClientIoError, io_exam.wire_exam.WireError, KeyError):
            refused += 1
            continue
        raise AbortError(f"control: the abort judge accepted the honest transcript under a schema with {label}")
    return refused


def all_controls() -> int:
    return transcript_controls() + reference_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed and retired
# to .tasks/terminal. Both counts are exact.
PINNED_CASES = 9
PINNED_CONTROLS = 199
ADMISSION = "slime-zenoh-service-client-abort-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_service_client_abort_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
