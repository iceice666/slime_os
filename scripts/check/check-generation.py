#!/usr/bin/env python3

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import struct
import subprocess
import sys
from pathlib import Path

from boot_contracts import *
from harness import ROOT
from release_trust import authority_manifest_identity, initial_public_keys, ssh_signed_payload

SEL4_EXTERNAL_KERNEL = b"SLIME-SEL4-KERNEL-EXTERNAL\0"

class CheckError(ValueError):
    pass


def require(condition: bool, message: str) -> None:
    if not condition:
        raise CheckError(message)


def read_string(data: bytes, base: int, table_len: int, offset: int) -> str:
    require(0 <= offset < table_len, "BadStringOffset")
    length = int.from_bytes(data[base + offset : base + offset + 2], "little")
    require(length <= MAX_STRING_BYTES and offset + 2 + length <= table_len, "OversizedString")
    try:
        return data[base + offset + 2 : base + offset + 2 + length].decode("utf-8")
    except UnicodeDecodeError as error:
        raise CheckError("BadUtf8") from error


def check_kernel_image(blob: bytes, profile) -> None:
    require(len(blob) >= KERNEL_HEADER.size, "TruncatedKernelImage")
    (
        magic,
        version,
        header,
        abi,
        flags,
        architecture,
        image_abi,
        page_profile,
        target_profile,
        required_features,
        preferred,
        entry,
        segments,
        relocations,
        payload,
        total,
    ) = KERNEL_HEADER.unpack_from(blob)
    require(magic == KERNEL_MAGIC, "BadKernelMagic")
    require(version == KERNEL_VERSION and header == KERNEL_HEADER.size and abi == KERNEL_ABI_VERSION, "BadKernelVersion")
    require(flags == 0 and total == len(blob), "BadKernelHeader")
    # The authenticated generation, kernel, and every component name the same
    # exact profile id. This also separates profiles that share an ISA and ABI,
    # such as AArch64 QEMU and Raspberry Pi 5.
    require(target_profile == profile.id, "KernelTargetProfileMismatch")
    require(architecture == profile.architecture, "KernelArchitectureMismatch")
    require(image_abi == profile.abi, "KernelAbiMismatch")
    require(page_profile == profile.page_profile, "KernelPageProfileMismatch")
    require(required_features == profile.required_features, "KernelFeatureMismatch")
    require(preferred == profile.kernel_preferred_base, "KernelLoadLayoutMismatch")
    require(1 <= segments <= MAX_KERNEL_SEGMENTS and relocations <= MAX_KERNEL_RELOCATIONS, "ExcessiveKernelCount")
    require(payload == KERNEL_HEADER.size + segments * KERNEL_SEGMENT.size + relocations * KERNEL_RELOCATION.size, "BadKernelBounds")
    require(len(blob) <= MAX_KERNEL_IMAGE_BYTES, "KernelImageTooLarge")
    previous = 0
    writable: list[tuple[int, int]] = []
    entry_ok = False
    image_end = 0
    for index in range(segments):
        vaddr, mem_len, file_offset, file_len, segment_flags, reserved = KERNEL_SEGMENT.unpack_from(blob, KERNEL_HEADER.size + index * KERNEL_SEGMENT.size)
        require(reserved == 0 and vaddr % 4096 == 0 and mem_len > 0 and file_len <= mem_len, "BadKernelSegment")
        require(vaddr >= previous and not segment_flags & ~(SEGMENT_WRITE | SEGMENT_EXEC) and segment_flags != SEGMENT_WRITE | SEGMENT_EXEC, "BadKernelSegment")
        require(payload <= file_offset <= file_offset + file_len <= len(blob), "BadKernelPayload")
        previous = vaddr + mem_len
        image_end = max(image_end, previous)
        if segment_flags & SEGMENT_WRITE:
            writable.append((vaddr, vaddr + mem_len))
        entry_ok |= bool(segment_flags & SEGMENT_EXEC and vaddr <= entry < vaddr + mem_len)
    require(entry_ok and image_end <= MAX_KERNEL_IMAGE_BYTES, "BadKernelEntry")
    relocation_start = KERNEL_HEADER.size + segments * KERNEL_SEGMENT.size
    for index in range(relocations):
        target, addend = KERNEL_RELOCATION.unpack_from(blob, relocation_start + index * KERNEL_RELOCATION.size)
        require(target % 8 == 0 and any(start <= target and target + 8 <= end for start, end in writable), "BadRelocation")
        absolute_addend = addend if addend >= 0 else (1 << 64) + addend
        require(profile.kernel_preferred_base <= absolute_addend <= profile.kernel_preferred_base + ((image_end + profile.page_bytes - 1) & -profile.page_bytes), "BadRelocationAddend")


def check_component_image(blob: bytes, profile, name: str) -> None:
    require(len(blob) >= COMPONENT_IMAGE_HEADER.size, f"TruncatedComponentImage:{name}")
    (
        magic,
        version,
        header,
        abi,
        architecture,
        image_abi,
        page_profile,
        _entry,
        _segments,
        reserved,
        _stack,
        target_profile,
        required_features,
    ) = COMPONENT_IMAGE_HEADER.unpack_from(blob)
    require(
        magic in (COMPONENT_IMAGE_MAGIC, COMPONENT_IMAGE_ELF_MAGIC),
        f"BadComponentMagic:{name}",
    )
    require(
        version == COMPONENT_IMAGE_VERSION
        and header == COMPONENT_IMAGE_HEADER.size
        and abi == COMPONENT_IMAGE_KERNEL_ABI,
        f"BadComponentVersion:{name}",
    )
    require(reserved == 0, f"BadComponentHeader:{name}")
    require(target_profile == profile.id, f"ComponentTargetProfileMismatch:{name}")
    require(architecture == profile.architecture, f"ComponentArchitectureMismatch:{name}")
    require(image_abi == profile.abi, f"ComponentAbiMismatch:{name}")
    require(page_profile == profile.page_profile, f"ComponentPageProfileMismatch:{name}")
    require(required_features == profile.required_features, f"ComponentFeatureMismatch:{name}")



