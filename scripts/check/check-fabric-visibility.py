#!/usr/bin/env python3

"""C8.8 filtered introspection and declared-interposition gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os

from harness import ROOT, run_qemu

REQUIRED = [
    "[fabric-publisher] graph view routes=2",
    "[fabric-subscriber] private view routes=1",
    "[fabric-intruder] ungranted view is byte-empty",
    "[fabric] filtered graph views complete",
    "[fabric] direct interposition bypass absent",
    "[fabric-intruder] proxy authority narrowed to chain",
    "[fabric-publisher] interposed sample published",
    "[fabric-subscriber] sample arrived through proxy",
    "[fabric-intruder] declared relay complete; exiting",
    "[fabric] declared proxy relayed telemetry",
    "[fabric-subscriber] proxy loss route event observed",
    "[fabric-subscriber] proxy loss visible in graph view",
    "[fabric] proxy death isolated to telemetry",
    "[fabric-publisher-b] unrelated diagnostics published",
    "[fabric-subscriber-b] unrelated diagnostics live after proxy death",
    "[fabric] unrelated diagnostics route live after proxy death",
    "[fabric] visibility plane complete",
    "[init] fabric visibility complete",
    "[generation] vertical slice healthy",
]
FORBIDDEN = [
    "[fabric] fail:",
    "[fabric-publisher] fail:",
    "[fabric-subscriber] fail:",
    "[fabric-publisher-b] fail:",
    "[fabric-subscriber-b] fail:",
    "[fabric-intruder] fail:",
]


def run_profile(*, early_proxy_death: bool = False) -> str:
    environment = os.environ.copy()
    # B11: this gate exercises verification scaffolding, so it selects the
    # boot profile that declares it. The product profile declares none.
    environment["SLIME_GENERATION_NUMBER"] = "16"
    environment["SLIME_FABRIC_PROFILE"] = "visibility"
    environment["SLIME_FABRIC_VISIBILITY_CHECK"] = "1"
    if early_proxy_death:
        environment["SLIME_FABRIC_PROXY_EARLY_EXIT"] = "1"
    else:
        environment.pop("SLIME_FABRIC_PROXY_EARLY_EXIT", None)
    return run_qemu(
        ["cargo", "run", "--release", "--", "-display", "none"],
        environment=environment,
        cwd=ROOT / "kernel",
        timeout=120,
        echo="on-error",
    )


def records(output: str, prefix: str) -> list[str]:
    return [line for line in output.splitlines() if line.startswith(prefix)]


def check_source_authority() -> None:
    proxy = (ROOT / "components" / "bins" / "src" / "bin" / "fabric-intruder.rs").read_text(
        encoding="utf-8"
    )
    service = (ROOT / "components" / "bins" / "src" / "visibility_broker.rs").read_text(
        encoding="utf-8"
    )
    for forbidden in ("yield_now", "fabric_call", "fabric_operation", "RIGHT_TRANSFER"):
        if forbidden in proxy:
            raise SystemExit(f"proxy contains undeclared authority surface: {forbidden}")
    if "FABRIC_INTERPOSITIONS" not in service or "direct interposition bypass absent" not in service:
        raise SystemExit("visibility broker is not compiled from the declared chain")


def main() -> None:
    check_source_authority()
    first = run_profile()
    second = run_profile()
    early = run_profile(early_proxy_death=True)
    for output in (first, second):
        for marker in FORBIDDEN:
            if marker in output:
                print(output, end="")
                raise SystemExit(f"fabric visibility reported failure: {marker}")
        for marker in REQUIRED:
            if marker not in output:
                print(output, end="")
                raise SystemExit(f"fabric visibility missing marker: {marker}")

    early_required = [
        "[fabric-intruder] injected early proxy death",
        "[fabric-subscriber] proxy loss route event observed",
        "[fabric-subscriber] proxy loss visible in graph view",
        "[fabric] proxy death isolated to telemetry",
        "[fabric-subscriber-b] unrelated diagnostics live after proxy death",
        "[fabric] unrelated diagnostics route live after proxy death",
        "[fabric] visibility plane complete",
        "[init] fabric visibility complete",
        "[generation] vertical slice healthy",
    ]
    for marker in FORBIDDEN:
        if marker in early:
            print(early, end="")
            raise SystemExit(f"early proxy death reported failure: {marker}")
    for marker in early_required:
        if marker not in early:
            print(early, end="")
            raise SystemExit(f"early proxy death missing marker: {marker}")
    if "[fabric] declared proxy relayed telemetry" in early:
        raise SystemExit("early proxy death unexpectedly completed the relay")

    first_views = records(first, "[fabric-view] ")
    second_views = records(second, "[fabric-view] ")
    first_traces = records(first, "[fabric-trace] ")
    second_traces = records(second, "[fabric-trace] ")
    if len(first_views) != 12 or first_views != second_views:
        raise SystemExit("introspection records are not byte-identical in fixed request order")
    if len(first_traces) != 2 or first_traces != second_traces:
        raise SystemExit("interposition traces are not byte-identical across identical runs")
    if len({*first_traces}) != 2:
        raise SystemExit("relay and proxy-loss traces collapsed into one event")
    early_traces = records(early, "[fabric-trace] ")
    if len(early_traces) != 1:
        raise SystemExit("early proxy death did not emit exactly one route-loss trace")

    for marker in REQUIRED:
        print(marker)
    print("fabric visibility check: ok")


if __name__ == "__main__":
    main()
