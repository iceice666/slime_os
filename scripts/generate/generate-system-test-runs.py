#!/usr/bin/env python3

"""Generate `contracts/system-test-run/v1` records from each plane gate's execution inputs.

CP11 declared that QEMU arguments, disk fixtures, injected device behavior,
timeouts, and marker contracts belong to a versioned test-run contract rather
than to the image closure, so a marker oracle or a timeout cannot alter the
image it exercises. CP14's fifth deliverable is moving the real values there.

The records are *frozen declarations*, extracted once and then held fixed —
the same convention `sel4_boot_layout_check` uses for its per-plane layout
fixtures, with `--check` refusing drift and `--bless` re-freezing after an
intended change. That direction matters: after the first freeze the record is
the authority and a checker that quietly grows a disk or doubles a timeout is a
refusal, not a silent update.

What this does *not* do is make the checkers read these records. That is CP15's
migration, and claiming it here would be false: the values below are still
executed from each checker's own constants. What the freeze buys today is that
an execution input cannot change without a reviewed record change.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import argparse
import re

from harness import ROOT
from system_image_closure import compile_closure

CHECK_ROOT = ROOT / "scripts" / "check"
RUN_ROOT = ROOT / "contracts" / "system-test-run" / "v1" / "runs"
CLOSURE_ROOT = ROOT / "contracts" / "system-image-closure" / "v1" / "closures"

# The marker contract each plane's expectations are stated against. One
# identity for the seL4 serial marker vocabulary: the planes assert different
# chains *within* it, and those chains stay in their owning checker.
MARKER_CONTRACT = "f03ce9b40628dcb82e3ec97154b2f5ec549a41bfbb9cfcf13a632e40113fd42e"

# Checkers that boot no seL4 QEMU plane of their own, so they own no test run:
# board gates, host-only contract gates, aggregate composers that delegate to
# the planes they compose, and the bless/derivation helpers.
NOT_A_PLANE = {
    "check-sel4-boot-layout.py",  # composes every plane; owns no single run
    "check-sel4-fabric-aggregate.py",  # composes the fault and traffic planes
    "check-sel4-capability-layout.py",  # audits the boot plane's root, no run of its own
    "check-sel4-gate-control.py",  # mutates other gates' transcripts
    "check-sel4-plane.py",  # shared runner
    "check-sel4-root-boot.py",  # root-only aggregate over the boot plane
    "check-sel4-gate-controls.py",  # asserts other gates reject bad input; boots nothing
    "check-sel4-pins.py",  # host-side pin assertion; boots nothing
    "check-sel4-trace-plane.py",  # analyses a transcript; boots nothing
}


def fail(message: str) -> None:
    raise SystemExit(f"system test run generation: {message}")


def plane_checkers() -> list[_Path]:
    return [
        path
        for path in sorted(CHECK_ROOT.glob("check-sel4-*.py"))
        if path.name not in NOT_A_PLANE
    ]


def run_name(path: _Path) -> str:
    """`check-sel4-io-block-plane.py` -> `sel4-io-block`."""
    stem = path.stem[len("check-") :]
    if stem.endswith("-plane"):
        stem = stem[: -len("-plane")]
    return stem


def booted_image(path: _Path) -> str:
    """The `build/slime-*.elf` this checker boots.

    Derived from the checker rather than from the run name, because the two
    differ: `check-sel4-component-graph.py` boots `slime-sel4-graph.elf`. Using
    the name would attribute a plane to the wrong image, which is exactly the
    mapping error the aggregate gate exists to catch.
    """
    text = path.read_text(encoding="utf-8")
    found = sorted(
        set(re.findall(r'"build"\s*/\s*"(slime-[a-z0-9.-]+\.elf)"', text))
        | set(re.findall(r"build/(slime-[a-z0-9.-]+\.elf)", text))
    )
    if not found:
        fail(f"{path.name}: boots no build/slime-*.elf image")
    # A checker touching several images (a plane plus a control arm) is keyed by
    # the one its own name implies, falling back to the single image it names.
    stem = run_name(path)
    preferred = f"slime-{stem}.elf"
    if preferred in found:
        return preferred
    return found[0]


def closure_name_for(path: _Path, name: str) -> str:
    """The closure this checker's own source declares, or its run name.

    Most checkers' closure matches their run name (`sel4-channel` builds the
    `sel4-channel` closure). A checker whose plane image predates the closure
    naming convention — `check-sel4-component-graph.py` and
    `check-sel4-device-plane.py` both build the `sel4` closure — declares a
    `CLOSURE = "..."` constant that names it explicitly; reading that instead
    of guessing from the run name is what keeps this generic rather than a
    second hand-maintained mapping beside the aggregate gate's.
    """
    match = re.search(r'^CLOSURE = "([a-z0-9-]+)"', path.read_text(encoding="utf-8"), re.MULTILINE)
    return match.group(1) if match is not None else name


def closure_identity_for(name: str) -> str:
    """The closure this run exercises, or the empty string when none exists.

    An empty identity is a truthful statement that the plane's image is not yet
    closure-reachable — the eight images CP15 still owns — rather than a
    fabricated digest. `check-system-test-run.py` requires a non-empty identity
    to resolve and an empty one to name an image the aggregate gate agrees is
    exempt.
    """
    candidate = CLOSURE_ROOT / f"{name}.zti"
    if not candidate.is_file():
        return ""
    return compile_closure(candidate).identity.hex()


def extract(path: _Path) -> dict:
    """Read one checker's execution-only inputs from its own constants."""
    text = path.read_text(encoding="utf-8")

    timeouts = re.findall(
        r"^(?:BOOT_TIMEOUT_SECONDS|TIMEOUT|BOOT_TIMEOUT|SESSION_TIMEOUT_SECONDS)\s*=\s*(\d+)",
        text,
        re.MULTILINE,
    )
    # Some planes state their bound as an argument default rather than a module
    # constant. That is the same declaration — the value a plain `just` run
    # executes with — so it counts.
    timeouts += re.findall(r'"--timeout",\s*type=int,\s*default=(\d+)', text)
    if not timeouts:
        fail(f"{path.name}: declares no boot timeout, so its run would be unbounded")
    # The largest declared bound: a plane with a session or recovery timeout
    # beyond its boot timeout may legitimately run that long.
    timeout = max(int(value) for value in timeouts)

    # One `-drive` argument is one attached disk. Counting distinct occurrences
    # rather than parsing QEMU argv keeps this honest about what the checker
    # actually attaches; a checker that attaches two disks in two code paths
    # declares two.
    drive_sites = len(re.findall(r'"-drive"', text))

    devices = sorted(
        set(re.findall(r'"(virtio-[a-z]+-device)[^"]*"', text))
        | set(re.findall(r'"(usb-kbd)"', text))
    )

    # A forbidden outcome is written either as a bare literal or as a regex with
    # a trailing matcher (`r"SLIME_ROOT FATAL .*"`). Matching only the literal
    # form silently reported "forbids nothing" for every plane using the regex
    # form, which is the more common one.
    forbidden = sorted(
        {
            marker
            for marker in re.findall(
                r'r?"(SLIME_ROOT FATAL|SLIME_GRAPH FAIL|SLIME_ROOT PANIC)(?: \.\*)?"', text
            )
        }
    )

    # Runtime fault injection, by the specific mechanism each kind uses. These
    # are deliberately narrow: a generic `inject` would match ordinary helper
    # names and declare a fault the plane does not exercise, and an over-broad
    # declaration is worse than none because it is what a reader trusts.
    faults = []
    for kind, pattern in (
        # The checker rewrites persisted bytes before or between boots.
        ("corruption", r"\bcorrupt(?:ed|ion|_[a-z]+)?\b|CORRUPT"),
        # A participant is compiled or driven to die mid-exchange.
        ("peer-death", r"EARLY_EXIT|peer[_ -]death|PEER_DEATH"),
        # A device is made to fail or to report a fault, rather than merely
        # being attached.
        ("device", r"device[_ -]fault|DEVICE_FAULT|fault[_ -]inject"),
    ):
        if re.search(pattern, text):
            faults.append(kind)

    return {
        "timeoutSeconds": timeout,
        "drives": drive_sites,
        "devices": devices,
        "forbiddenOutcomes": forbidden,
        "faults": faults,
    }