RIGHT_TRANSFER = 1 << 2
RIGHT_EXEC = 1 << 3
RIGHT_SPAWN = 1 << 16
RIGHT_ALL = (1 << 26) - 1
CAPABILITY_ENDPOINT = 1
CAPABILITY_EXECUTABLE = 2
CAPABILITY_SHARED_BUFFER_FACTORY = 3
CAPABILITY_BLOCK = 4
CAPABILITY_DIRECTORY = 5
CAPABILITY_INPUT = 6
CAPABILITY_SUPERVISION = 7
CAPABILITY_SHARED_BUFFER = 8
CAPABILITY_LOAN = 9


def capability_rights_valid(kind: int, rights: int) -> bool:
    allowed = {
        CAPABILITY_ENDPOINT: 0b11 | RIGHT_TRANSFER,
        CAPABILITY_EXECUTABLE: RIGHT_EXEC | RIGHT_SPAWN | RIGHT_TRANSFER,
        CAPABILITY_SHARED_BUFFER_FACTORY: (1 << 24) | RIGHT_TRANSFER,
        CAPABILITY_BLOCK: (1 << 10) | (1 << 11),
        CAPABILITY_DIRECTORY: (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22) | RIGHT_TRANSFER,
        CAPABILITY_INPUT: 1 << 23,
        CAPABILITY_SUPERVISION: (1 << 18) | RIGHT_TRANSFER,
        CAPABILITY_SHARED_BUFFER: (1 << 8) | (1 << 9) | (1 << 25) | RIGHT_TRANSFER,
        CAPABILITY_LOAN: (1 << 8) | (1 << 9) | RIGHT_TRANSFER,
    }.get(kind)
    required = {
        CAPABILITY_ENDPOINT: 0b11,
        CAPABILITY_EXECUTABLE: RIGHT_EXEC | RIGHT_SPAWN,
        CAPABILITY_SHARED_BUFFER_FACTORY: 1 << 24,
        CAPABILITY_BLOCK: (1 << 10) | (1 << 11),
        CAPABILITY_DIRECTORY: (1 << 19) | (1 << 20) | (1 << 21) | (1 << 22),
        CAPABILITY_INPUT: 1 << 23,
        CAPABILITY_SUPERVISION: 1 << 18,
        CAPABILITY_SHARED_BUFFER: (1 << 8) | (1 << 9) | (1 << 25),
        CAPABILITY_LOAN: 1 << 9,
    }.get(kind, 0)
    return (
        allowed is not None
        and rights != 0
        and not rights & ~allowed
        and bool(rights & required)
        and (kind != CAPABILITY_EXECUTABLE or rights & (RIGHT_EXEC | RIGHT_SPAWN) == RIGHT_EXEC | RIGHT_SPAWN)
        and (kind != CAPABILITY_INPUT or rights == 1 << 23)
    )
MAX_SPAWN_BUDGET = 32
PLAN_NONE = 0xFFFFFFFF
SERVICE_LIFECYCLE = 1
SERVICE_SPAWN = 2
SERVICE_SUPERVISION = 3
SERVICE_CAPABILITY_TRANSFER = 4
SERVICE_SHARED_BUFFER = 5
SERVICE_DIRECTORY = 6
SERVICE_INPUT = 7
SERVICE_BLOCK = 8
SERVICE_CONSOLE = 9
ROOT_SERVICE_SLOT = 1
CONSOLE_SERVICE_SLOT = 32
SERVICE_BY_CAPABILITY_KIND = {
    CAPABILITY_SHARED_BUFFER_FACTORY: SERVICE_SHARED_BUFFER,
    CAPABILITY_SHARED_BUFFER: SERVICE_SHARED_BUFFER,
    CAPABILITY_LOAN: SERVICE_SHARED_BUFFER,
    CAPABILITY_DIRECTORY: SERVICE_DIRECTORY,
    CAPABILITY_INPUT: SERVICE_INPUT,
    CAPABILITY_BLOCK: SERVICE_BLOCK,
    CAPABILITY_SUPERVISION: SERVICE_SUPERVISION,
}
SHARED_BUFFER_BUDGET_MAGIC = b"SLIMESB\0"
SHARED_BUFFER_BUDGET_HEADER = struct.Struct("<8sIIQII")
SHARED_BUFFER_BUDGET_ENTRY = struct.Struct("<32sIIII")


def shared_buffer_holders(object_rows: list[tuple[str, int, bytes]]) -> set[bytes]:
    resources = [
        blob
        for _object_id, kind, blob in object_rows
        if kind == 4 and blob[:8] == SHARED_BUFFER_BUDGET_MAGIC
    ]
    require(len(resources) <= 1, "BadSharedBufferBudget")
    if not resources:
        return set()
    blob = resources[0]
    require(len(blob) >= SHARED_BUFFER_BUDGET_HEADER.size, "BadSharedBufferBudget")
    magic, version, header, required_flags, count, total_len = SHARED_BUFFER_BUDGET_HEADER.unpack_from(blob)
    require(
        magic == SHARED_BUFFER_BUDGET_MAGIC
        and version == 1
        and header == SHARED_BUFFER_BUDGET_HEADER.size
        and required_flags == 0
        and total_len == len(blob)
        and total_len == header + count * SHARED_BUFFER_BUDGET_ENTRY.size,
        "BadSharedBufferBudget",
    )
    holders: set[bytes] = set()
    previous = bytes(32)
    for index in range(count):
        identity, _pages, _buffers, _mappings, _loans = SHARED_BUFFER_BUDGET_ENTRY.unpack_from(
            blob, header + index * SHARED_BUFFER_BUDGET_ENTRY.size
        )
        require(
            identity != bytes(32) and (index == 0 or identity > previous),
            "BadSharedBufferBudget",
        )
        holders.add(identity)
        previous = identity
    return holders
