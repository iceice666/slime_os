from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Callable

import interface_schema_contract as default_contract
from harness import ROOT
from zutai_cli import STDLIB, binary

CHECKER = ROOT / "contracts" / "interface-schema" / "v1" / "check.zt"
GENERATION_SOURCE = ROOT / "contracts" / "generation" / "v1" / "fixtures" / "valid.zti"
INTERFACE_SCHEMA_ROOT = ROOT / "contracts" / "interface-schema" / "v1" / "interfaces"
_ALLOWED_WIDTHS = (1, 2, 4, 8)
_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_]*$")
_RUST_KEYWORDS = {
    "Self",
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "box",
    "break",
    "const",
    "continue",
    "crate",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "gen",
    "if",
    "impl",
    "in",
    "let",
    "loop",
    "macro",
    "match",
    "mod",
    "move",
    "mut",
    "override",
    "priv",
    "pub",
    "ref",
    "return",
    "static",
    "self",
    "struct",
    "super",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
}


class InterfaceSchemaError(ValueError):
    pass


@dataclass(frozen=True)
class CompiledInterface:
    name: str
    kind: str
    normalized: bytes
    identity: bytes
    type_tag: int
    schema: dict
    max_encoded_bytes: int


def _fail(message: str) -> None:
    raise InterfaceSchemaError(message)