def render(name: str, closure: str, facts: dict) -> str:
    """Render one record. Zutai's JSON projection sorts keys, so field order is fixed."""

    def text_list(values: list[str], indent: str = "    ") -> str:
        if not values:
            return "[]"
        rows = "".join(f'{indent}  "{value}";\n' for value in values)
        return "[\n" + rows + indent + "]"

    disks = ""
    if facts["drives"]:
        rows = []
        for index in range(facts["drives"]):
            rows.append(
                "    {\n"
                f'      name = "disk-{index}";\n'
                f'      path = "";\n'
                f'      identity = "";\n'
                "      writable = true;\n"
                "    };"
            )
        disks = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        disks = "[]"

    devices = ""
    if facts["devices"]:
        rows = []
        for device in facts["devices"]:
            rows.append(
                "    {\n"
                f'      name = "{device}";\n'
                f'      path = "";\n'
                f'      identity = "";\n'
                "      writable = false;\n"
                "    };"
            )
        devices = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        devices = "[]"

    faults = ""
    if facts["faults"]:
        rows = []
        for kind in facts["faults"]:
            rows.append(
                "    {\n"
                f'      kind = "{kind}";\n'
                f'      target = "";\n'
                f'      value = "";\n'
                "    };"
            )
        faults = "[\n" + "\n".join(rows) + "\n  ]"
    else:
        faults = "[]"

    return (
        "{\n"
        "  formatVersion = 1;\n"
        f'  name = "{name}";\n'
        f'  imageClosureIdentity = "{closure}";\n'
        '  executionKind = "emulator";\n'
        '  executionProfile = "qemu-arm-virt";\n'
        f"  disks = {disks};\n"
        "  networks = [];\n"
        f"  devices = {devices};\n"
        f"  faultControls = {faults};\n"
        f'  timeoutSeconds = {facts["timeoutSeconds"]};\n'
        f'  markerContractIdentity = "{MARKER_CONTRACT}";\n'
        f'  forbiddenOutcomes = {text_list(facts["forbiddenOutcomes"], "  ")};\n'
        "}\n"
    )


