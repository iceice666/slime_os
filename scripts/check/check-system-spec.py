#!/usr/bin/env python3

"""CP1 system-specification and generation-derivation gate.

Validates every `contracts/system-spec/v1/systems/*.zti` against the component
specs it references, derives a `contracts/generation/v1` manifest from each, and
requires the derived manifest to be semantically identical to the committed
fixture it replaces — same components, same authority, same graph, same resolved
slots, same admitted bytes.

"Semantically identical" rather than "byte-identical" is deliberate and is
checked rather than asserted: the derived manifest is emitted in canonical
order, while the hand-authored fixtures carry the order they were typed in. Every
one of those sections is sorted by `build-generation.py` before it is encoded, so
the comparison below normalizes exactly what the builder normalizes and nothing
else. Any other divergence is a real one and fails.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import json
import os
import subprocess
import tempfile
from pathlib import Path

import system_spec_contract as CONTRACT
from component_spec import admit_specs, interface_catalogue
from harness import ROOT, load_script
from system_spec import (
    DERIVED_GENERATION_FIXTURES,
    GENERATION_FIXTURES,
    SystemSpecError,
    compile_system,
    derive_manifest,
    system_paths,
)
from zutai_cli import STDLIB, binary


# The pre-CP1 hand-authored fixtures, frozen. They are the derivation's
# reference: once a fixture became this generator's output, comparing the
# derivation against it would only assert the generator agrees with itself.
# These files are never regenerated and never edited.
BASELINE_FIXTURES = ROOT / "contracts" / "system-spec" / "v1" / "baselines"
BUILDER = load_script("system_spec_check_builder", "build/build-generation.py")

# Which committed generation fixture each system spec derives. Declared in
# `scripts/lib/system_spec.py` so the gate and
# `scripts/generate/generate-generation-from-spec.py` cannot disagree about what
# is converted, and so importing one does not run the other.
DERIVED_FIXTURES = DERIVED_GENERATION_FIXTURES

# Sections `build-generation.py` sorts before encoding, and the key it sorts by.
# Normalizing exactly these is what makes the comparison below a real equality
# test rather than a lenient one: anything outside this set must match as-is.
SORTED_SECTIONS = {
    "objects": lambda entry: entry["id"],
    "executables": lambda entry: entry["name"],
    "instances": lambda entry: entry["name"],
    "state": lambda entry: entry["name"],
    "grants": lambda entry: (entry["name"], entry["source"], entry["target"]),
    "sharedBufferBudget": lambda entry: entry["holder"],
    "privateMemoryBudget": lambda entry: entry["holder"],
}

# Sections the frozen baseline predates.
#
# The baseline is the pre-CP1 hand-authored `valid.zti`, and it is never
# regenerated and never edited — that is what makes it evidence rather than the
# generator's own output. So a section the repository adds *after* it was frozen
# cannot appear there, and the derivation legitimately produces one the baseline
# has no opinion about. C10.4's `privateMemoryBudget`, and the
# `private-memory-budget` resource object that carries it, are the first such
# section.
#
# Excused for the baseline comparison, never unchecked. `check_post_baseline`
# below asserts the derived content equals what the component specs declare,
# independently of the baseline, so the excusal is "the baseline cannot speak to
# this" rather than "this is unverified". A blanket ignore here would let any
# future divergence hide inside these names, which is exactly the failure
# `KNOWN_DEAD_BINDINGS` is written to avoid on its own axis.
POST_BASELINE_SECTIONS = ("privateMemoryBudget",)
POST_BASELINE_OBJECTS = ("private-memory-budget",)

# Bindings the committed `valid.zti` declares that name no grant at all. They are
# dead text: `resolve_boot_profile` drops any binding whose grant is absent, so
# every profile — `default`, `test`, `visibility`, `unified` — already boots
# without them, and no generation byte carries them. The derivation cannot
# reproduce them because it builds bindings from the grant table, which is the
# point; listing them here records that the divergence is a removal of dead text
# rather than a lost fact.
KNOWN_DEAD_BINDINGS = {
    ("filesystem-service", "filesystem-store"),
    ("generation-manager", "generation-boot-update"),
    ("generation-manager", "health-confirmation"),
    ("storage-store-probe", "store-access"),
}


def fail(message: str) -> None:
    raise SystemExit(f"system spec check: {message}")


def zti(value: object, indent: int = 0) -> str:
    padding = " " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = "".join(f"{padding}  {zti(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def _decode(path: Path, label: str) -> dict:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        check=False,
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        fail(f"cannot read {label}: {process.stderr.strip()}")
    return json.loads(process.stdout)


def load_baseline(name: str) -> dict:
    """The frozen pre-CP1 hand-authored fixture this derivation must reproduce."""
    path = BASELINE_FIXTURES / name
    if not path.is_file():
        fail(f"no frozen baseline for {name}; it is what the derivation is checked against")
    return _decode(path, f"baseline {name}")


def normalized(manifest: dict) -> dict:
    """The manifest as the builder will see it: sorted sections, resolved slots."""
    value = copy.deepcopy(manifest)
    for section, key in SORTED_SECTIONS.items():
        if section in value:
            value[section] = sorted(value[section], key=key)
    if "health" in value:
        value["health"] = dict(
            value["health"], requiredInstances=sorted(value["health"]["requiredInstances"])
        )
    return BUILDER.assign_declared_slots(value)


def strip_dead_bindings(manifest: dict) -> dict:
    value = copy.deepcopy(manifest)
    for instance in value["instances"]:
        instance["bindings"] = [
            binding
            for binding in instance["bindings"]
            if (instance["name"], binding["grant"]) not in KNOWN_DEAD_BINDINGS
        ]
    return value


def split_post_baseline(manifest: dict) -> tuple[dict, dict]:
    """Separate the sections the frozen baseline predates from the rest.

    Returns `(comparable, added)`. `comparable` is what the baseline can be
    compared against; `added` is what `check_post_baseline` asserts on its own
    terms.
    """
    value = copy.deepcopy(manifest)
    added = {section: value.pop(section) for section in POST_BASELINE_SECTIONS if section in value}
    if "objects" in value:
        kept, removed = [], []
        for entry in value["objects"]:
            (removed if entry["id"] in POST_BASELINE_OBJECTS else kept).append(entry)
        value["objects"] = kept
        added["objects"] = removed
    return value, added


def check_post_baseline(name: str, derived: dict, system) -> None:
    """The post-baseline sections say exactly what the component specs declare.

    The baseline predates these, so it cannot check them — and an excusal with
    nothing behind it would let a wrong budget through under a name the
    comparison skips. This is the replacement assertion, and it is stricter than
    the baseline's would have been: it compares the derived budget against the
    specs the system composes rather than against a frozen copy of one answer.
    """
    expected = {
        component: COMPONENTS[component]["runtime"]["resource"]["privatePageQuota"]
        for component in system.spec["components"]
        if COMPONENTS[component]["runtime"]["resource"]["privatePageQuota"]
    }
    budget = {entry["holder"]: entry["pageQuota"] for entry in derived.get("privateMemoryBudget", [])}
    if budget != expected:
        fail(
            f"{name}: derived privateMemoryBudget {sorted(budget.items())} does not match "
            f"the declared privatePageQuota of the components it composes "
            f"{sorted(expected.items())}"
        )
    # And the resource object is present exactly when the section has holders:
    # the builder refuses a budget without the object and encodes nothing from an
    # object with no holders, so either half alone is a generation that boots
    # with every declared quota silently denied.
    objects = {entry["id"] for entry in derived["objects"]}
    carried = "private-memory-budget" in objects
    if carried != bool(budget):
        fail(
            f"{name}: privateMemoryBudget has {len(budget)} holder(s) but the "
            f"private-memory-budget resource object is {'present' if carried else 'absent'}"
        )


def first_difference(left: object, right: object, label: str) -> str:
    if isinstance(left, list) and isinstance(right, list):
        if len(left) != len(right):
            return f"{label}: {len(left)} entries vs {len(right)}"
        for index, (a, b) in enumerate(zip(left, right, strict=True)):
            if a != b:
                return first_difference(a, b, f"{label}[{index}]")
    if isinstance(left, dict) and isinstance(right, dict):
        for key in sorted(set(left) | set(right)):
            if left.get(key) != right.get(key):
                return first_difference(left.get(key), right.get(key), f"{label}.{key}")
    return f"{label}: {left!r} != {right!r}"


CATALOGUE = interface_catalogue()
COMPONENTS = {entry.name: entry.spec for entry in admit_specs(catalogue=CATALOGUE)}

paths = system_paths()
if not paths:
    fail("no system specs declared")
if {path.stem for path in paths} != set(DERIVED_FIXTURES):
    fail(
        f"system specs {sorted(path.stem for path in paths)} do not match the declared "
        f"derivation table {sorted(DERIVED_FIXTURES)}"
    )

systems = {}
for path in paths:
    try:
        systems[path.stem] = compile_system(path, components=COMPONENTS)
    except SystemSpecError as error:
        fail(f"{path.name}: {error}")

identities = {entry.identity for entry in systems.values()}
if len(identities) != len(systems):
    fail("two system specs computed the same identity")

# 1. Each system derives the fixture it replaces.
for name, system in sorted(systems.items()):
    # The pre-CP1 hand-authored fixture, frozen under
    # `contracts/system-spec/v1/baselines/`. Comparing against the *committed*
    # fixture would be circular now that the fixture is this generator's own
    # output: it would assert the generator agrees with itself. The baseline is
    # what the derivation has to reproduce, and it never changes again.
    fixture = load_baseline(DERIVED_FIXTURES[name])
    derived = normalized(derive_manifest(system))
    committed = normalized(strip_dead_bindings(fixture))
    # Sections the baseline predates are lifted out and asserted separately: it
    # was frozen before they existed, so it has no opinion to compare against.
    derived, _added = split_post_baseline(derived)
    committed, _ = split_post_baseline(committed)
    if derived != committed:
        fail(
            f"{name}: derived manifest diverges from {DERIVED_FIXTURES[name]}: "
            f"{first_difference(derived, committed, 'manifest')}"
        )
    check_post_baseline(name, normalized(derive_manifest(system)), system)
    # And the removal above must be exactly the dead bindings, never a live one:
    # comparing against the unmodified fixture must fail for that reason alone.
    untouched = normalized(fixture)
    if untouched != committed:
        removed = {
            (instance["name"], binding["grant"])
            for instance in fixture["instances"]
            for binding in instance["bindings"]
        } - {
            (instance["name"], binding["grant"])
            for instance in committed["instances"]
            for binding in instance["bindings"]
        }
        if not removed <= KNOWN_DEAD_BINDINGS:
            fail(f"{name}: stripped a binding that is not declared dead: {sorted(removed)}")
        grants = {grant["name"] for grant in fixture["grants"]}
        live = [entry for entry in removed if entry[1] in grants]
        if live:
            fail(f"{name}: a stripped binding names a real grant: {sorted(live)}")

    # Every surviving binding must keep the exact slot the *unstripped* fixture
    # resolved for it.
    #
    # This is the check whose absence let a real defect through. Removing the
    # four dead bindings frees the slots they occupied, and
    # `assign_declared_slots` then hands those numbers to the next binding in
    # grant-name order: `generation-manager`'s rollback/select/stage bindings
    # silently moved 2->1, 3->2, 4->3. The comparison above could not see it,
    # because it strips first and resolves second, so both sides shifted
    # together. Resolving the untouched fixture independently is what makes the
    # slot a checked fact rather than a coincidence, and the system spec pins
    # those three numbers to hold it.
    resolved_fixture = BUILDER.assign_declared_slots(copy.deepcopy(fixture))
    original_slots = {
        (instance["name"], binding["grant"]): binding["slot"]
        for instance in resolved_fixture["instances"]
        for binding in instance["bindings"]
    }
    derived_slots = {
        (instance["name"], binding["grant"]): binding["slot"]
        for instance in derived["instances"]
        for binding in instance["bindings"]
    }
    moved = {
        key: (original_slots[key], slot)
        for key, slot in derived_slots.items()
        if key in original_slots and original_slots[key] != slot
    }
    if moved:
        fail(
            f"{name}: derivation moved capability slot(s) the committed fixture pinned: "
            + ", ".join(
                f"{holder}/{grant} {was}->{now}" for (holder, grant), (was, now) in sorted(moved.items())
            )
            + "; pin them in the system spec"
        )

# 1b. Byte-level drift. The committed fixtures are now this generator's output,
#     so regenerating them must reproduce the committed bytes exactly — the same
#     `--check` discipline every other generated artifact in this repository is
#     held to. Without this, the semantic comparison above would be trivially
#     true against a fixture nobody could reproduce, and the fixtures would drift
#     back into hand-edited text one edit at a time.
for name, system in sorted(systems.items()):
    fixture = GENERATION_FIXTURES / DERIVED_FIXTURES[name]
    rendered = zti(derive_manifest(system)) + "\n"
    if fixture.read_text(encoding="utf-8") != rendered:
        fail(
            f"{DERIVED_FIXTURES[name]} is stale: regenerate it with "
            f"python3 scripts/generate/generate-generation-from-spec.py"
        )

# 2. The derivation is a function of its inputs alone.
for name, system in sorted(systems.items()):
    if normalized(derive_manifest(system)) != normalized(derive_manifest(system)):
        fail(f"{name}: two derivations of one system spec disagree")

# 3. Every derived manifest is a real `contracts/generation/v1` record: written
#    out, it decodes under the generation schema. A derivation that produced a
#    shape the manifest contract rejects would pass every check above.
with tempfile.TemporaryDirectory(prefix="slime-system-spec-check-") as temporary:
    root = Path(temporary)
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    for name, system in sorted(systems.items()):
        path = root / f"{name}.zti"
        path.write_text(zti(derive_manifest(system)) + "\n", encoding="utf-8")
        process = subprocess.run(
            [str(binary()), "json", str(path)],
            cwd=ROOT,
            check=False,
            env=environment,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
        )
        if process.returncode != 0:
            fail(f"{name}: derived manifest is not valid Zutai: {process.stderr.strip()}")

    # 4. Named refusals. Each mutation is applied to a compiled system and must
    #    be refused by the rule it names, with the unmutated baseline admitted
    #    first so no arm can pass by tripping an unrelated guard (B67).
    baseline = systems["sel4-channel"]

    def write_system(directory: Path, spec: dict) -> Path:
        path = directory / f"{spec['name']}.zti"
        path.write_text(zti(spec) + "\n", encoding="utf-8")
        return path

    def source() -> dict:
        return copy.deepcopy(baseline.spec)

    arms = root / "arms"
    arms.mkdir()
    compile_system(write_system(arms, source()), components=COMPONENTS)

    def rejected(label: str, mutate) -> None:
        spec = source()
        mutate(spec)
        try:
            compile_system(write_system(arms, spec), components=COMPONENTS)
        except SystemSpecError:
            return
        fail(f"{label} was accepted")

    def undeclared_component(spec: dict) -> None:
        spec["components"] = sorted(spec["components"] + ["no-such-component"])

    def unknown_target(spec: dict) -> None:
        spec["targetRequirement"] = "aarch64-imaginary"

    def negative_slot_pin(spec: dict) -> None:
        spec["slotPins"] = [{"holder": "init", "grant": "dango-output", "slot": -1}]

    def grant_to_unadmitted(spec: dict) -> None:
        spec["grants"][0]["target"] = "dango"

    def unknown_capability_kind(spec: dict) -> None:
        spec["grants"][0]["capabilityKind"] = "pciFunction"

    def rights_outside_kind(spec: dict) -> None:
        spec["grants"][0]["rights"] = ["blockWrite"]

    def stale_slot_pin(spec: dict) -> None:
        spec["slotPins"] = [{"holder": "console", "grant": "no-such-grant", "slot": 3}]

    def duplicate_slot_pin(spec: dict) -> None:
        spec["slotPins"] = [
            {"holder": "init", "grant": "dango-output", "slot": 4},
            {"holder": "init", "grant": "init-console", "slot": 4},
        ]

    def bootstrap_not_init(spec: dict) -> None:
        spec["bootstrapInstance"] = "console"

    def bootstrap_unadmitted(spec: dict) -> None:
        spec["bootstrapInstance"] = "dango"

    def duplicate_grant_name(spec: dict) -> None:
        spec["grants"].append(copy.deepcopy(spec["grants"][0]))

    def commands_without_executables(spec: dict) -> None:
        # A command profile in a system granting no executable authority: the
        # profile advertises commands nothing in this generation can launch.
        spec["commandBindings"] = [{"component": "console", "commands": ["init"]}]
        spec["grants"] = [
            grant for grant in spec["grants"] if grant["capabilityKind"] != "executable"
        ]
        spec["slotPins"] = []

    def state_owner_unadmitted(spec: dict) -> None:
        spec["state"] = [
            {"name": "orphan", "owner": "dango", "policy": "preserve", "schemaVersion": 1}
        ]

    def unknown_state_policy(spec: dict) -> None:
        spec["state"] = [
            {"name": "orphan", "owner": "console", "policy": "forever", "schemaVersion": 1}
        ]

    def unsorted_components(spec: dict) -> None:
        spec["components"] = list(reversed(spec["components"]))

    def over_bounded_slot_pins(spec: dict) -> None:
        # One more pin than the contract's `maxSlotPins` admits. Every pin names
        # a real binding, so only the bound can refuse this.
        spec["slotPins"] = [
            {"holder": "init", "grant": "dango-output", "slot": index}
            for index in range(CONTRACT.MAX_SLOT_PINS + 1)
        ]

    def over_bounded_text(spec: dict) -> None:
        spec["acceptanceCriteria"] = "x" * (CONTRACT.MAX_TEXT_BYTES + 1)

    def duplicate_state_name(spec: dict) -> None:
        spec["state"] = [
            {"name": "twice", "owner": "console", "policy": "preserve", "schemaVersion": 1},
            {"name": "twice", "owner": "init", "policy": "preserve", "schemaVersion": 1},
        ]

    def dependency_unadmitted(spec: dict) -> None:
        # A component whose declared dependency the system does not admit.
        #
        # `bootstrapInstance` must stay an admitted init component or the
        # bootstrap rules fire first and this arm proves nothing — which is what
        # it did before, in both directions. So `init` stays, and `dango` is
        # added instead: its component spec depends on `console`, which this
        # mutation removes, leaving the dependency rule as the only reason to
        # refuse. Verified by neutralizing that rule and observing the arm
        # falsely pass.
        spec["components"] = sorted(
            name for name in spec["components"] + ["dango"] if name != "console"
        )
        spec["placements"] = [
            entry for entry in spec["placements"] if entry["component"] != "console"
        ]
        spec["grants"] = []
        spec["slotPins"] = []

    def interface_without_graph(spec: dict) -> None:
        # A component declaring route roles, in a system with no fabric graph.
        #
        # `fabric-publisher` depends on `fabric-service`, so adding it alone was
        # refused by the dependency rule rather than the graph rule. Admitting
        # both, and clearing the dependency that would fire first, leaves the
        # missing graph as the only reason to refuse.
        spec["components"] = sorted(spec["components"] + ["fabric-publisher", "fabric-service"])
        spec["placements"] = spec["placements"] + [
            {"component": "fabric-publisher", "owner": "init"},
            {"component": "fabric-service", "owner": "init"},
        ]
        spec["placements"].sort(key=lambda entry: entry["component"])

    refusals = 0
    for label, mutate in (
        ("a component no component spec declares", undeclared_component),
        ("a target profile the table does not name", unknown_target),
        ("a negative slot pin", negative_slot_pin),
        ("a grant naming an unadmitted target", grant_to_unadmitted),
        ("a grant with an unknown capability kind", unknown_capability_kind),
        ("a grant carrying rights its kind forbids", rights_outside_kind),
        ("a slot pin for a binding the grants do not produce", stale_slot_pin),
        ("two slot pins on one holder slot", duplicate_slot_pin),
        ("a bootstrap instance that is not an init component", bootstrap_not_init),
        ("a bootstrap instance the system does not admit", bootstrap_unadmitted),
        ("a duplicate grant name", duplicate_grant_name),
        ("commands with no executable authority anywhere", commands_without_executables),
        ("a state binding owned by an unadmitted component", state_owner_unadmitted),
        ("an unknown state policy", unknown_state_policy),
        ("an unsorted component list", unsorted_components),
        ("more slot pins than the declared bound", over_bounded_slot_pins),
        ("text beyond the declared byte bound", over_bounded_text),
        ("a duplicate state binding name", duplicate_state_name),
        ("a dependency the system does not admit", dependency_unadmitted),
        ("a route role declared with no fabric graph", interface_without_graph),
    ):
        rejected(label, mutate)
        refusals += 1

print(
    f"system spec derivation: {len(systems)} systems compiled and "
    f"{len(DERIVED_FIXTURES)} generation manifests derived semantically identical to "
    f"their committed fixtures; {refusals} named mutations refused"
)
