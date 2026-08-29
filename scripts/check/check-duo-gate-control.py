#!/usr/bin/env python3

"""P3.D: prove `check-duo-boot.py`'s marker chain rejects tampered evidence.

`duo_boot_check` can only qualify a board if its assertions have teeth. This
gate takes the observed transcript committed beside the P3.D devlog entry,
confirms the unmodified bytes pass, then mutates them one way at a time and
requires each mutation to fail:

  * a required marker deleted,
  * a required marker reordered,
  * an explicit failure marker appended.

It reads a committed transcript rather than touching hardware, so it runs
anywhere and guards the checker itself rather than the board. The board gate is
`duo_boot_check`; this is its control.
"""

from __future__ import annotations

import importlib.util
import io
import re
from contextlib import redirect_stdout
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]
GATE = ROOT / "scripts" / "check" / "check-duo-boot.py"
TRANSCRIPT = (
    ROOT
    / "devlog"
    / "2026-08-29-p3d-milkv-duo-bringup"
    / "duo-boot-serial.log"
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"duo gate control: {message}")


def load_gate():
    if not GATE.is_file():
        fail(f"{GATE.relative_to(ROOT)} is missing")
    spec = importlib.util.spec_from_file_location("duo_boot_gate", GATE)
    if spec is None or spec.loader is None:
        fail("could not load the duo boot gate as a module")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    for attribute in ("check_transcript", "REQUIRED_MARKERS", "FAILURE_MARKERS"):
        if not hasattr(module, attribute):
            fail(f"the duo boot gate does not expose {attribute}")
    return module


def accepts(gate, transcript: str) -> bool:
    """True when the gate accepts this transcript. Its report output is muted."""
    try:
        with redirect_stdout(io.StringIO()):
            gate.check_transcript(transcript)
    except SystemExit:
        return False
    return True


def main() -> None:
    gate = load_gate()
    if not TRANSCRIPT.is_file():
        fail(
            f"{TRANSCRIPT.relative_to(ROOT)} is missing; this control needs the "
            "observed board transcript P3.D recorded"
        )
    observed = TRANSCRIPT.read_text()

    if not accepts(gate, observed):
        fail(
            "the committed board transcript does not satisfy the gate's own "
            "marker chain; the gate and its recorded evidence have diverged"
        )
    print("[control] the observed transcript passes, as it must")

    failures = 0

    # 1. Every required marker, deleted one at a time.
    for description, pattern in gate.REQUIRED_MARKERS:
        match = re.search(pattern, observed)
        if match is None:
            fail(
                f"the committed transcript does not contain {description!r}, so "
                "the deletion arm cannot be tested"
            )
        mutated = observed[: match.start()] + observed[match.end() :]
        if accepts(gate, mutated):
            print(f"  [ ] deleting {description!r} still passed")
            failures += 1
        else:
            print(f"  [X] deleting {description!r} fails the gate")

    # 2. Reordering: move the terminal marker ahead of the entry banner.
    entry_pattern = gate.REQUIRED_MARKERS[4][1]
    terminal_description, terminal_pattern = gate.REQUIRED_MARKERS[-1]
    entry = re.search(entry_pattern, observed)
    terminal = re.search(terminal_pattern, observed)
    if entry is None or terminal is None:
        fail("the committed transcript lacks the markers the reorder arm needs")
    without_terminal = observed[: terminal.start()] + observed[terminal.end() :]
    anchor = re.search(entry_pattern, without_terminal)
    if anchor is None:
        fail("removing the terminal marker also removed the reorder anchor")
    reordered = (
        without_terminal[: anchor.start()]
        + terminal.group(0)
        + "\n"
        + without_terminal[anchor.start() :]
    )
    if accepts(gate, reordered):
        print(f"  [ ] moving {terminal_description!r} before the entry still passed")
        failures += 1
    else:
        print(f"  [X] moving {terminal_description!r} before the entry fails the gate")

    # 3. Every failure marker, appended to an otherwise passing transcript.
    for pattern in gate.FAILURE_MARKERS:
        literal = re.sub(r"\\(.)", r"\1", pattern)
        if re.search(r"[\[\]\(\)\*\+\?\|]", literal):
            # A pattern this control cannot render as a literal line is exercised
            # by the gate's own regex, not here; skipping it silently would hide
            # that, so say so.
            print(f"  [-] failure marker {pattern!r} is not a plain literal; skipped")
            continue
        if accepts(gate, observed + "\n" + literal + "\n"):
            print(f"  [ ] appending {literal!r} still passed")
            failures += 1
        else:
            print(f"  [X] appending {literal!r} fails the gate")

    if failures:
        fail(f"{failures} tamper arm(s) were accepted; the gate lacks teeth")

    print(
        "duo gate control: every required marker is load-bearing, ordering is "
        "enforced, and every literal failure marker is fatal"
    )


if __name__ == "__main__":
    main()
