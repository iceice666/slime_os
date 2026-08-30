#!/usr/bin/env python3

"""Validate generation-manifest slot-pin reasons and automatic slot expectations.

Every pinned instance binding must declare the strongest reason supported by
the manifest and all of its boot profiles. A `componentAbi` reason is accepted
only when the holder's source does not demonstrate that the binding is resolved
by name without compiling the pinned position.

Source analysis is deliberately one-sided: it rejects unsupported positional
claims but does not prove every remaining `componentAbi` pin necessary. Slot
permutations among positional consumers remain the responsibility of the owning
QEMU plane.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[2] / "lib"))

import json
import os
import re
import subprocess
import sys
from pathlib import Path

from component_paths import crate_paths
from component_spec import admit_specs
from harness import GENERATION_COMPOSITIONS, GENERATION_FIXTURES, ROOT, load_script
from zutai_cli import STDLIB, binary

BUILDER = load_script("slime_build_generation_slot_reasons", "build/build-generation.py")

# The reference fixture plus every plane and product composition. The fixture is
# included because it is a real manifest the builder encodes, not only a
# schema-conformance input, and its pins are subject to the same rules.
CORPUS = [GENERATION_FIXTURES / "valid.zti", *sorted(GENERATION_COMPOSITIONS.glob("*.zti"))]

# These unpinned bindings must keep the allocator-produced slots observed by
# their consumers. Re-pinning one would restore unnecessary positional state;
# changing its automatic slot requires re-verifying the owning plane.
AUTOMATIC_SLOT_EXPECTATIONS = {
    ("valid.zti", "spawn-service", "spawn-service-echo"): 1,
    ("valid.zti", "spawn-service", "spawn-service-sysinfo"): 2,
    ("sel4-io-network.zti", "io-link-loopback", "network-service-link-device"): 0,
    ("sel4-io-network.zti", "network-service", "network-intruder-service"): 1,
    ("sel4-io-network.zti", "network-service", "network-service-link-device"): 2,
}

# `resolve_binding(b"<name>")` with a literal name: the holder asking the root
# for a slot by its stable generation name, which is the mechanism that makes a
# pin removable.
RESOLVE_BY_NAME = re.compile(rb'resolve_binding\(\s*&?\s*b"([^"]*)"')

# Most components do not call `resolve_binding` directly. Sixteen crates define a
# one-argument wrapper — `fn binding(name: &[u8]) -> u32 { resolve_binding(name)… }`,
# also spelled `route_slot`, `crossing_factory`, `resolve_executable` — and pass
# the literal to that. A name test blind to the wrapper reads those grants as
# positional and misses exactly the pins this clause exists to find: two
# `network-service` pins in `sel4-io-network.zti` are resolved through such a
# wrapper and compile no slot number at all.
#
# `WRAPPER_DEFINITION` finds a function whose body forwards its single `&[u8]`
# parameter to `resolve_binding`; `wrapper_calls` then collects the byte-string
# literals passed to it. Matching the *parameter name* in the body is what keeps
# this narrow — a function that resolves some other name is not a forwarder.
WRAPPER_DEFINITION = re.compile(
    rb"fn\s+(\w+)\s*\(\s*(\w+)\s*:\s*&\[u8\]\s*\)[^{]*\{(?P<body>[^}]*)\}",
    re.DOTALL,
)

# A slot number compiled into the holder: either a `*_SLOT` constant or an
# integer handed directly to a slot-taking syscall. Presence of the pinned number
# in either form is enough to treat the pin as positionally consumed.
SLOT_CONSTANT = re.compile(rb"const\s+\w*SLOT\w*\s*:\s*u32\s*=\s*(\d+)\s*;")
SLOT_LITERAL_CALL = re.compile(
    rb"\b(?:send|send_on|recv|recv_blocking|reply|notification_wait|notification_poll"
    rb"|notification_signal|shared_buffer_create|shared_buffer_loan|directory_inspect"
    rb"|io_mmio_map|io_dma_map|io_irq_ack|io_device_bind|io_queue_map|spawn)\s*\(\s*(\d+)\b"
)

# `#[path = "..."] mod ...`: several components pull a shared scenario module out
# of `components/lib/src` this way rather than through the library crate, so a
# scan that only walks `<crate>/src` misses where they resolve their bindings.
PATH_MOD = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')

# Names built at runtime from a fixed affix plus a component or command name --
# `<component>-control`, `spawn-service-<command>`. The holder resolves these by
# name too, so a pin whose grant matches one is name-resolved even though the
# full string appears nowhere in the source.
AFFIX = re.compile(rb'b"(-[a-z0-9-]+|[a-z0-9-]+-)"')


def wrapper_resolved_names(blob: bytes) -> set[bytes]:
    """Names reaching `resolve_binding` through a single-argument wrapper.

    Two passes: find every function that forwards its own `&[u8]` parameter to
    `resolve_binding`, then collect the byte-string literals passed to those
    functions by name. Without this the clause sees only direct calls, and the
    sixteen crates that wrap read as fully positional.
    """
    wrappers = {
        match.group(1)
        for match in WRAPPER_DEFINITION.finditer(blob)
        if b"resolve_binding" in match.group("body")
        and re.search(
            rb"resolve_binding\(\s*&?\s*" + re.escape(match.group(2)) + rb"\b", match.group("body")
        )
    }
    names: set[bytes] = set()
    for wrapper in wrappers:
        names |= set(re.findall(rb"\b" + re.escape(wrapper) + rb'\(\s*&?\s*b"([^"]*)"', blob))
    return names


def fail(message: str) -> None:
    raise SystemExit(f"slot pin reasons: {message}")


def decode(path: Path) -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    output = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    return json.loads(output)


def crate_sources(crate: Path) -> list[Path]:
    """Every `.rs` file this crate compiles, following `#[path]` module includes."""
    seen: set[Path] = set()
    frontier = list((crate / "src").rglob("*.rs"))
    while frontier:
        candidate = frontier.pop()
        resolved = candidate.resolve()
        if resolved in seen or not resolved.is_file():
            continue
        seen.add(resolved)
        text = resolved.read_text(encoding="utf-8", errors="replace")
        frontier.extend(resolved.parent / relative for relative in PATH_MOD.findall(text))
    return sorted(seen)


# Freestanding C components, whose sources are not a Cargo crate. `slisp` is a
# real product component with real positional slots (`components/slisp/main.c`
# defines `INPUT_SLOT`/`SPAWN_SERVICE_SLOT`), so leaving it out of the evidence
# map would exempt it rather than check it.
C_COMPONENT_SOURCES = {"slisp": ROOT / "components" / "slisp"}

# `#define <NAME>_SLOT <n>` — the C spelling of the `*_SLOT` constant the Rust
# components declare, and the same claim: this number is compiled in.
C_SLOT_DEFINE = re.compile(rb"#define\s+\w*SLOT\w*\s+(\d+)")

# Manifest executables with no source this gate can read at all. `dango` is a
# retired identity kept only for the frozen CP1 baseline: its component spec
# declares `provider = "undeclared"` with no binary, so there is nothing to
# scan. Listing it here rather than defaulting to empty evidence keeps the
# exemption explicit, and `main` still refuses any executable that is neither
# scannable nor listed.
SOURCELESS_EXECUTABLES = {"dango"}


def component_evidence() -> dict[str, tuple[set[bytes], set[bytes], set[int]]]:
    """Per manifest executable: names resolved, affixes composed, slots compiled.

    A manifest's `executable` may be either a component-spec identity or the
    binary implementing it — `sel4-filesystem-service` appears under both names
    across the corpus — so each crate is registered under both. Resolving through
    the specs' own `implementation.binary` and `component_paths.crate_paths()`
    rather than by directory name is what makes that possible: spec identity,
    binary, and directory are three namespaces that need not agree
    (`generation-manager` is implemented by `sel4-generation-manager`), and
    guessing from directories returns empty evidence for exactly the components
    where it looks like it worked.

    Freestanding C components are scanned too, from `C_COMPONENT_SOURCES`. An
    executable with no readable source at all is deliberately absent, and `main`
    then refuses to audit a `componentAbi` claim about it unless it is listed in
    `SOURCELESS_EXECUTABLES`, rather than silently accepting one.
    """
    crates = crate_paths()
    evidence: dict[str, tuple[set[bytes], set[bytes], set[int]]] = {}
    for binary_name, crate in crates.items():
        blob = b"\n".join(path.read_bytes() for path in crate_sources(crate))
        names = set(RESOLVE_BY_NAME.findall(blob)) | wrapper_resolved_names(blob)
        affixes = set(AFFIX.findall(blob)) if b"resolve_binding" in blob else set()
        numbers = {int(value) for value in SLOT_CONSTANT.findall(blob)}
        numbers |= {int(value) for value in SLOT_LITERAL_CALL.findall(blob)}
        evidence[binary_name] = (names, affixes, numbers)
    # Spec identities that differ from the binary implementing them, so a
    # manifest naming either resolves. The spec corpus is smaller than the crate
    # population, which is why the crates are enumerated first and the specs only
    # add aliases.
    for spec in admit_specs():
        entry = evidence.get(spec.spec["implementation"]["binary"])
        if entry is not None:
            evidence.setdefault(spec.name, entry)
    for name, directory in C_COMPONENT_SOURCES.items():
        blob = b"\n".join(
            path.read_bytes()
            for path in sorted(directory.glob("*.c"))
            if path.name != "host_main.c"
        )
        numbers = {int(value) for value in C_SLOT_DEFINE.findall(blob)}
        # A C component reaches no `resolve_binding`, so it resolves nothing by
        # name: every slot it uses is a compiled position.
        evidence[name] = (set(), set(), numbers)
    return evidence


def resolves_by_name(evidence: dict, executable: str, grant: str) -> bool:
    """Whether `executable` asks the root for `grant`'s slot by name.

    Either the literal grant name appears in a `resolve_binding` call, or the
    grant matches an affix the holder composes names from at runtime —
    `<component>-control`, `spawn-service-<command>` — where the full string
    exists in no source file.
    """
    names, affixes, _ = evidence[executable]
    encoded = grant.encode()
    if encoded in names:
        return True
    return any(
        encoded.endswith(affix) if affix.startswith(b"-") else encoded.startswith(affix)
        for affix in affixes
    )


def compiles_slot(evidence: dict, executable: str, slot: int) -> bool:
    """Whether this number appears as a literal slot anywhere in the holder."""
    return slot in evidence[executable][2]


def main() -> None:
    evidence = component_evidence()
    if not evidence:
        fail("found no component crates to check pins against")

    labelled = {reason: 0 for reason in BUILDER.SLOT_REASONS}
    unpinned = 0
    profiles = 0
    exempt = 0
    migratable: list[str] = []
    seen_automatic: set[tuple[str, str, str]] = set()

    for source in CORPUS:
        manifest = decode(source)
        # Totality and soundness, on exactly the predicate every product build
        # applies, so this gate and `load_manifest` cannot disagree. That
        # predicate takes each pin's expected reason over the source *and* every
        # boot profile it declares, so a label that only holds before
        # `resolve_boot_profile` narrows the graph is refused here too.
        BUILDER.validate_slot_reasons(manifest, source.name)
        profiles += len(manifest.get("bootProfiles") or [])

        for instance in manifest.get("instances", []):
            executable = instance["executable"]
            for binding in instance.get("bindings", []):
                if binding.get("slot") is None:
                    unpinned += 1
                    continue
                reason = binding["slotReason"]
                labelled[reason] += 1
                if reason != BUILDER.SLOT_REASON_COMPONENT_ABI:
                    continue
                if executable in SOURCELESS_EXECUTABLES:
                    exempt += 1
                    continue
                if executable not in evidence:
                    # Fail closed. Defaulting to empty evidence would make the
                    # clause silently vacuous for exactly the holders whose
                    # sources this gate cannot read, which is indistinguishable
                    # from a pass.
                    fail(
                        f"{source.name}: {instance['name']} pins slot {binding['slot']} as "
                        f"{reason}, but no readable source implements {executable!r}, so the "
                        "claim that its source consumes the position cannot be checked; add it "
                        "to C_COMPONENT_SOURCES or SOURCELESS_EXECUTABLES with a reason"
                    )
                # Minimality: `componentAbi` asserts a positional consumer. If
                # the holder resolves this grant by name and its own sources
                # never mention the number, the assertion is unsupported and the
                # pin is a migration this gate should surface rather than keep.
                if resolves_by_name(evidence, executable, binding["grant"]) and not compiles_slot(
                    evidence, executable, binding["slot"]
                ):
                    migratable.append(
                        f"{instance['name']}/{binding['grant']} in {source.name} pins slot "
                        f"{binding['slot']} as {reason}, but {executable} resolves that grant "
                        "by name and compiles no such slot number"
                    )

        # Automatic-slot expectations remain unpinned and allocator-stable.
        resolved = BUILDER.resolved_slot_table(manifest)
        for instance in manifest.get("instances", []):
            for binding in instance.get("bindings", []):
                key = (source.name, instance["name"], binding["grant"])
                if key not in AUTOMATIC_SLOT_EXPECTATIONS:
                    continue
                seen_automatic.add(key)
                if binding.get("slot") is not None:
                    fail(
                        f"{source.name}: {instance['name']}/{binding['grant']} is pinned again to "
                        f"slot {binding['slot']}. It was removed because the holder resolves it by "
                        "name and compiles no such position; re-pinning it needs a slotReason the "
                        "builder can confirm, and AUTOMATIC_SLOT_EXPECTATIONS updated to say why"
                    )
                expected = AUTOMATIC_SLOT_EXPECTATIONS[key]
                actual = resolved[(instance["name"], "capability", binding["grant"])]
                if actual != expected:
                    fail(
                        f"{source.name}: {instance['name']}/{binding['grant']} now resolves to slot "
                        f"{actual}, not the {expected} observed when its pin was removed. The "
                        "removal was proven byte-identical at that number; re-check the plane"
                    )

    if migratable:
        for line in migratable:
            print(f"slot pin reasons: {line}", file=sys.stderr)
        fail(f"{len(migratable)} pin(s) claim a positional consumer that does not exist")

    # Every expectation must be reached so a rename cannot silently retire it.
    missing = sorted(set(AUTOMATIC_SLOT_EXPECTATIONS) - seen_automatic)
    if missing:
        fail(
            "AUTOMATIC_SLOT_EXPECTATIONS names bindings that no longer exist: "
            + ", ".join(f"{manifest}:{instance}/{grant}" for manifest, instance, grant in missing)
            + " -- update the expectation with the new names rather than dropping it"
        )

    total = sum(labelled.values())
    print(
        f"slot pin reasons check: {total} pinned bindings across {len(CORPUS)} manifests "
        f"and {profiles} boot profiles ({unpinned} automatic)"
    )
    for reason in BUILDER.SLOT_REASONS:
        print(f"  {reason}: {labelled[reason]}")
    print(
        f"  componentAbi pins on sourceless executables, exempt from the minimality "
        f"clause: {exempt} ({', '.join(sorted(SOURCELESS_EXECUTABLES))})"
    )
    print(
        f"  formerly pinned, now allocator-reproduced at their observed slots: "
        f"{len(AUTOMATIC_SLOT_EXPECTATIONS)}"
    )
    print("slot pin reasons check: ok")


if __name__ == "__main__":
    main()
