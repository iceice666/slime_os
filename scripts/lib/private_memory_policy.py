"""Private-memory v2 policy validation and schema-generated wire encoding."""

from __future__ import annotations

import copy
import hashlib

from boot_contracts import (
    PRIVATE_MEMORY_POLICY_MAGIC,
    PRIVATE_MEMORY_POLICY_VERSION,
    PRIVATE_MEMORY_POLICY_HEADER,
    PRIVATE_MEMORY_POLICY_HEADER_BYTES,
    PRIVATE_MEMORY_POLICY_ENTITLEMENT,
    PRIVATE_MEMORY_POLICY_SUBJECT,
    PRIVATE_MEMORY_POLICY_MAX_ENTITLEMENTS,
    PRIVATE_MEMORY_POLICY_MAX_SUBJECTS,
    PRIVATE_MEMORY_POLICY_FIXED,
    PRIVATE_MEMORY_POLICY_MAX_VALUE,
    PRIVATE_MEMORY_POLICY_MAX_PAGES,
    PRIVATE_MEMORY_POLICY_POOL,
)
MAX_INT = PRIVATE_MEMORY_POLICY_MAX_VALUE
MAX_PAGES = PRIVATE_MEMORY_POLICY_MAX_PAGES
RESOURCES = ("bytes", "slots", "descriptors", "extents", "tables")


def _record(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"private memory policy: {label}: expected {sorted(keys)}")
    return value


def _integer(value: object, maximum: int, label: str) -> int:
    if type(value) is not int or not 0 <= value <= maximum:
        raise ValueError(f"private memory policy: invalid {label}")
    return value


def _name(value: object) -> str:
    if not isinstance(value, str) or not value or len(value.encode()) > 96:
        raise ValueError("private memory policy: invalid identity name")
    return value


def _maximum(entry: dict) -> int:
    pages = _integer(entry["maximumPages"], MAX_PAGES, "maximumPages")
    mode = entry["maximumMode"]
    if mode == "fixed" and pages > 0:
        return pages
    if mode == "pool" and pages == 0:
        return MAX_PAGES
    raise ValueError("private memory policy: invalid maximum mode/value")


def identity(name: str, *, entitlement: bool = False) -> bytes:
    domain = b"slime-private-memory-entitlement-v2\0" if entitlement else b"slime-private-memory-subject-v2\0"
    return hashlib.sha256(domain + name.encode()).digest()


def validate(policy: object, instances: list[dict]) -> dict:
    policy = _record(policy, {"formatVersion", "reserve", "entitlements", "subjects"}, "policy")
    if type(policy["formatVersion"]) is not int or policy["formatVersion"] != PRIVATE_MEMORY_POLICY_VERSION:
        raise ValueError("private memory policy: unsupported version")
    reserve = _record(policy["reserve"], set(RESOURCES), "reserve")
    for field in RESOURCES:
        _integer(reserve[field], MAX_INT, field)
    entitlements = policy["entitlements"]
    subjects = policy["subjects"]
    if not isinstance(entitlements, list) or len(entitlements) > PRIVATE_MEMORY_POLICY_MAX_ENTITLEMENTS:
        raise ValueError("private memory policy: entitlement count")
    if not isinstance(subjects, list) or len(subjects) > PRIVATE_MEMORY_POLICY_MAX_SUBJECTS:
        raise ValueError("private memory policy: subject count")
    owners = {}
    for instance in instances:
        name = _name(instance["name"])
        if name in owners:
            raise ValueError("private memory policy: duplicate instance")
        owner = instance["owner"]
        # The generation's special root owner is not an instance identity.
        owners[name] = None if owner == "root" else owner
    for name in owners:
        seen = set()
        current = name
        while current is not None:
            if current not in owners or current in seen:
                raise ValueError("private memory policy: invalid ownership topology")
            seen.add(current)
            current = owners[current]
    by_name = {}
    guarantees = 0
    for entry in entitlements:
        _record(entry, {"name", "subtreeRoot", "guaranteePages", "maximumMode", "maximumPages"}, "entitlement")
        name = _name(entry["name"])
        if name in by_name:
            raise ValueError("private memory policy: duplicate entitlement")
        root = entry["subtreeRoot"]
        if not isinstance(root, str) or (root and root not in owners):
            raise ValueError("private memory policy: unknown subtree root")
        guarantee = _integer(entry["guaranteePages"], MAX_PAGES, "guaranteePages")
        if guarantee > _maximum(entry):
            raise ValueError("private memory policy: guarantee exceeds maximum")
        guarantees += guarantee
        if guarantees > MAX_PAGES:
            raise ValueError("private memory policy: guarantee overflow")
        by_name[name] = entry
    members = set()
    member_limits = {name: 0 for name in by_name}
    for entry in subjects:
        _record(entry, {"instance", "entitlement", "maximumMode", "maximumPages"}, "subject")
        name = _name(entry["instance"])
        if name not in owners or name in members:
            raise ValueError("private memory policy: unknown or duplicate subject")
        members.add(name)
        entitlement = by_name.get(_name(entry["entitlement"]))
        if entitlement is None:
            raise ValueError("private memory policy: unknown entitlement")
        member_limits[entry["entitlement"]] = min(MAX_PAGES, member_limits[entry["entitlement"]] + _maximum(entry))
        root = entitlement["subtreeRoot"]
        if root:
            current = name
            while current is not None and current != root:
                current = owners[current]
            if current != root:
                raise ValueError("private memory policy: subject outside declared subtree")
    for name, entry in by_name.items():
        if member_limits[name] == 0 or entry["guaranteePages"] > member_limits[name]:
            raise ValueError("private memory policy: unreachable entitlement guarantee")
    result = copy.deepcopy(policy)
    result["entitlements"].sort(key=lambda entry: entry["name"])
    result["subjects"].sort(key=lambda entry: entry["instance"])
    return result


