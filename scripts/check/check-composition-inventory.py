#!/usr/bin/env python3

"""CP12 closed-inventory gate.

`contracts/composition-inventory/v1/inventory.zti` claims, for every
composition this repository ships, whether it is generator output derived from
a `contracts/system-spec/v1` source or still hand-authored, and why a
hand-authored one is deferred. This gate refuses the claim unless it agrees
with the repository itself:

1. the row set and the composition directory are the same set — no composition
   without a row, no row without a composition;
2. every `derived` row names an existing system spec and an existing frozen
   baseline, and is exactly a member of `system_spec.DERIVED_GENERATION_FIXTURES`;
3. every `handAuthored` row is absent from that table and carries a reason from
   the contract's closed vocabulary;
4. the two path fields are non-empty exactly for `derived` rows and the reason
   exactly for `handAuthored` ones, so a row cannot be both;
5. every `owningGate` is a real Justfile recipe, on the same terms
   `contracts/component-spec/v1`'s `requiredTestEnvironment` is;
6. the derived count is not zero and the inventory is within its declared bounds.

Then it runs six named mutations of the committed record through the same rules
and requires each to be refused, so the gate is proven able to fail rather than
merely observed passing.
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

import composition_inventory_contract as CONTRACT
from harness import GENERATION_COMPOSITIONS, ROOT
from just_metadata import targets as just_targets
from system_spec import DERIVED_GENERATION_FIXTURES
from zutai_cli import STDLIB, binary

CONTRACT_ROOT = ROOT / "contracts" / "composition-inventory" / "v1"
CHECKER = CONTRACT_ROOT / "check.zt"
INVENTORY = CONTRACT_ROOT / "inventory.zti"
SYSTEM_ROOT = ROOT / "contracts" / "system-spec" / "v1" / "systems"
BASELINE_ROOT = ROOT / "contracts" / "system-spec" / "v1" / "baselines"

_ENTRY_FIELDS = {
    "composition",
    "state",
    "systemSpec",
    "baseline",
    "owningGate",
    "deferralReason",
}


class InventoryError(ValueError):
    pass


def fail(message: str) -> None:
    raise SystemExit(f"composition inventory check: {message}")


def refuse(message: str) -> None:
    raise InventoryError(message)


def decode(path: Path) -> dict:
    """Decode one inventory through the contract's own Zutai checker."""
    if not path.is_file():
        refuse(f"inventory not found: {path}")
    if path.stat().st_size > CONTRACT.MAX_SOURCE_BYTES:
        refuse(f"{path.name}: source exceeds bound")
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_COMPOSITION_INVENTORY_PATH"] = str(path)
    process = subprocess.run(
        [str(binary()), "run", str(CHECKER)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0 or not process.stdout.startswith("#valid"):
        refuse(f"{path.name}: does not match the inventory schema")
    process = subprocess.run(
        [str(binary()), "json", str(path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        refuse(f"{path.name}: invalid Zutai JSON projection")
    return json.loads(process.stdout)


def committed_compositions() -> set[str]:
    return {path.stem for path in GENERATION_COMPOSITIONS.glob("*.zti")}


def validate(inventory: dict, compositions: set[str], derived_table: dict[str, str]) -> int:
    """Every rule above, over one decoded inventory. Returns the derived count."""
    if inventory["formatVersion"] != CONTRACT.FORMAT_VERSION:
        refuse(f"unsupported inventory version {inventory['formatVersion']}")
    entries = inventory["entries"]
    if len(entries) > CONTRACT.MAX_ENTRIES:
        refuse("entries: exceeds the declared bound")
    if not entries:
        refuse("entries: an empty inventory claims nothing")

    names = [entry["composition"] for entry in entries]
    if len(set(names)) != len(names):
        refuse("entries: duplicate composition")
    if names != sorted(names):
        refuse("entries: must be sorted by composition")

    missing = sorted(compositions - set(names))
    if missing:
        refuse(f"compositions with no inventory row: {missing}")
    unknown = sorted(set(names) - compositions)
    if unknown:
        refuse(f"inventory rows naming no composition: {unknown}")

    recipes = just_targets()
    derived = 0
    for entry in entries:
        if set(entry) != _ENTRY_FIELDS:
            refuse(f"{entry.get('composition')}: fields are {sorted(entry)}")
        name = entry["composition"]
        for field in ("composition", "state", "owningGate"):
            if not entry[field]:
                refuse(f"{name}: {field} must be declared")
        for field, bound in (
            ("composition", CONTRACT.MAX_NAME_BYTES),
            ("state", CONTRACT.MAX_NAME_BYTES),
            ("owningGate", CONTRACT.MAX_NAME_BYTES),
            ("deferralReason", CONTRACT.MAX_NAME_BYTES),
            ("systemSpec", CONTRACT.MAX_PATH_BYTES),
            ("baseline", CONTRACT.MAX_PATH_BYTES),
        ):
            if len(entry[field].encode("utf-8")) > bound:
                refuse(f"{name}: {field} exceeds {bound} bytes")
        if entry["state"] not in CONTRACT.STATES:
            refuse(f"{name}: unknown state {entry['state']!r}")
        if entry["owningGate"] not in recipes:
            refuse(f"{name}: owningGate {entry['owningGate']!r} is no Justfile recipe")

        if entry["state"] == CONTRACT.STATE_DERIVED:
            derived += 1
            if entry["deferralReason"]:
                refuse(f"{name}: a derived composition declares no deferral reason")
            for field, root in (("systemSpec", SYSTEM_ROOT), ("baseline", BASELINE_ROOT)):
                declared = entry[field]
                if not declared:
                    refuse(f"{name}: a derived composition must name its {field}")
                path = (ROOT / declared).resolve()
                if not path.is_relative_to(root) or not path.is_file():
                    refuse(f"{name}: {field} {declared!r} is no file under {root.name}/")
                if path.stem != name:
                    refuse(f"{name}: {field} {declared!r} names another composition")
            if name not in derived_table:
                refuse(
                    f"{name}: claimed derived, but the generator's derivation table "
                    "does not convert it"
                )
        else:
            if entry["systemSpec"] or entry["baseline"]:
                refuse(f"{name}: a hand-authored composition names no system spec or baseline")
            if entry["deferralReason"] not in CONTRACT.DEFERRAL_REASONS:
                refuse(f"{name}: unknown deferral reason {entry['deferralReason']!r}")
            if name in derived_table:
                refuse(
                    f"{name}: claimed hand-authored, but the generator derives it; "
                    "the inventory is stale"
                )
    if derived == 0:
        refuse("no composition is derived, so this inventory records no migration at all")
    return derived


def rejected(label: str, mutate) -> None:
    """One named mutation of the committed record, required to be refused."""
    global REFUSALS
    value = copy.deepcopy(COMMITTED)
    mutate(value)
    with tempfile.TemporaryDirectory(prefix="slime-inventory-mutation-") as scope:
        path = Path(scope) / "inventory.zti"
        path.write_text(render(value) + "\n", encoding="utf-8")
        try:
            validate(decode(path), COMPOSITIONS, DERIVED_GENERATION_FIXTURES)
        except InventoryError:
            REFUSALS += 1
            return
    fail(f"{label} was accepted")


def render(value: object, indent: int = 0) -> str:
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
        rows = "".join(f"{padding}  {render(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {render(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


COMPOSITIONS = committed_compositions()
try:
    COMMITTED = decode(INVENTORY)
    DERIVED_COUNT = validate(COMMITTED, COMPOSITIONS, DERIVED_GENERATION_FIXTURES)
except InventoryError as error:
    fail(str(error))

REFUSALS = 0

# The committed record round-trips through this module's own renderer, so a
# mutation below differs from the committed bytes only in what it mutates.
if render(COMMITTED) + "\n" != INVENTORY.read_text(encoding="utf-8"):
    fail(
        "the committed inventory is not in canonical form; "
        "regenerate it rather than hand-editing"
    )


def drop_row(value: dict) -> None:
    value["entries"] = value["entries"][1:]


def add_unknown_row(value: dict) -> None:
    row = copy.deepcopy(value["entries"][0])
    row.update(
        composition="zzz-not-a-composition",
        state=CONTRACT.STATE_HAND_AUTHORED,
        systemSpec="",
        baseline="",
        deferralReason=CONTRACT.DEFERRAL_MULTI_INSTANCE_EXECUTABLE,
    )
    value["entries"].append(row)


def claim_deferred_is_derived(value: dict) -> None:
    """A row cannot be derived *and* carry a deferral reason.

    Stated over a derived row rather than by promoting a hand-authored one,
    because the corpus reached 42 of 42 derived and there is no longer a
    hand-authored row to promote. The invariant is the same one either way --
    `state` and `deferralReason` cannot both be claimed -- and phrasing it from
    the derived side keeps the control alive in exactly the state the milestone
    was working toward.
    """
    for row in value["entries"]:
        if row["state"] == CONTRACT.STATE_DERIVED:
            row.update(deferralReason=CONTRACT.DEFERRAL_ROUTE_NAME_VARIANCE)
            return
    fail("no derived row to mutate")


def claim_derived_is_deferred(value: dict) -> None:
    for row in value["entries"]:
        if row["state"] == CONTRACT.STATE_DERIVED:
            row.update(
                state=CONTRACT.STATE_HAND_AUTHORED,
                systemSpec="",
                baseline="",
                deferralReason=CONTRACT.DEFERRAL_MULTI_INSTANCE_EXECUTABLE,
            )
            return
    fail("no derived row to mutate")


def unknown_reason(value: dict) -> None:
    """A deferral reason outside the closed vocabulary is refused.

    Applied to a hand-authored row when one exists and otherwise to a derived
    row demoted in the same mutation, so the vocabulary stays checked now that
    the corpus is fully derived. The reason string is what this control is
    about; which state carries it is incidental.
    """
    for row in value["entries"]:
        if row["state"] == CONTRACT.STATE_HAND_AUTHORED:
            row["deferralReason"] = "becauseISaidSo"
            return
    for row in value["entries"]:
        if row["state"] == CONTRACT.STATE_DERIVED:
            row.update(
                state=CONTRACT.STATE_HAND_AUTHORED,
                systemSpec="",
                baseline="",
                deferralReason="becauseISaidSo",
            )
            return
    fail("no row to mutate")


def unknown_gate(value: dict) -> None:
    value["entries"][0]["owningGate"] = "not_a_just_recipe"


def missing_baseline(value: dict) -> None:
    for row in value["entries"]:
        if row["state"] == CONTRACT.STATE_DERIVED:
            row["baseline"] = "contracts/system-spec/v1/baselines/nonexistent.zti"
            return
    fail("no derived row to mutate")


rejected("a composition with no inventory row", drop_row)
rejected("an inventory row naming no composition", add_unknown_row)
rejected("a deferred composition claimed as derived", claim_deferred_is_derived)
rejected("a derived composition claimed as deferred", claim_derived_is_deferred)
rejected("an unknown deferral reason", unknown_reason)
rejected("an owning gate that is no Justfile recipe", unknown_gate)
rejected("a derived row whose frozen baseline is missing", missing_baseline)

deferred = len(COMMITTED["entries"]) - DERIVED_COUNT
reasons = sorted(
    {row["deferralReason"] for row in COMMITTED["entries"] if row["deferralReason"]}
)
print(
    f"composition inventory: {len(COMMITTED['entries'])} compositions "
    f"({DERIVED_COUNT} derived from system specs, {deferred} hand-authored), "
    f"every row backed by an owning gate; deferral reasons in use: {', '.join(reasons)}; "
    f"{REFUSALS} named mutations refused"
)
