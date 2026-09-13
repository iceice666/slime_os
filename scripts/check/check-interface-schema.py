#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))
_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "generate"))

import copy
import importlib.util
import tempfile
from pathlib import Path
from types import SimpleNamespace

import interface_schema_contract as contract
from harness import ROOT
from interface_schema import (
    InterfaceSchemaError,
    admit_interfaces,
    compile_interface,
    render_rust,
)

GENERATOR_PATH = ROOT / "scripts" / "generate" / "generate-interface-schema-bindings.py"


def load_generator():
    specification = importlib.util.spec_from_file_location("interface_schema_generator", GENERATOR_PATH)
    if specification is None or specification.loader is None:
        raise SystemExit("cannot load interface-schema generator")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def zti(value: object, indent: int = 0) -> str:
    padding = " " * indent
    if isinstance(value, dict):
        rows = ["{"]
        for key, item in value.items():
            rows.append(f"{' ' * (indent + 2)}{key} = {zti(item, indent + 2)};")
        rows.append(f"{padding}}}")
        return "\n".join(rows)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = ["["]
        rows.extend(f"{' ' * (indent + 2)}{zti(item, indent + 2)};" for item in value)
        rows.append(f"{padding}]")
        return "\n".join(rows)
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'
    raise TypeError(type(value))


def scalar(name: str, width: int = 4, signed: bool = False) -> dict:
    return {
        "name": name,
        "kind": "scalar",
        "width": width,
        "signed": signed,
        "bound": 0,
        "typeName": "",
    }


def record(name: str, target: str) -> dict:
    return {
        "name": name,
        "kind": "record",
        "width": 0,
        "signed": False,
        "bound": 0,
        "typeName": target,
    }


def sequence(name: str, bound: int, *, width: int = 4, signed: bool = False, target: str = "") -> dict:
    return {
        "name": name,
        "kind": "sequence",
        "width": 0 if target else width,
        "signed": False if target else signed,
        "bound": bound,
        "typeName": target,
    }


def base_schema() -> dict:
    return {
        "formatVersion": 1,
        "name": "FixtureStream",
        "kind": "stream",
        "roles": [{"role": "item", "typeName": "Envelope"}],
        "types": [
            {"name": "Envelope", "fields": [record("payload", "Payload")]},
            {"name": "Payload", "fields": [scalar("value")]},
        ],
    }


def write_schema(root: Path, name: str, schema: dict) -> Path:
    path = root / name
    path.write_text(zti(schema) + "\n", encoding="utf-8")
    return path


def rejected(label: str, action) -> None:
    try:
        action()
    except InterfaceSchemaError:
        return
    raise SystemExit(f"{label} was accepted")