def narrow(policy: dict, instances: list[dict]) -> dict:
    result = copy.deepcopy(policy)
    kept = {entry["name"] for entry in instances}
    result["subjects"] = [entry for entry in result["subjects"] if entry["instance"] in kept]
    used = {entry["entitlement"] for entry in result["subjects"]}
    result["entitlements"] = [entry for entry in result["entitlements"] if entry["name"] in used]
    # A shared guarantee is not divided or weakened by profile selection.
    return validate(result, instances)


def encode(policy: dict, instances: list[dict]) -> bytes:
    policy = validate(policy, instances)
    mode = {"fixed": PRIVATE_MEMORY_POLICY_FIXED, "pool": PRIVATE_MEMORY_POLICY_POOL}
    entitlements = sorted(
        (identity(entry["name"], entitlement=True),
         identity(entry["subtreeRoot"]) if entry["subtreeRoot"] else bytes(32),
         entry["guaranteePages"], entry["maximumPages"], mode[entry["maximumMode"]], 0)
        for entry in policy["entitlements"]
    )
    subjects = sorted(
        (identity(entry["instance"]), identity(entry["entitlement"], entitlement=True),
         entry["maximumPages"], mode[entry["maximumMode"]], 0)
        for entry in policy["subjects"]
    )
    body = b"".join(PRIVATE_MEMORY_POLICY_ENTITLEMENT.pack(*entry) for entry in entitlements)
    body += b"".join(PRIVATE_MEMORY_POLICY_SUBJECT.pack(*entry) for entry in subjects)
    return PRIVATE_MEMORY_POLICY_HEADER.pack(
        PRIVATE_MEMORY_POLICY_MAGIC, PRIVATE_MEMORY_POLICY_VERSION,
        PRIVATE_MEMORY_POLICY_HEADER_BYTES, 0, len(entitlements), len(subjects),
        PRIVATE_MEMORY_POLICY_HEADER_BYTES + len(body), 0,
        *(policy["reserve"][field] for field in RESOURCES),
    ) + body


def manifest_policy(manifest: dict) -> dict | None:
    """Refuse dual authority and resource/declaration mismatches before building."""
    policy = manifest.get("privateMemoryPolicy")
    objects = [entry for entry in manifest["objects"] if entry["id"] in ("private-memory-budget", "private-memory-policy")]
    if len(objects) > 1 or any(entry["kind"] != "resource" for entry in objects):
        raise ValueError("private memory policy: conflicting resource objects")
    if policy is None:
        if objects and objects[0]["id"] == "private-memory-policy":
            raise ValueError("private memory policy: resource without declaration")
        return None
    if "privateMemoryBudget" in manifest:
        raise ValueError("private memory policy: conflicting fixed declaration")
    if not objects or objects[0]["id"] != "private-memory-policy":
        raise ValueError("private memory policy: declaration without resource")
    return validate(policy, manifest["instances"])