def outputs() -> dict[_Path, str]:
    emitted: dict[_Path, str] = {}
    for path in plane_checkers():
        name = run_name(path)
        emitted[RUN_ROOT / f"{name}.zti"] = render(
            name, closure_identity_for(closure_name_for(path, name)), extract(path)
        )
    return emitted


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="refuse any record that drifted from its checker"
    )
    parser.add_argument(
        "--bless", action="store_true", help="re-freeze records after an intended change"
    )
    arguments = parser.parse_args()

    RUN_ROOT.mkdir(parents=True, exist_ok=True)
    emitted = outputs()

    if arguments.check:
        stale = [
            path.name
            for path, content in emitted.items()
            if not path.is_file() or path.read_text(encoding="utf-8") != content
        ]
        if stale:
            fail(
                f"record(s) disagreeing with their checker: {sorted(stale)}; "
                "re-freeze with `just system_test_run_bless` if the change is intended"
            )
        print(f"{len(emitted)} system test-run records are current")
        return 0

    if not arguments.bless:
        fail("pass --check to verify or --bless to re-freeze")

    for path, content in sorted(emitted.items()):
        path.write_text(content, encoding="utf-8")
        print(f"Blessed {path.relative_to(ROOT)}")
    print(f"{len(emitted)} system test-run records frozen")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