def main() -> None:
    generator = load_generator()
    first = generator.render()
    second = generator.render()
    if first != second:
        raise SystemExit("interface bindings changed across identical runs")

    with tempfile.TemporaryDirectory(prefix="slime-interface-schema-check-") as temporary:
        root = Path(temporary)
        original = base_schema()
        reordered = copy.deepcopy(original)
        reordered["types"].reverse()
        left = compile_interface(write_schema(root, "left.zti", original))
        right = compile_interface(write_schema(root, "right.zti", reordered))
        if (left.normalized, left.identity, left.type_tag) != (
            right.normalized,
            right.identity,
            right.type_tag,
        ):
            raise SystemExit("declaration order changed normalized interface identity")
        if render_rust([left]) != render_rust([right]):
            raise SystemExit("equivalent input changed generated Rust bindings")
        formatted = root / "formatted.zti"
        formatted.write_text(
            "\n\n" + zti(original).replace(" = ", "    =    ") + "\n\n",
            encoding="utf-8",
        )
        formatted_interface = compile_interface(formatted)
        if (left.normalized, left.identity, left.type_tag) != (
            formatted_interface.normalized,
            formatted_interface.identity,
            formatted_interface.type_tag,
        ):
            raise SystemExit("source formatting changed normalized interface identity")

        changed = []
        width = copy.deepcopy(original)
        width["types"][1]["fields"][0]["width"] = 8
        changed.append(width)
        signed = copy.deepcopy(original)
        signed["types"][1]["fields"][0]["signed"] = True
        changed.append(signed)
        field_order = copy.deepcopy(original)
        field_order["types"][1]["fields"].append(scalar("other"))
        reversed_fields = copy.deepcopy(field_order)
        reversed_fields["types"][1]["fields"].reverse()
        changed.append(field_order)
        bound_four = copy.deepcopy(original)
        bound_four["types"][1]["fields"] = [sequence("values", 4)]
        bound_five = copy.deepcopy(bound_four)
        bound_five["types"][1]["fields"][0]["bound"] = 5
        changed.append(bound_four)
        nesting_left = copy.deepcopy(original)
        nesting_left["types"] = [
            {
                "name": "Envelope",
                "fields": [record("left", "Payload"), record("right", "Alternate")],
            },
            {"name": "Payload", "fields": [scalar("value")]},
            {"name": "Alternate", "fields": [scalar("value")]},
        ]
        nesting_right = copy.deepcopy(nesting_left)
        nesting_right["types"][0]["fields"][0]["typeName"] = "Alternate"
        nesting_right["types"][0]["fields"][1]["typeName"] = "Payload"
        changed.append(nesting_left)
        call = copy.deepcopy(original)
        call["kind"] = "call"
        call["roles"] = [
            {"role": "reply", "typeName": "Payload"},
            {"role": "request", "typeName": "Envelope"},
        ]
        changed.append(call)
        changed_identities = [
            compile_interface(write_schema(root, f"changed-{index}.zti", schema)).identity
            for index, schema in enumerate(changed)
        ]
        reversed_identity = compile_interface(
            write_schema(root, "reversed-fields.zti", reversed_fields)
        ).identity
        bound_five_identity = compile_interface(
            write_schema(root, "bound-five.zti", bound_five)
        ).identity
        nesting_right_identity = compile_interface(
            write_schema(root, "nesting-right.zti", nesting_right)
        ).identity
        if any(identity == left.identity for identity in changed_identities):
            raise SystemExit("semantic schema change reused the original identity")
        if changed_identities[2] == reversed_identity:
            raise SystemExit("field order did not affect interface identity")
        if changed_identities[3] == bound_five_identity:
            raise SystemExit("sequence bound did not affect interface identity")
        if changed_identities[4] == nesting_right_identity:
            raise SystemExit("nesting edge did not affect interface identity")

        malformed = root / "malformed.zti"
        malformed.write_text("{", encoding="utf-8")
        rejected("malformed schema", lambda: compile_interface(malformed))

        unsupported = copy.deepcopy(original)
        unsupported["types"][1]["fields"][0]["kind"] = "pointer"
        rejected(
            "unsupported field kind",
            lambda: compile_interface(write_schema(root, "unsupported.zti", unsupported)),
        )

        duplicate = copy.deepcopy(original)
        duplicate["types"][1]["fields"].append(scalar("value"))
        rejected(
            "duplicate field",
            lambda: compile_interface(write_schema(root, "duplicate.zti", duplicate)),
        )

        keyword = copy.deepcopy(original)
        keyword["types"][1]["fields"][0]["name"] = "self"
        rejected(
            "Rust keyword identifier",
            lambda: compile_interface(write_schema(root, "keyword.zti", keyword)),
        )

        alias_collision = copy.deepcopy(original)
        alias_collision["name"] = "Envelope"
        rejected(
            "contract alias collision",
            lambda: admit_interfaces(
                [write_schema(root, "alias-collision.zti", alias_collision)]
            ),
        )

        module_a = copy.deepcopy(original)
        module_a["name"] = "HTTPServer"
        module_b = copy.deepcopy(original)
        module_b["name"] = "HttpServer"
        rejected(
            "Rust module collision",
            lambda: admit_interfaces(
                [
                    write_schema(root, "module-a.zti", module_a),
                    write_schema(root, "module-b.zti", module_b),
                ]
            ),
        )

        over_sequence = copy.deepcopy(original)
        over_sequence["types"][1]["fields"] = [
            sequence("values", contract.MAX_SEQUENCE_ELEMENTS + 1)
        ]
        rejected(
            "over-bound sequence",
            lambda: compile_interface(write_schema(root, "over-sequence.zti", over_sequence)),
        )

        deep = {
            "formatVersion": 1,
            "name": "DeepStream",
            "kind": "stream",
            "roles": [{"role": "item", "typeName": "Depth0"}],
            "types": [],
        }
        for index in range(contract.MAX_DEPTH + 1):
            fields = (
                [record("next", f"Depth{index + 1}")]
                if index < contract.MAX_DEPTH
                else [scalar("value")]
            )
            deep["types"].append({"name": f"Depth{index}", "fields": fields})
        rejected(
            "over-bound depth",
            lambda: compile_interface(write_schema(root, "deep.zti", deep)),
        )

        huge = {
            "formatVersion": 1,
            "name": "HugeStream",
            "kind": "stream",
            "roles": [{"role": "item", "typeName": "Root"}],
            "types": [
                {"name": "Root", "fields": [sequence("items", 4096, target="Wide")]},
                {
                    "name": "Wide",
                    "fields": [scalar(f"field{index}", width=8) for index in range(32)],
                },
            ],
        }
        rejected(
            "over-bound encoded size",
            lambda: compile_interface(write_schema(root, "huge.zti", huge)),
        )

        distinct = copy.deepcopy(original)
        distinct["name"] = "DistinctStream"
        collision_paths = [
            write_schema(root, "collision-a.zti", original),
            write_schema(root, "collision-b.zti", distinct),
        ]
        rejected(
            "forced type-tag collision",
            lambda: admit_interfaces(collision_paths, tag_deriver=lambda _: 1),
        )
        rejected(
            "over-bound admitted set",
            lambda: admit_interfaces([collision_paths[0]] * (contract.MAX_SCHEMAS + 1)),
        )

        tiny = SimpleNamespace(
            **{
                name: getattr(contract, name)
                for name in dir(contract)
                if name.isupper()
            }
        )
        tiny.MAX_GENERATED_BYTES = 1
        rejected("over-bound generated output", lambda: render_rust([left], contract=tiny))

    print("interface schema normalization, identity, bounds, collision, and bindings: ok")


if __name__ == "__main__":
    main()