GRANT_POLICY_ONLY = 1
GRANT_MINTED = 1
BOOT_ACTIONS = {
    "product", "boot", "call", "channel", "crossing", "dango", "directory",
    "filesystem", "generation", "input", "loan", "operation", "powerbox",
    "qos", "reclamation", "recovery", "rollback", "sample", "spawn",
    "storage", "store", "stream", "supervision", "transfer", "visibility",
}


def check_generation(data: bytes, expected_identity: bytes | None = None) -> dict:
    require(len(data) >= GENERATION_HEADER.size and len(data) <= MAX_GENERATION_BYTES, "TruncatedGeneration")
    fields = GENERATION_HEADER.unpack_from(data)
    (
        magic, version, header, required_flags, identity, number, parent,
        target_offset, boot_action_offset, bootstrap, boot_attempts,
        objects, executables, instances, dependencies, bindings, grants, states, health,
        processes, threads, kernel_objects, mappings, cap_bindings, service_bindings,
        schedules, fault_policies, spawn_templates, resource_quotas,
        minted_bindings, notification_grants, notification_bindings, header_reserved,
        object_offset, executable_offset, instance_offset, dependency_offset, binding_offset,
        grant_offset, state_offset, health_offset, process_offset, thread_offset,
        kernel_object_offset, mapping_offset, cap_binding_offset, service_binding_offset,
        schedule_offset, fault_policy_offset, spawn_template_offset, resource_quota_offset,
        minted_binding_offset, notification_grant_offset, notification_binding_offset,
        strings_offset, strings_len, payload_offset, total_len,
    ) = fields
    require(magic == GENERATION_MAGIC, "BadGenerationMagic")
    require(version == GENERATION_VERSION and header == GENERATION_HEADER.size, "UnsupportedGenerationVersion")
    require(required_flags == 0 and header_reserved == 0, "UnknownGenerationFlags")
    require(not any(data[400 : GENERATION_HEADER.size]), "UnknownGenerationFlags")
    require(total_len == len(data) and generation_identity(data) == identity, "BadGenerationHash")
    if expected_identity is not None:
        require(identity == expected_identity, "GenerationIdentityMismatch")
    require(1 <= objects <= MAX_OBJECTS and 1 <= executables <= MAX_EXECUTABLES and 1 <= instances <= MAX_INSTANCES, "ExcessiveGenerationCount")
    require(dependencies <= MAX_DEPENDENCIES and bindings <= MAX_BINDINGS and grants <= MAX_GRANTS and states <= MAX_STATES and health <= MAX_HEALTH_INSTANCES, "ExcessiveGenerationCount")
    require(1 <= processes <= MAX_PROCESSES and 1 <= threads <= MAX_THREADS and 1 <= kernel_objects <= MAX_KERNEL_OBJECTS, "ExcessiveGenerationCount")
    require(mappings <= MAX_MAPPINGS and 1 <= cap_bindings <= MAX_CAP_BINDINGS and 1 <= service_bindings <= MAX_SERVICE_BINDINGS, "ExcessiveGenerationCount")
    require(1 <= schedules <= MAX_SCHEDULES and 1 <= fault_policies <= MAX_FAULT_POLICIES and spawn_templates <= MAX_SPAWN_TEMPLATES and 1 <= resource_quotas <= MAX_RESOURCE_QUOTAS, "ExcessiveGenerationCount")
    require(strings_len <= MAX_STRING_TABLE_BYTES and target_offset < strings_len and boot_action_offset < strings_len, "BadStringTable")
    require(object_offset == GENERATION_HEADER.size, "BadGenerationBounds")
    require(executable_offset == object_offset + objects * GENERATION_OBJECT.size, "BadGenerationBounds")
    require(instance_offset == executable_offset + executables * GENERATION_EXECUTABLE.size, "BadGenerationBounds")
    require(dependency_offset == instance_offset + instances * GENERATION_INSTANCE.size, "BadGenerationBounds")
    require(binding_offset == dependency_offset + dependencies * GENERATION_DEPENDENCY.size, "BadGenerationBounds")
    require(grant_offset == binding_offset + bindings * GENERATION_BINDING.size, "BadGenerationBounds")
    require(state_offset == grant_offset + grants * GENERATION_GRANT.size, "BadGenerationBounds")
    require(health_offset == state_offset + states * GENERATION_STATE.size, "BadGenerationBounds")
    require(process_offset == health_offset + health * GENERATION_HEALTH.size, "BadGenerationBounds")
    require(thread_offset == process_offset + processes * GENERATION_PROCESS.size, "BadGenerationBounds")
    require(kernel_object_offset == thread_offset + threads * GENERATION_THREAD.size, "BadGenerationBounds")
    require(mapping_offset == kernel_object_offset + kernel_objects * GENERATION_KERNEL_OBJECT.size, "BadGenerationBounds")
    require(cap_binding_offset == mapping_offset + mappings * GENERATION_MAPPING.size, "BadGenerationBounds")
    require(service_binding_offset == cap_binding_offset + cap_bindings * GENERATION_CAP_BINDING.size, "BadGenerationBounds")
    require(schedule_offset == service_binding_offset + service_bindings * GENERATION_SERVICE_BINDING.size, "BadGenerationBounds")
    require(fault_policy_offset == schedule_offset + schedules * GENERATION_SCHEDULE.size, "BadGenerationBounds")
    require(spawn_template_offset == fault_policy_offset + fault_policies * GENERATION_FAULT_POLICY.size, "BadGenerationBounds")
    require(resource_quota_offset == spawn_template_offset + spawn_templates * GENERATION_SPAWN_TEMPLATE.size, "BadGenerationBounds")
    require(minted_bindings <= MAX_MINTED_BINDINGS and notification_grants <= MAX_NOTIFICATION_GRANTS and notification_bindings <= MAX_NOTIFICATION_BINDINGS, "ExcessiveGenerationCount")
    require(minted_binding_offset == resource_quota_offset + resource_quotas * GENERATION_RESOURCE_QUOTA.size, "BadGenerationBounds")
    require(notification_grant_offset == minted_binding_offset + minted_bindings * GENERATION_MINTED_BINDING.size, "BadGenerationBounds")
    require(notification_binding_offset == notification_grant_offset + notification_grants * GENERATION_NOTIFICATION_GRANT.size, "BadGenerationBounds")
    require(strings_offset == notification_binding_offset + notification_bindings * GENERATION_NOTIFICATION_BINDING.size, "BadGenerationBounds")
    require(payload_offset == strings_offset + strings_len, "BadGenerationBounds")
    target = read_string(data, strings_offset, strings_len, target_offset)
    profile = TARGET_PROFILES_BY_NAME.get(target)
    require(profile is not None, "UnknownGenerationTarget")
    boot_action = read_string(data, strings_offset, strings_len, boot_action_offset)
    require(boot_action in BOOT_ACTIONS, "UnknownBootAction")
    object_rows = []
    previous_id = ""
    previous_payload = payload_offset
    for index in range(objects):
        id_offset, kind, offset, length, digest = GENERATION_OBJECT.unpack_from(data, object_offset + index * GENERATION_OBJECT.size)
        object_id = read_string(data, strings_offset, strings_len, id_offset)
        require(object_id > previous_id, "NonCanonicalObjects")
        require(kind in (1, 2, 3, 4) and length <= MAX_OBJECT_PAYLOAD_BYTES, "BadObject")
        require(offset == previous_payload and offset + length <= len(data), "BadObjectBounds")
        blob = data[offset : offset + length]
        require(sha256(blob) == digest, "BadObjectHash")
        object_rows.append((object_id, kind, blob))
        previous_id, previous_payload = object_id, offset + length
    require(previous_payload == len(data), "TrailingGenerationBytes")
    executable_rows = []
    previous_name = ""
    for index in range(executables):
        name_offset, object_index, role, spawn_budget = GENERATION_EXECUTABLE.unpack_from(data, executable_offset + index * GENERATION_EXECUTABLE.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(name > previous_name and object_index < objects and 1 <= role <= 4, "BadExecutable")
        require(object_rows[object_index][1] in (2, 3) and 0 <= spawn_budget <= MAX_SPAWN_BUDGET, "BadExecutable")
        check_component_image(object_rows[object_index][2], profile, name)
        executable_rows.append((name, object_index, role, spawn_budget))
        previous_name = name
    instance_rows = []
    previous_name = ""
    for index in range(instances):
        row = GENERATION_INSTANCE.unpack_from(data, instance_offset + index * GENERATION_INSTANCE.size)
        name = read_string(data, strings_offset, strings_len, row[0])
        _, executable, owner_kind, owner_index, autostart, dependency_start, dependency_count, binding_start, binding_count, required = row
        require(name > previous_name and executable < executables, "BadInstance")
        require(owner_kind in (0, 1) and (owner_kind == 0 or owner_index < instances) and owner_index != index, "BadInstanceOwner")
        require(autostart in (0, 1) and required in (0, 1), "BadInstance")
        require(dependency_start + dependency_count <= dependencies and binding_start + binding_count <= bindings, "BadInstanceBounds")
        instance_rows.append((name, executable, owner_kind, owner_index, autostart, dependency_start, dependency_count, binding_start, binding_count, required))
        previous_name = name
    require(bootstrap < instances, "BadBootstrap")
    bootstrap_row = instance_rows[bootstrap]
    bootstrap_executable = executable_rows[bootstrap_row[1]]
    require(bootstrap_row[2] == 0 and bootstrap_row[4] == 1 and bootstrap_executable[2] == 1 and object_rows[bootstrap_executable[1]][1] == 2, "BadBootstrap")
    dependency_rows = [GENERATION_DEPENDENCY.unpack_from(data, dependency_offset + index * GENERATION_DEPENDENCY.size)[0] for index in range(dependencies)]
    binding_rows = [GENERATION_BINDING.unpack_from(data, binding_offset + index * GENERATION_BINDING.size) for index in range(bindings)]
    for index, row in enumerate(instance_rows):
        previous_dependency = -1
        for dependency in dependency_rows[row[5] : row[5] + row[6]]:
            require(dependency < instances and dependency != index and dependency > previous_dependency, "BadDependency")
            previous_dependency = dependency
        previous_slot = -1
        for grant, slot in binding_rows[row[7] : row[7] + row[8]]:
            require(grant < grants and slot < 64 and slot > previous_slot, "BadBinding")
            previous_slot = slot
        require(
            len({grant for grant, _slot in binding_rows[row[7] : row[7] + row[8]]})
            == row[8],
            "BadBinding",
        )
    grant_rows = []
    previous_grant = None
    for index in range(grants):
        name_offset, source, destination, rights, transferable, grant_flags, capability_kind = GENERATION_GRANT.unpack_from(data, grant_offset + index * GENERATION_GRANT.size)
        require(grant_flags & ~GRANT_MINTED == 0, "UnknownGrantFlags")
        name = read_string(data, strings_offset, strings_len, name_offset)
        key = (name, source, destination)
        require(previous_grant is None or key > previous_grant, "NonCanonicalGrants")
        require(source < instances and rights and not rights & ~RIGHT_ALL and transferable in (0, 1) and bool(rights & RIGHT_TRANSFER) == bool(transferable), "BadGrant")
        require(capability_rights_valid(capability_kind, rights), "BadGrantKind")
        require(destination < (executables if capability_kind == CAPABILITY_EXECUTABLE else instances), "BadGrant")
        grant_rows.append((name, source, destination, rights, bool(grant_flags & GRANT_MINTED), capability_kind))
        previous_grant = key
    previous_state = ""
    for index in range(states):
        name_offset, owner, schema_version, policy = GENERATION_STATE.unpack_from(data, state_offset + index * GENERATION_STATE.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(name > previous_state and owner < instances and schema_version > 0 and policy in (1, 2, 3, 4, 5), "BadState")
        previous_state = name
    require(boot_attempts > 0, "BadHealthPolicy")
    health_rows = [GENERATION_HEALTH.unpack_from(data, health_offset + index * GENERATION_HEALTH.size)[0] for index in range(health)]
    require(all(instance < instances for instance in health_rows) and health_rows == sorted(set(health_rows)), "BadHealthInstance")
    require(set(health_rows) == {index for index, row in enumerate(instance_rows) if row[9]}, "BadHealthPolicy")
    require(processes == instances == threads == schedules == fault_policies == resource_quotas, "BadPlanShape")
    process_rows = []
    seen_instances = set()
    for index in range(processes):
        name_offset, instance, cspace_object, vspace_object, main_thread, quota, flags = GENERATION_PROCESS.unpack_from(data, process_offset + index * GENERATION_PROCESS.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(instance < instances and instance not in seen_instances, "BadProcess")
        require(cspace_object < kernel_objects and vspace_object < kernel_objects and main_thread < threads and quota < resource_quotas and flags == 0, "BadProcess")
        require(name == instance_rows[instance][0], "BadProcess")
        seen_instances.add(instance)
        process_rows.append({"instance": instance, "cspace_object": cspace_object, "vspace_object": vspace_object, "main_thread": main_thread, "quota": quota})
    require(len(seen_instances) == instances, "BadPlanShape")
    kernel_object_rows = []
    for index in range(kernel_objects):
        name_offset, kind, owner_process, size_bits, count, source_object, flags = GENERATION_KERNEL_OBJECT.unpack_from(data, kernel_object_offset + index * GENERATION_KERNEL_OBJECT.size)
        require(owner_process < processes and 1 <= kind <= 6 and count > 0 and flags == 0, "BadKernelObject")
        require(source_object == PLAN_NONE or source_object < objects, "BadKernelObject")
        kernel_object_rows.append({"kind": kind, "size_bits": size_bits})
    for process in process_rows:
        require(kernel_object_rows[process["cspace_object"]]["kind"] == 1, "BadKernel")
        # The root sizes the real CNode from this, so a zero-bit CSpace would
        # leave nowhere to install even the null slot.
        require(kernel_object_rows[process["cspace_object"]]["size_bits"] > 0, "BadKernel")
        require(kernel_object_rows[process["vspace_object"]]["kind"] == 2, "BadKernel")
    thread_rows = []
    for index in range(threads):
        name_offset, process, tcb_object, schedule, fault_policy, ipc_buffer_object, ipc_buffer_vaddr, entry, flags = GENERATION_THREAD.unpack_from(data, thread_offset + index * GENERATION_THREAD.size)
        require(process < processes and tcb_object < kernel_objects and schedule < schedules and fault_policy < fault_policies and ipc_buffer_object < kernel_objects and flags == 0, "BadThread")
        require(kernel_object_rows[tcb_object]["kind"] == 3 and kernel_object_rows[ipc_buffer_object]["kind"] == 4, "BadKernel")
        thread_rows.append({"process": process, "schedule": schedule, "fault_policy": fault_policy})
    for index, process in enumerate(process_rows):
        require(thread_rows[process["main_thread"]]["process"] == index, "BadProcess")
    for index in range(mappings):
        process, obj, vaddr, page_count, rights, attributes, source_object, flags = GENERATION_MAPPING.unpack_from(data, mapping_offset + index * GENERATION_MAPPING.size)
        require(process < processes and obj < kernel_objects and page_count > 0 and rights and not rights & ~RIGHT_ALL and flags == 0, "BadMapping")
        require(source_object == PLAN_NONE or source_object < objects, "BadMapping")
    materialized = [0] * grants
    policy_only_grants = set()
    for index in range(cap_bindings):
        process, slot, obj, rights, badge, grant, flags = GENERATION_CAP_BINDING.unpack_from(data, cap_binding_offset + index * GENERATION_CAP_BINDING.size)
        # Every declared slot must be addressable in the CNode the plan sized
        # for that process: the root installs at exactly this slot, so one
        # outside the CSpace has no destination.
        cspace_bits = kernel_object_rows[process_rows[process]["cspace_object"]]["size_bits"] if process < processes else 0
        require(process < processes and slot < (1 << cspace_bits) and obj < kernel_objects and rights and flags & ~GRANT_POLICY_ONLY == 0, "BadCapBinding")
        require(grant == PLAN_NONE or grant < grants, "BadCapBinding")
        if grant != PLAN_NONE:
            if flags & GRANT_POLICY_ONLY == 0:
                materialized[grant] += 1
            else:
                policy_only_grants.add(grant)
            require(rights == grant_rows[grant][3], "BadCapBinding")
    # The root reads a process's own TCB and fault slots out of these bindings
    # and refuses a plan naming either twice, so the twin refuses it here too.
    own_slots = {}
    for index in range(cap_bindings):
        process, slot, obj, rights, badge, grant, flags = GENERATION_CAP_BINDING.unpack_from(data, cap_binding_offset + index * GENERATION_CAP_BINDING.size)
        if grant != PLAN_NONE:
            continue
        kind = kernel_object_rows[obj]["kind"]
        if kind not in (3, 5):
            continue
        key = (process, kind)
        require(key not in own_slots, "BadCapBinding")
        own_slots[key] = slot
    known_services = set(range(SERVICE_LIFECYCLE, SERVICE_CONSOLE + 1))
    seen_services = [set() for _ in range(processes)]
    for index in range(service_bindings):
        process, service, slot, obj, rights, badge, flags = GENERATION_SERVICE_BINDING.unpack_from(data, service_binding_offset + index * GENERATION_SERVICE_BINDING.size)
        expected_slot = CONSOLE_SERVICE_SLOT if service == SERVICE_CONSOLE else ROOT_SERVICE_SLOT
        require(process < processes and service in known_services, "BadServiceBinding")
        require(service not in seen_services[process], "BadServiceBinding")
        require(slot == expected_slot and obj < kernel_objects and kernel_object_rows[obj]["kind"] == 5, "BadServiceBinding")
        require(rights == 1 and badge != 0 and flags == 0, "BadServiceBinding")
        seen_services[process].add(service)
    budgeted_holders = shared_buffer_holders(object_rows)
    for process_index, process in enumerate(process_rows):
        instance = instance_rows[process["instance"]]
        executable = executable_rows[instance[1]]
        required_services = {SERVICE_LIFECYCLE, SERVICE_CONSOLE}
        if executable[2] == 1 or executable[3] != 0:
            required_services.update(
                {SERVICE_SPAWN, SERVICE_SUPERVISION, SERVICE_CAPABILITY_TRANSFER}
            )
        instance_name = instance[0].encode("utf-8")
        holder_identity = sha256(
            b"slime-shared-buffer-holder-v1"
            + struct.pack("<H", len(instance_name))
            + instance_name
        )
        if holder_identity in budgeted_holders:
            required_services.add(SERVICE_SHARED_BUFFER)
        for grant_index, _slot in binding_rows[instance[7] : instance[7] + instance[8]]:
            grant = grant_rows[grant_index]
            service = SERVICE_BY_CAPABILITY_KIND.get(grant[5])
            if service is not None:
                required_services.add(service)
            if grant[5] == CAPABILITY_EXECUTABLE:
                required_services.add(SERVICE_SPAWN)
            if grant[5] == CAPABILITY_ENDPOINT or grant[3] & RIGHT_TRANSFER:
                required_services.add(SERVICE_CAPABILITY_TRANSFER)
        for minted_index in range(minted_bindings):
            record = minted_binding_offset + minted_index * GENERATION_MINTED_BINDING.size
            _name, _owner, holder, _slot, rights, _flags, capability_kind = GENERATION_MINTED_BINDING.unpack_from(data, record)
            if holder != process["instance"]:
                continue
            service = SERVICE_BY_CAPABILITY_KIND.get(capability_kind)
            if service is not None:
                required_services.add(service)
            if capability_kind == CAPABILITY_EXECUTABLE:
                required_services.add(SERVICE_SPAWN)
            if capability_kind == CAPABILITY_ENDPOINT or rights & RIGHT_TRANSFER:
                required_services.add(SERVICE_CAPABILITY_TRANSFER)
        require(seen_services[process_index] == required_services, "BadServiceBinding")

    for process_index in range(processes):
        # Both are required: the root has nowhere to put the child's TCB or its
        # fault endpoint otherwise, and refuses the instance at construction.
        require((process_index, 3) in own_slots, "BadCapBinding")
        require((process_index, 5) in own_slots, "BadCapBinding")
        require(own_slots[(process_index, 3)] != own_slots[(process_index, 5)], "BadCapBinding")

    for index, grant_row in enumerate(grant_rows):
        # A minted grant's object does not exist at admission, so the plan
        # carries no capability for it; its two minted bindings state where
        # each end lands instead.
        if grant_row[4]:
            require(materialized[index] == 0, "UnmaterializedGrant")
            continue
        policy_only = index in policy_only_grants
        require(materialized[index] + int(policy_only) == 1, "UnmaterializedGrant")
        if policy_only:
            require(grant_row[5] not in (CAPABILITY_EXECUTABLE, CAPABILITY_ENDPOINT), "BadCapBinding")
    for index in range(schedules):
        name_offset, thread, authority_process, priority, max_controlled_priority, budget_us, period_us, flags = GENERATION_SCHEDULE.unpack_from(data, schedule_offset + index * GENERATION_SCHEDULE.size)
        require(thread < threads and (authority_process == PLAN_NONE or authority_process < processes) and priority <= max_controlled_priority and flags == 0, "BadSchedule")
        require(thread_rows[thread]["schedule"] == index, "BadSchedule")
    for index in range(fault_policies):
        name_offset, thread, handler_process, endpoint_object, badge, action = GENERATION_FAULT_POLICY.unpack_from(data, fault_policy_offset + index * GENERATION_FAULT_POLICY.size)
        require(thread < threads and (handler_process == PLAN_NONE or handler_process < processes) and endpoint_object < kernel_objects and action != 0, "BadFaultPolicy")
        require(thread_rows[thread]["fault_policy"] == index, "BadFaultPolicy")
    for index in range(spawn_templates):
        name_offset, executable, owner_process, quota, schedule, fault_policy, max_instances, flags = GENERATION_SPAWN_TEMPLATE.unpack_from(data, spawn_template_offset + index * GENERATION_SPAWN_TEMPLATE.size)
        require(executable < executables and owner_process < processes and quota < resource_quotas and schedule < schedules and fault_policy < fault_policies and max_instances > 0 and flags == 0, "BadSpawnTemplate")
    for index in range(resource_quotas):
        name_offset, owner_process, cnode_count, tcb_count, endpoint_count, notification_count, frame_count, page_table_count, mapping_count, irq_count, cslot_count, untyped_bytes, dynamic_reserve_bytes, flags = GENERATION_RESOURCE_QUOTA.unpack_from(data, resource_quota_offset + index * GENERATION_RESOURCE_QUOTA.size)
        require(owner_process < processes and cnode_count > 0 and tcb_count > 0 and cslot_count > 0 and flags == 0, "BadResourceQuota")
        require(process_rows[owner_process]["quota"] == index, "BadResourceQuota")
    previous_minted = ""
    seen_minted_slots: set[tuple[int, int]] = set()
    for index in range(minted_bindings):
        record = minted_binding_offset + index * GENERATION_MINTED_BINDING.size
        name_offset, owner, holder, slot, rights, flags, capability_kind = GENERATION_MINTED_BINDING.unpack_from(data, record)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(name > previous_minted, "NonCanonicalMintedBindings")
        require(owner < instances and holder < instances and slot < 64 and flags == 0, "BadMintedBinding")
        require(rights and not rights & ~RIGHT_ALL, "BadMintedBinding")
        require(capability_rights_valid(capability_kind, rights), "BadMintedBindingKind")
        # The holder must be owned by the minter, so a minted capability cannot
        # cross an ownership edge the instance graph does not declare.
        require(instance_rows[holder][2] == 1 and instance_rows[holder][3] == owner, "BadMintedBindingOwner")
        # No two declarations may claim one holder slot — neither two minted
        # bindings, nor a minted binding and one of the holder's own
        # grant-backed bindings. A collision would leave the slot naming two
        # capabilities and make the spawn-time slot ordering ambiguous.
        require((holder, slot) not in seen_minted_slots, "BadMintedBinding")
        holder_row = instance_rows[holder]
        bound_slots = {
            bound_slot
            for _, bound_slot in binding_rows[holder_row[7] : holder_row[7] + holder_row[8]]
        }
        require(slot not in bound_slots, "BadMintedBinding")
        seen_minted_slots.add((holder, slot))
        previous_minted = name
    return {"identity": identity, "number": number, "parent": None if parent == bytes(32) else parent, "target": target, "kernel_len": 0, "total_len": total_len}


def decode_bootstate(slot: bytes) -> dict:
    require(len(slot) == BOOTSTATE_SLOT_BYTES and slot[BOOTSTATE_MAGIC_OFFSET:BOOTSTATE_MAGIC_END] == BOOTSTATE_MAGIC, "BadBootStateMagic")
    version, header, flags, sequence = __import__("struct").unpack_from("<IIQQ", slot, BOOTSTATE_FORMAT_VERSION_OFFSET)
    require(version == BOOTSTATE_VERSION and header == BOOTSTATE_SLOT_BYTES and flags == 0, "BadBootStateVersion")
    require(sequence != 2**64 - 1 and not any(slot[BOOTSTATE_RESERVED_OFFSET:BOOTSTATE_RESERVED_END]) and not any(slot[BOOTSTATE_CHECKSUM_END:]), "BadBootStateReserved")
    require(slot[BOOTSTATE_CHECKSUM_OFFSET:BOOTSTATE_CHECKSUM_END] == bootstate_checksum(slot), "BadBootStateChecksum")
    known_good = slot[BOOTSTATE_KNOWN_GOOD_OFFSET:BOOTSTATE_KNOWN_GOOD_END]
    pending = slot[BOOTSTATE_PENDING_OFFSET:BOOTSTATE_PENDING_END]
    attempts = int.from_bytes(slot[BOOTSTATE_REMAINING_ATTEMPTS_OFFSET:BOOTSTATE_REMAINING_ATTEMPTS_END], "little")
    generation_root = slot[BOOTSTATE_GENERATION_ROOT_OFFSET:BOOTSTATE_GENERATION_ROOT_END]
    state_root = slot[BOOTSTATE_STATE_ROOT_OFFSET:BOOTSTATE_STATE_ROOT_END]
    accepted_release_sequence = int.from_bytes(slot[BOOTSTATE_ACCEPTED_RELEASE_SEQUENCE_OFFSET:BOOTSTATE_ACCEPTED_RELEASE_SEQUENCE_END], "little")
    require(known_good != bytes(32) and generation_root != bytes(32), "BadBootStateRoot")
    require((pending == bytes(32) and attempts == 0) or pending != bytes(32), "BadPendingAttempts")
    return {"sequence": sequence, "known_good": known_good, "pending": None if pending == bytes(32) else pending, "remaining_attempts": attempts, "generation_root": generation_root, "state_root": state_root, "accepted_release_sequence": accepted_release_sequence}


def check_release(data: bytes, generation: bytes, accepted_sequence: int | None = None) -> int:
    require(len(data) == RELEASE_BYTES and data[:8] == RELEASE_MAGIC, "BadReleaseMagic")
    version, header, flags = struct.unpack_from("<IIQ", data, RELEASE_HEADER_FORMAT_VERSION_OFFSET)
    require(version == RELEASE_VERSION and header == RELEASE_HEADER_BYTES and flags == 0, "BadReleaseVersion")
    sequence, target_len, trust_version = struct.unpack_from("<QII", data, RELEASE_HEADER_RELEASE_SEQUENCE_OFFSET)
    signature_count = struct.unpack_from("<I", data, RELEASE_HEADER_SIGNATURE_COUNT_OFFSET)[0]
    require(1 <= target_len <= MAX_TARGET_BYTES and trust_version == 1, "BadReleaseBounds")
    require(2 <= signature_count <= MAX_RELEASE_SIGNATURES and not any(data[RELEASE_HEADER_RESERVED_OFFSET:RELEASE_HEADER_RESERVED_END]), "BadReleaseSignatures")
    generation_info = check_generation(generation)
    require(data[RELEASE_HEADER_GENERATION_IDENTITY_OFFSET:RELEASE_HEADER_GENERATION_IDENTITY_END] == generation_info["identity"], "WrongReleaseGeneration")
    parent = generation_info["parent"] or bytes(32)
    require(data[RELEASE_HEADER_PARENT_IDENTITY_OFFSET:RELEASE_HEADER_PARENT_IDENTITY_END] == parent, "WrongReleaseParent")
    target = data[RELEASE_HEADER_TARGET_OFFSET : RELEASE_HEADER_TARGET_OFFSET + target_len].decode("utf-8")
    require(target == generation_info["target"] and not any(data[RELEASE_HEADER_TARGET_OFFSET + target_len : RELEASE_HEADER_TARGET_END]), "WrongReleaseTarget")
    version = struct.unpack_from("<I", generation, 8)[0]
    bundle = data[RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_OFFSET:RELEASE_HEADER_BOOT_BUNDLE_IDENTITY_END]
    require(version == GENERATION_VERSION and bundle != bytes(32), "MissingReleaseBootBundle")
    require(data[RELEASE_HEADER_AUTHORITY_MANIFEST_OFFSET:RELEASE_HEADER_AUTHORITY_MANIFEST_END] == authority_manifest_identity(generation), "WrongReleaseAuthority")
    if accepted_sequence is not None:
        require(sequence > accepted_sequence, "StaleRelease")
    key_by_id = {sha256(key): key for key in initial_public_keys()}
    previous = bytes(32)
    signed = ssh_signed_payload(data[:RELEASE_HEADER_BYTES])
    for index in range(signature_count):
        offset = RELEASE_HEADER_BYTES + index * RELEASE_SIGNATURE_BYTES
        key_id = data[offset + RELEASE_SIGNATURE_KEY_ID_OFFSET : offset + RELEASE_SIGNATURE_KEY_ID_END]
        signature = data[offset + RELEASE_SIGNATURE_SIGNATURE_OFFSET : offset + RELEASE_SIGNATURE_SIGNATURE_END]
        require(key_id > previous and key_id in key_by_id, "DuplicateOrUnknownReleaseKey")
        public = key_by_id[key_id]
        process = subprocess.run(
            [
                "cargo",
                "run",
                "--quiet",
                "--manifest-path",
                str(ROOT / "boot-contracts" / "Cargo.toml"),
                "--features",
                "release-crypto",
                "--example",
                "verify_release",
                "--",
                "signature",
                public.hex(),
                signed.hex(),
                signature.hex(),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        require(process.returncode == 0, "BadReleaseSignature")
        previous = key_id
    require(not any(data[RELEASE_HEADER_BYTES + signature_count * RELEASE_SIGNATURE_BYTES :]), "TrailingReleaseBytes")
    return sequence

def check_bootstore(data: bytes) -> dict:
    require(len(data) == BOOTSTORE_CAPACITY, "BadBootStoreCapacity")
    header = BOOTSTORE_HEADER.unpack_from(data, BOOTSTORE_DIRECTORY_OFFSET)
    magic, version, header_size, flags, count, reserved, directory_len, capacity, checksum = header
    require(magic == BOOTSTORE_MAGIC and version == BOOTSTORE_VERSION and header_size == BOOTSTORE_HEADER.size, "BadBootStoreVersion")
    require(flags == 0 and reserved == 0 and 1 <= count <= 64 and directory_len == count * BOOTSTORE_ENTRY.size and capacity == len(data), "BadBootStoreHeader")
    require(checksum == bootstore_checksum(data), "BadBootStoreChecksum")
    slots = []
    for label, offset in (("A", 0), ("B", BOOTSTATE_SLOT_BYTES)):
        try:
            slots.append((label, decode_bootstate(data[offset : offset + BOOTSTATE_SLOT_BYTES])))
        except CheckError:
            pass
    directory = []
    directory_start = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER.size
    previous_identity = bytes(32)
    for index in range(count):
        identity, offset, length, release_offset, release_length = BOOTSTORE_ENTRY.unpack_from(data, directory_start + index * BOOTSTORE_ENTRY.size)
        require(identity > previous_identity and offset % 4096 == 0 and offset >= BOOTSTORE_GENERATIONS_OFFSET and offset + length <= len(data), "BadBootDirectory")
        require(release_offset >= BOOTSTORE_RELEASES_OFFSET and release_offset % RELEASE_BYTES == 0 and release_length == RELEASE_BYTES and release_offset + release_length <= BOOTSTORE_GENERATIONS_OFFSET, "BadReleaseDirectory")
        generation = check_generation(data[offset : offset + length], identity)
        release = data[release_offset : release_offset + release_length]
        generation["release_sequence"] = check_release(release, data[offset : offset + length])
        generation.update({"offset": offset, "length": length})
        directory.append(generation)
        previous_identity = identity
    root = sha256(b"".join(generation["identity"] for generation in directory))
    matching_slots = [item for item in slots if item[1]["generation_root"] == root]
    require(matching_slots, "BadGenerationRoot")
    if len(matching_slots) == 2 and matching_slots[0][1]["sequence"] == matching_slots[1][1]["sequence"]:
        require(matching_slots[0][1] == matching_slots[1][1], "ConflictingBootStateSlots")
    matching_slots.sort(key=lambda item: (item[1]["sequence"], item[0] == "A"), reverse=True)
    selected_label, selected_state = matching_slots[0]
    by_identity = {generation["identity"]: generation for generation in directory}
    require(selected_state["known_good"] in by_identity, "MissingKnownGood")
    for generation in directory:
        if generation["parent"] is not None:
            require(generation["parent"] in by_identity, "BrokenParent")
    known_good_release = by_identity[selected_state["known_good"]]["release_sequence"]
    require(known_good_release <= selected_state["accepted_release_sequence"], "UnacceptedKnownGoodRelease")
    if selected_state["pending"] is not None:
        require(selected_state["pending"] in by_identity, "MissingPending")
        pending_release = by_identity[selected_state["pending"]]["release_sequence"]
        require(pending_release > selected_state["accepted_release_sequence"], "StalePendingRelease")
    return {"slot": selected_label, "state": selected_state, "generations": directory, "selected": by_identity[selected_state["known_good"]]}

def check_slot_recovery(data: bytes) -> None:
    for offset, expected_label in ((0, "B"), (BOOTSTATE_SLOT_BYTES, "A")):
        corrupted = bytearray(data)
        corrupted[offset + BOOTSTATE_CHECKSUM_OFFSET] ^= 0xFF
        require(bootstore_checksum(corrupted) == bootstore_checksum(data), "BootStateCoveredByBootStoreChecksum")
        result = check_bootstore(bytes(corrupted))
        require(result["slot"] == expected_label, "BootStateFallbackFailed")



def check_unknown_generation_version(data: bytes) -> None:
    generation = bytearray(data)
    generation[8:12] = (GENERATION_VERSION + 1).to_bytes(4, "little")
    try:
        check_generation(bytes(generation))
    except CheckError as error:
        require(str(error) == "UnsupportedGenerationVersion", "UnknownVersionAccepted")
    else:
        raise CheckError("UnknownVersionAccepted")


def main() -> None:
    try:
        data = Path(sys.argv[1]).read_bytes()
        result = check_bootstore(data)
        selected = result["selected"]
        offset = selected["offset"]
        check_unknown_generation_version(data[offset : offset + selected["length"]])
        check_slot_recovery(data)
    except (IndexError, OSError, CheckError, ValueError) as error:
        raise SystemExit(str(error)) from error
    selected = result["selected"]
    print(f"Boot store passed: slot {result['slot']} sequence {result['state']['sequence']}")
    print(f"selected={selected['identity'].hex()} parent={selected['parent'].hex() if selected['parent'] else 'none'} target={selected['target']} kernel={selected['kernel_len']}")


if __name__ == "__main__":
    main()