def _exact_record(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        _fail(f"{label}: expected fields {sorted(keys)}")
    return value


def _text(value: object, label: str) -> str:
    if not isinstance(value, str):
        _fail(f"{label}: expected text")
    return value


def _integer(value: object, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        _fail(f"{label}: expected integer")
    return value


def _boolean(value: object, label: str) -> bool:
    if not isinstance(value, bool):
        _fail(f"{label}: expected boolean")
    return value


def _list(value: object, label: str) -> list:
    if not isinstance(value, list):
        _fail(f"{label}: expected list")
    return value


def _identifier(value: object, label: str, contract: ModuleType) -> str:
    name = _text(value, label)
    if not name or len(name.encode("utf-8")) > contract.MAX_NAME_BYTES:
        _fail(f"{label}: name exceeds bound")
    if not _NAME.fullmatch(name) or name in _RUST_KEYWORDS:
        _fail(f"{label}: unsupported identifier {name!r}")
    return name


def _run_zutai(path: Path, command: str, *, contract: ModuleType) -> str:
    if not path.is_file():
        _fail(f"interface schema not found: {path}")
    if path.stat().st_size > contract.MAX_SOURCE_BYTES:
        _fail(f"{path}: source exceeds bound")
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    environment["SLIME_INTERFACE_SCHEMA_PATH"] = str(path)
    process = subprocess.run(
        [str(binary()), command, str(CHECKER if command == "run" else path)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _fail(f"{path}: malformed Zutai input: {(process.stderr or process.stdout).strip()}")
    return process.stdout


def _load(path: Path, contract: ModuleType) -> dict:
    decoded = _run_zutai(path, "run", contract=contract)
    if not decoded.startswith("#valid"):
        _fail(f"{path}: input does not match interface schema")
    raw = _run_zutai(path, "json", contract=contract)
    try:
        value = json.loads(raw)
    except json.JSONDecodeError as error:
        _fail(f"{path}: invalid Zutai JSON projection: {error}")
    return _exact_record(
        value,
        {"formatVersion", "name", "kind", "roles", "types"},
        str(path),
    )

def resolve_interface_paths(
    entries: object, contract: ModuleType = default_contract
) -> list[Path]:
    if not isinstance(entries, list):
        _fail("interfaceSchemas must be a list")
    if len(entries) > contract.MAX_SCHEMAS:
        _fail("admitted interface schema count exceeds bound")
    paths = []
    for entry in entries:
        if not isinstance(entry, str):
            _fail("interface schema path must be text")
        path = (ROOT / entry).resolve()
        if not path.is_relative_to(INTERFACE_SCHEMA_ROOT):
            _fail(f"interface schema escapes contract root: {entry}")
        paths.append(path)
    return paths


def load_manifest_interface_paths(
    source: Path = GENERATION_SOURCE, contract: ModuleType = default_contract
) -> list[Path]:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    process = subprocess.run(
        [str(binary()), "json", str(source)],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _fail(f"cannot load interface schema catalog: {process.stderr.strip()}")
    try:
        manifest = json.loads(process.stdout)
    except json.JSONDecodeError as error:
        _fail(f"invalid interface schema catalog: {error}")
    if not isinstance(manifest, dict) or "interfaceSchemas" not in manifest:
        _fail("generation manifest has no interface schema catalog")
    return resolve_interface_paths(manifest["interfaceSchemas"], contract)


def _field(raw: object, owner: str, index: int, contract: ModuleType) -> dict:
    label = f"{owner}.fields[{index}]"
    value = _exact_record(
        raw,
        {"name", "kind", "width", "signed", "bound", "typeName"},
        label,
    )
    field = {
        "name": _identifier(value["name"], f"{label}.name", contract),
        "kind": _text(value["kind"], f"{label}.kind"),
        "width": _integer(value["width"], f"{label}.width"),
        "signed": _boolean(value["signed"], f"{label}.signed"),
        "bound": _integer(value["bound"], f"{label}.bound"),
        "typeName": _text(value["typeName"], f"{label}.typeName"),
    }
    kind = field["kind"]
    if kind not in contract.FIELD_KINDS:
        _fail(f"{label}: unsupported field kind {kind!r}")
    if kind == "scalar":
        if (
            field["width"] not in _ALLOWED_WIDTHS
            or field["bound"] != 0
            or field["typeName"]
        ):
            _fail(f"{label}: invalid scalar shape")
    elif kind == "bytes":
        if (
            field["width"] != 0
            or field["signed"]
            or not 1 <= field["bound"] <= contract.MAX_ENCODED_BYTES
            or field["typeName"]
        ):
            _fail(f"{label}: invalid byte-array shape")
    elif kind == "record":
        if (
            field["width"] != 0
            or field["signed"]
            or field["bound"] != 0
            or not field["typeName"]
        ):
            _fail(f"{label}: invalid record shape")
        _identifier(field["typeName"], f"{label}.typeName", contract)
    else:
        if not 1 <= field["bound"] <= contract.MAX_SEQUENCE_ELEMENTS:
            _fail(f"{label}: sequence bound exceeds limit")
        if field["typeName"]:
            if field["width"] != 0 or field["signed"]:
                _fail(f"{label}: record sequence has scalar metadata")
            _identifier(field["typeName"], f"{label}.typeName", contract)
        elif field["width"] not in _ALLOWED_WIDTHS:
            _fail(f"{label}: scalar sequence has invalid width")
    return field


def _role_order(kind: str) -> tuple[str, ...]:
    return {
        "stream": ("item",),
        "call": ("request", "reply"),
        "operation": ("goal", "feedback", "result"),
    }[kind]


def _normalize(raw: dict, contract: ModuleType) -> dict:
    version = _integer(raw["formatVersion"], "formatVersion")
    if version != contract.FORMAT_VERSION:
        _fail(f"unsupported interface schema version {version}")
    name = _identifier(raw["name"], "name", contract)
    kind = _text(raw["kind"], "kind")
    if kind not in contract.CONTRACT_KINDS:
        _fail(f"unsupported contract kind {kind!r}")

    raw_types = _list(raw["types"], "types")
    if not 1 <= len(raw_types) <= contract.MAX_TYPES:
        _fail("type declaration count exceeds bound")
    types = []
    total_fields = 0
    for index, raw_type in enumerate(raw_types):
        label = f"types[{index}]"
        value = _exact_record(raw_type, {"name", "fields"}, label)
        type_name = _identifier(value["name"], f"{label}.name", contract)
        fields_raw = _list(value["fields"], f"{label}.fields")
        if not 1 <= len(fields_raw) <= contract.MAX_FIELDS_PER_TYPE:
            _fail(f"{type_name}: field count exceeds bound")
        fields = [_field(field, type_name, field_index, contract) for field_index, field in enumerate(fields_raw)]
        field_names = [field["name"] for field in fields]
        if len(set(field_names)) != len(field_names):
            _fail(f"{type_name}: duplicate field name")
        total_fields += len(fields)
        types.append({"name": type_name, "fields": fields})
    if total_fields > contract.MAX_TOTAL_FIELDS:
        _fail("total field count exceeds bound")
    type_names = [item["name"] for item in types]
    if len(set(type_names)) != len(type_names):
        _fail("duplicate type declaration")
    types.sort(key=lambda item: item["name"])

    expected_roles = _role_order(kind)
    raw_roles = _list(raw["roles"], "roles")
    roles_by_name = {}
    for index, raw_role in enumerate(raw_roles):
        label = f"roles[{index}]"
        value = _exact_record(raw_role, {"role", "typeName"}, label)
        role = _text(value["role"], f"{label}.role")
        type_name = _identifier(value["typeName"], f"{label}.typeName", contract)
        if role in roles_by_name:
            _fail(f"duplicate contract role {role!r}")
        roles_by_name[role] = type_name
    if set(roles_by_name) != set(expected_roles):
        _fail(f"{name}: {kind} requires roles {expected_roles}")
    roles = [{"role": role, "typeName": roles_by_name[role]} for role in expected_roles]

    known_types = set(type_names)
    for role in roles:
        if role["typeName"] not in known_types:
            _fail(f"role {role['role']}: unknown type {role['typeName']}")
    for item in types:
        for field in item["fields"]:
            if field["typeName"] and field["typeName"] not in known_types:
                _fail(f"{item['name']}.{field['name']}: unknown type {field['typeName']}")

    return {
        "formatVersion": version,
        "name": name,
        "kind": kind,
        "roles": roles,
        "types": types,
    }


def _metrics(schema: dict, contract: ModuleType) -> int:
    types = {item["name"]: item for item in schema["types"]}
    sizes: dict[str, int] = {}
    depths: dict[str, int] = {}
    active: set[str] = set()

    def record_metrics(name: str) -> tuple[int, int]:
        if name in sizes:
            return sizes[name], depths[name]
        if name in active:
            _fail(f"recursive type cycle through {name}")
        active.add(name)
        size = 0
        depth = 1
        for field in types[name]["fields"]:
            kind = field["kind"]
            if kind == "scalar":
                field_size, field_depth = field["width"], 1
            elif kind == "bytes":
                field_size, field_depth = field["bound"], 1
            elif kind == "record":
                child_size, child_depth = record_metrics(field["typeName"])
                field_size, field_depth = child_size, child_depth + 1
            elif field["typeName"]:
                child_size, child_depth = record_metrics(field["typeName"])
                field_size = 4 + field["bound"] * child_size
                field_depth = child_depth + 1
            else:
                field_size = 4 + field["bound"] * field["width"]
                field_depth = 1
            size += field_size
            depth = max(depth, field_depth)
            if size > contract.MAX_ENCODED_BYTES:
                _fail(f"{name}: encoded size exceeds bound")
            if depth > contract.MAX_DEPTH:
                _fail(f"{name}: schema depth exceeds bound")
        active.remove(name)
        sizes[name], depths[name] = size, depth
        return size, depth

    roots = [role["typeName"] for role in schema["roles"]]
    reachable: set[str] = set()

    def visit(name: str) -> None:
        if name in reachable:
            return
        reachable.add(name)
        for field in types[name]["fields"]:
            if field["typeName"]:
                visit(field["typeName"])

    for root in roots:
        visit(root)
    if reachable != set(types):
        unused = sorted(set(types) - reachable)
        _fail(f"unreachable type declarations: {', '.join(unused)}")
    maximum = 0
    for root in roots:
        size, _ = record_metrics(root)
        maximum = max(maximum, size)
    return maximum


def derive_type_tag(identity: bytes, contract: ModuleType = default_contract) -> int:
    digest = hashlib.sha256(contract.TAG_DOMAIN + identity).digest()
    return int.from_bytes(digest[:8], "little")


def compile_interface(
    path: Path,
    *,
    contract: ModuleType = default_contract,
    tag_deriver: Callable[[bytes], int] | None = None,
) -> CompiledInterface:
    schema = _normalize(_load(path.resolve(), contract), contract)
    maximum = _metrics(schema, contract)
    normalized = (
        json.dumps(schema, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"
    ).encode("utf-8")
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail(f"{path}: normalized schema exceeds bound")
    identity = hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest()
    type_tag = (tag_deriver or (lambda value: derive_type_tag(value, contract)))(identity)
    if not isinstance(type_tag, int) or not 0 < type_tag < 1 << 64:
        _fail(f"{path}: invalid derived type tag")
    return CompiledInterface(
        name=schema["name"],
        kind=schema["kind"],
        normalized=normalized,
        identity=identity,
        type_tag=type_tag,
        schema=schema,
        max_encoded_bytes=maximum,
    )


def admit_interfaces(
    paths: list[Path],
    *,
    contract: ModuleType = default_contract,
    tag_deriver: Callable[[bytes], int] | None = None,
) -> list[CompiledInterface]:
    if len(paths) > contract.MAX_SCHEMAS:
        _fail("admitted interface schema count exceeds bound")
    compiled = [
        compile_interface(path, contract=contract, tag_deriver=tag_deriver) for path in paths
    ]
    compiled.sort(key=lambda item: item.name)
    names = [item.name for item in compiled]
    if len(set(names)) != len(names):
        _fail("duplicate admitted interface name")
    module_names = [_snake(item.name) for item in compiled]
    if any(name in _RUST_KEYWORDS for name in module_names):
        _fail("interface name produces a reserved Rust module")
    if len(set(module_names)) != len(module_names):
        _fail("interface names collide in the Rust module namespace")
    for item in compiled:
        type_names = {record["name"] for record in item.schema["types"]}
        if item.name in type_names:
            _fail(f"{item.name}: contract alias collides with a record type")
    identities = [item.identity for item in compiled]
    if len(set(identities)) != len(identities):
        _fail("duplicate admitted interface identity")
    tags: dict[int, bytes] = {}
    for item in compiled:
        previous = tags.get(item.type_tag)
        if previous is not None and previous != item.identity:
            _fail(f"type-tag collision for {item.name}")
        tags[item.type_tag] = item.identity
    if _generated_rust_size(compiled) > contract.MAX_GENERATED_BYTES:
        _fail("generated Rust bindings exceed bound")
    return compiled


def _snake(name: str) -> str:
    output = []
    for index, character in enumerate(name):
        if character.isupper() and index and (
            name[index - 1].islower()
            or (index + 1 < len(name) and name[index + 1].islower())
        ):
            output.append("_")
        output.append(character.lower())
    return "".join(output)


def _rust_scalar(width: int, signed: bool) -> str:
    return f"{'i' if signed else 'u'}{width * 8}"


def _rust_field_type(field: dict) -> str:
    kind = field["kind"]
    if kind == "scalar":
        return _rust_scalar(field["width"], field["signed"])
    if kind == "bytes":
        return f"[u8; {field['bound']}]"
    if kind == "record":
        return field["typeName"]
    element = (
        field["typeName"]
        if field["typeName"]
        else _rust_scalar(field["width"], field["signed"])
    )
    return f"super::BoundedSequence<{element}, {field['bound']}>"


def _identity_literal(identity: bytes) -> str:
    return ", ".join(f"0x{byte:02x}" for byte in identity)


def _render_module(interface: CompiledInterface) -> str:
    schema = interface.schema
    lines = [f"pub mod {_snake(interface.name)} {{"]
    for item in schema["types"]:
        lines.extend(
            [
                "    #[derive(Debug, Clone, Copy, PartialEq, Eq)]",
                f"    pub struct {item['name']} {{",
            ]
        )
        for field in item["fields"]:
            lines.append(f"        pub {field['name']}: {_rust_field_type(field)},")
        lines.extend(["    }", ""])
    role_types = {role["role"]: role["typeName"] for role in schema["roles"]}
    if interface.kind == "stream":
        contract_type = f"super::Stream<{role_types['item']}>"
    elif interface.kind == "call":
        contract_type = f"super::Call<{role_types['request']}, {role_types['reply']}>"
    else:
        contract_type = (
            f"super::Operation<{role_types['goal']}, {role_types['feedback']}, "
            f"{role_types['result']}>"
        )
    lines.extend(
        [
            f"    pub type {interface.name} = {contract_type};",
            "",
            "    pub const INTERFACE_IDENTITY: [u8; 32] = [",
            f"        {_identity_literal(interface.identity)},",
            "    ];",
            f"    pub const TYPE_TAG: u64 = 0x{interface.type_tag:016x};",
            f"    pub const MAX_ENCODED_BYTES: usize = {interface.max_encoded_bytes};",
            "}",
            "",
        ]
    )
    return "\n".join(lines)


_RUST_HEADER = """// @generated by scripts/generate/generate-interface-schema-bindings.py; do not edit.
// Source contracts: contracts/interface-schema/v1/interfaces/*.zti

use core::marker::PhantomData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stream<T>(PhantomData<fn() -> T>);

impl<T> Stream<T> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<T> Default for Stream<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call<Request, Reply>(PhantomData<fn(Request) -> Reply>);

impl<Request, Reply> Call<Request, Reply> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Request, Reply> Default for Call<Request, Reply> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Operation<Goal, Feedback, Result>(PhantomData<fn(Goal, Feedback) -> Result>);

impl<Goal, Feedback, Result> Operation<Goal, Feedback, Result> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

impl<Goal, Feedback, Result> Default for Operation<Goal, Feedback, Result> {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BoundedSequence<T, const N: usize> {
    length: u32,
    values: [T; N],
}

impl<T, const N: usize> BoundedSequence<T, N> {
    pub fn new(length: usize, values: [T; N]) -> Option<Self> {
        if length > N || length > u32::MAX as usize {
            return None;
        }
        Some(Self {
            length: length as u32,
            values,
        })
    }

    pub const fn len(&self) -> usize {
        self.length as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub fn as_slice(&self) -> &[T] {
        &self.values[..self.len()]
    }
}

"""


def _generated_rust_size(interfaces: list[CompiledInterface]) -> int:
    return len(_RUST_HEADER.encode("utf-8")) + sum(
        len(_render_module(interface).encode("utf-8")) for interface in interfaces
    )


def render_rust(
    interfaces: list[CompiledInterface], contract: ModuleType = default_contract
) -> str:
    if _generated_rust_size(interfaces) > contract.MAX_GENERATED_BYTES:
        _fail("generated Rust bindings exceed bound")
    return _RUST_HEADER + "".join(_render_module(interface) for interface in interfaces)
