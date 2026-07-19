#!/usr/bin/env python3

from __future__ import annotations

import sys
from pathlib import Path

from boot_contracts import *


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


def check_kernel_image(blob: bytes) -> None:
    require(len(blob) >= KERNEL_HEADER.size, "TruncatedKernelImage")
    magic, version, header, abi, flags, preferred, entry, segments, relocations, payload, total = KERNEL_HEADER.unpack_from(blob)
    require(magic == KERNEL_MAGIC, "BadKernelMagic")
    require(version == KERNEL_VERSION and header == KERNEL_HEADER.size and abi == KERNEL_ABI_VERSION, "BadKernelVersion")
    require(flags == 0 and preferred == KERNEL_PREFERRED_BASE and total == len(blob), "BadKernelHeader")
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
        require(KERNEL_PREFERRED_BASE <= absolute_addend < KERNEL_PREFERRED_BASE + image_end, "BadRelocationAddend")


def check_generation(data: bytes, expected_identity: bytes | None = None) -> dict:
    require(len(data) >= GENERATION_HEADER.size and len(data) <= MAX_GENERATION_BYTES, "TruncatedGeneration")
    fields = GENERATION_HEADER.unpack_from(data)
    (
        magic, version, header, required_flags, identity, number, parent,
        target_offset, kernel_index, bootstrap, boot_attempts,
        objects, components, dependencies, grants, states, health,
        object_offset, component_offset, dependency_offset, grant_offset,
        state_offset, health_offset, strings_offset, strings_len, payload_offset, total_len,
    ) = fields
    require(magic == GENERATION_MAGIC, "BadGenerationMagic")
    require(version == GENERATION_VERSION and header == GENERATION_HEADER.size, "UnsupportedGenerationVersion")
    require(required_flags == 0, "UnknownGenerationFlags")
    require(total_len == len(data) and generation_identity(data) == identity, "BadGenerationHash")
    if expected_identity is not None:
        require(identity == expected_identity, "GenerationIdentityMismatch")
    require(1 <= objects <= MAX_OBJECTS and 1 <= components <= MAX_COMPONENTS, "ExcessiveGenerationCount")
    require(dependencies <= MAX_DEPENDENCIES and grants <= MAX_GRANTS and states <= MAX_STATES and health <= MAX_HEALTH_COMPONENTS, "ExcessiveGenerationCount")
    require(strings_len <= MAX_STRING_TABLE_BYTES and target_offset < strings_len, "BadStringTable")
    require(object_offset == GENERATION_HEADER.size, "BadGenerationBounds")
    require(component_offset == object_offset + objects * GENERATION_OBJECT.size, "BadGenerationBounds")
    require(dependency_offset == component_offset + components * GENERATION_COMPONENT.size, "BadGenerationBounds")
    require(grant_offset == dependency_offset + dependencies * GENERATION_DEPENDENCY.size, "BadGenerationBounds")
    require(state_offset == grant_offset + grants * GENERATION_GRANT.size, "BadGenerationBounds")
    require(health_offset == state_offset + states * GENERATION_STATE.size, "BadGenerationBounds")
    require(strings_offset == health_offset + health * GENERATION_HEALTH.size, "BadGenerationBounds")
    require(payload_offset == strings_offset + strings_len, "BadGenerationBounds")
    target = read_string(data, strings_offset, strings_len, target_offset)
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
    require(kernel_index < objects and object_rows[kernel_index][1] == 1, "BadKernelObject")
    check_kernel_image(object_rows[kernel_index][2])
    component_rows = []
    previous_name = ""
    for index in range(components):
        name_offset, object_index, role, dependency_start, dependency_count = GENERATION_COMPONENT.unpack_from(data, component_offset + index * GENERATION_COMPONENT.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(name > previous_name and object_index < objects and 1 <= role <= 4, "BadComponent")
        require(dependency_start + dependency_count <= dependencies, "BadDependencyBounds")
        component_rows.append((name, object_index, role, dependency_start, dependency_count))
        previous_name = name
    require(bootstrap < components and component_rows[bootstrap][2] == 1 and object_rows[component_rows[bootstrap][1]][1] == 2, "BadBootstrap")
    for index, (_, _, _, start, count) in enumerate(component_rows):
        previous_dependency = -1
        for dependency_index in range(start, start + count):
            dependency = GENERATION_DEPENDENCY.unpack_from(data, dependency_offset + dependency_index * GENERATION_DEPENDENCY.size)[0]
            require(dependency < components and dependency != index and dependency > previous_dependency, "BadDependency")
            previous_dependency = dependency
    previous_grant = None
    for index in range(grants):
        name_offset, source, destination, rights, transferable = GENERATION_GRANT.unpack_from(data, grant_offset + index * GENERATION_GRANT.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        key = (name, source, destination)
        require(previous_grant is None or key > previous_grant, "NonCanonicalGrants")
        require(source < components and destination < components and rights and not rights & ~7 and transferable in (0, 1), "BadGrant")
        previous_grant = key
    previous_state = ""
    for index in range(states):
        name_offset, owner, schema_version, policy = GENERATION_STATE.unpack_from(data, state_offset + index * GENERATION_STATE.size)
        name = read_string(data, strings_offset, strings_len, name_offset)
        require(name > previous_state and owner < components and schema_version > 0 and policy in (1, 2, 3), "BadState")
        previous_state = name
    require(boot_attempts > 0, "BadHealthPolicy")
    previous_health = -1
    for index in range(health):
        component = GENERATION_HEALTH.unpack_from(data, health_offset + index * GENERATION_HEALTH.size)[0]
        require(component < components and component > previous_health, "BadHealthComponent")
        previous_health = component
    return {"identity": identity, "number": number, "parent": None if parent == bytes(32) else parent, "target": target, "kernel_len": len(object_rows[kernel_index][2])}


def decode_bootstate(slot: bytes) -> dict:
    require(len(slot) == BOOTSTATE_SLOT_BYTES and slot[:8] == BOOTSTATE_MAGIC, "BadBootStateMagic")
    version, header, flags, sequence = __import__("struct").unpack_from("<IIQQ", slot, 8)
    require(version == BOOTSTATE_VERSION and header == BOOTSTATE_SLOT_BYTES and flags == 0, "BadBootStateVersion")
    require(sequence != 2**64 - 1 and not any(slot[100:104]) and not any(slot[200:]), "BadBootStateReserved")
    require(slot[168:200] == bootstate_checksum(slot), "BadBootStateChecksum")
    known_good = slot[32:64]; pending = slot[64:96]; attempts = int.from_bytes(slot[96:100], "little")
    generation_root = slot[104:136]; state_root = slot[136:168]
    require(known_good != bytes(32) and generation_root != bytes(32), "BadBootStateRoot")
    require((pending == bytes(32) and attempts == 0) or (pending != bytes(32) and attempts > 0), "BadPendingAttempts")
    return {"sequence": sequence, "known_good": known_good, "pending": None if pending == bytes(32) else pending, "generation_root": generation_root, "state_root": state_root}


def check_bootstore(data: bytes) -> dict:
    require(len(data) == BOOTSTORE_CAPACITY, "BadBootStoreCapacity")
    header = BOOTSTORE_HEADER.unpack_from(data, BOOTSTORE_DIRECTORY_OFFSET)
    magic, version, header_size, flags, count, reserved, directory_len, capacity, checksum = header
    require(magic == BOOTSTORE_MAGIC and version == BOOTSTORE_VERSION and header_size == BOOTSTORE_HEADER.size, "BadBootStoreVersion")
    require(flags == 0 and reserved == 0 and count == 2 and directory_len == count * BOOTSTORE_ENTRY.size and capacity == len(data), "BadBootStoreHeader")
    require(checksum == bootstore_checksum(data), "BadBootStoreChecksum")
    slots = []
    for label, offset in (("A", 0), ("B", BOOTSTATE_SLOT_BYTES)):
        try:
            slots.append((label, decode_bootstate(data[offset : offset + BOOTSTATE_SLOT_BYTES])))
        except CheckError:
            pass
    require(slots, "NoValidBootState")
    if len(slots) == 2 and slots[0][1]["sequence"] == slots[1][1]["sequence"]:
        require(slots[0][1] == slots[1][1], "ConflictingBootStateSlots")
    slots.sort(key=lambda item: (item[1]["sequence"], item[0] == "A"), reverse=True)
    selected_label, selected_state = slots[0]
    directory = []
    directory_start = BOOTSTORE_DIRECTORY_OFFSET + BOOTSTORE_HEADER.size
    previous_identity = bytes(32)
    for index in range(count):
        identity, offset, length = BOOTSTORE_ENTRY.unpack_from(data, directory_start + index * BOOTSTORE_ENTRY.size)
        require(identity > previous_identity and offset % 4096 == 0 and offset >= BOOTSTORE_GENERATIONS_OFFSET and offset + length <= len(data), "BadBootDirectory")
        generation = check_generation(data[offset : offset + length], identity)
        directory.append(generation)
        previous_identity = identity
    root = sha256(b"".join(generation["identity"] for generation in directory))
    require(root == selected_state["generation_root"], "BadGenerationRoot")
    by_identity = {generation["identity"]: generation for generation in directory}
    require(selected_state["known_good"] in by_identity, "MissingKnownGood")
    for generation in directory:
        if generation["parent"] is not None:
            require(generation["parent"] in by_identity, "BrokenParent")
    return {"slot": selected_label, "state": selected_state, "generations": directory, "selected": by_identity[selected_state["known_good"]]}

def check_slot_recovery(data: bytes) -> None:
    for offset, expected_label in ((0, "B"), (BOOTSTATE_SLOT_BYTES, "A")):
        corrupted = bytearray(data)
        corrupted[offset + BOOTSTATE_CHECKSUM_OFFSET] ^= 0xFF
        require(bootstore_checksum(corrupted) == bootstore_checksum(data), "BootStateCoveredByBootStoreChecksum")
        result = check_bootstore(bytes(corrupted))
        require(result["slot"] == expected_label, "BootStateFallbackFailed")



def main() -> None:
    try:
        data = Path(sys.argv[1]).read_bytes()
        result = check_bootstore(data)
        check_slot_recovery(data)
    except (IndexError, OSError, CheckError, ValueError) as error:
        raise SystemExit(str(error)) from error
    selected = result["selected"]
    print(f"Boot store passed: slot {result['slot']} sequence {result['state']['sequence']}")
    print(f"selected={selected['identity'].hex()} parent={selected['parent'].hex() if selected['parent'] else 'none'} target={selected['target']} kernel={selected['kernel_len']}")


if __name__ == "__main__":
    main()
