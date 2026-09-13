#!/usr/bin/env python3

"""RP0 format-2 Raspberry Pi 5 ROS 2 demo contract gate.

Format 2 replaces format 1's frozen DDSI-RTPS shape with a named transport
discriminator plus one optional profile record per admitted family, so a
userspace transport can be swapped by changing generation data rather than by
migrating the contract.

Every wire-level constant this gate asserts is *derived*, not transcribed:

- the RIHS01 type hash is recomputed from the message's field types through the
  same hashable-JSON rendering `rcl_type_description_to_hashable_json` emits,
  and the implementation is first validated against the upstream
  `sensor_msgs/msg/PointCloud2` fixture copied into `rcl/test/rcl/test_type_hash.cpp`;
- the Zenoh data key expression is recomposed the way
  `liveliness::TopicInfo::TopicInfo` composes it;
- the per-sample CDR bytes are re-encoded from the classic `DDS_CDR`
  encapsulation `rmw_zenoh_cpp/src/detail/cdr.cpp` selects;
- the 33-byte attachment length is recomputed from `zenoh::ext::Serializer`'s
  fixed-width little-endian integers and LEB128-prefixed fixed arrays.

A literal in the fixture that the derivation does not reproduce fails the gate.
That is the point: REP-2011 was never merged, so `ros2/rcl`'s implementation is
the only specification of the hash, and a hand-copied digest would be
unverifiable.
"""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import hashlib
import json
import os
import struct
import subprocess
import tempfile
from typing import NoReturn

from harness import ROOT
from zutai_cli import STDLIB, binary

CONTRACT = ROOT / "contracts" / "rpi5-ros2-demo" / "v2"
SCHEMA = CONTRACT / "schema.zt"
CHECK = CONTRACT / "check.zt"
INVALID_CHECK = CONTRACT / "check-invalid.zt"
FIXTURE = CONTRACT / "fixtures" / "valid.zti"

EXPECTED_CONTRACT_ID = "rpi5-ros2-demo-v2"
EXPECTED_TARGET = "aarch64-rpi5"
EXPECTED_BOARD = "raspberry-pi-5-model-b-rev1"
EXPECTED_REVISIONS = ["c04170", "d04170"]
EXPECTED_FIRMWARE = "rpi-eeprom-2712-2026-05-27-default"
EXPECTED_ROS_PROFILE = "ros2-profile-0-kilted-native"
EXPECTED_TRANSPORT = "zenoh"
EXPECTED_ZENOH_PROFILE = "zenoh-profile-0-static-local-reliable"
EXPECTED_MESSAGE_TYPE = "slime_demo_msgs/msg/Counter"

# Every transport family this format admits, and the contract field each one
# requires. A `transport` value outside this map is rejected outright rather
# than resolved to a nearby family.
TRANSPORT_PROFILES = {"zenoh": "zenoh", "ddsi-rtps": "ddsiRtps"}

# REP-2000 lists rmw_zenoh_cpp Tier 1 under Kilted Kaiju and Rolling, and omits
# it from the Jazzy Jalisco middleware table, so the ROS baseline moves with the
# transport choice.
EXPECTED_DISTRIBUTION = "kilted-kaiju"

# `commons/zenoh-protocol/src/lib.rs`: `pub const VERSION: u8 = 0x09`.
EXPECTED_ZENOH_PROTOCOL_VERSION = 9

# Zenoh 1.0.0 session establishment plus the framing and declaration surface a
# bounded local pub/sub needs, and nothing else.
EXPECTED_SESSION_MESSAGES = [
    "INIT_SYN",
    "INIT_ACK",
    "OPEN_SYN",
    "OPEN_ACK",
    "FRAME",
    "CLOSE",
]
EXPECTED_DECLARATION_MESSAGES = [
    "DECLARE_SUBSCRIBER",
    "UNDECLARE_SUBSCRIBER",
    "PUSH_PUT",
]
EXPECTED_ATTACHMENT_FIELDS = [
    "sequenceNumber:i64le",
    "sourceTimestamp:i64le",
    "sourceGid:leb128len+16",
]

EXPECTED_BOARD_DEVICES = [
    "bcm2712-cortex-a76",
    "arm-gic-400",
    "arm-armv8-timer",
    "uart10-pl011",
    "microsd-boot-media-read-only-after-boot",
    "firmware-final-device-tree",
]
EXPECTED_ROS_API = [
    "init",
    "create-node",
    "create-publisher",
    "create-subscription",
    "spin",
    "publish",
    "receive-callback",
    "log",
    "shutdown",
]
EXPECTED_PUBLISHER = {
    "component": "ros2-demo-publisher",
    "name": "slime_counter_publisher",
    "namespace": "/slime_demo",
    "role": "publisher",
}
EXPECTED_SUBSCRIBER = {
    "component": "ros2-demo-subscriber",
    "name": "slime_counter_subscriber",
    "namespace": "/slime_demo",
    "role": "subscriber",
}
EXPECTED_BOUNDS = {
    "maxTextBytes": 384,
    "maxSequenceItems": 16,
    "maxMessageBytes": 12,
    "maxPayloadBytes": 12,
    "maxTransportMessageBytes": 512,
    "maxAttachmentBytes": 33,
    "maxKeyexprBytes": 129,
    "maxFragmentBytes": 0,
    "maxFragmentsPerSample": 0,
    "maxQueueDepth": 4,
    "maxHistoryDepth": 4,
    "maxSessions": 2,
    "maxPublishers": 1,
    "maxSubscribers": 1,
    "maxOutstandingSamples": 4,
    "maxRetries": 3,
    "maxTraceRecords": 32,
    "maxTraceRecordBytes": 192,
    "maxTraceBytes": 6144,
    "maxSerialLineBytes": 256,
    "maxLogBytes": 8192,
    "maxCapabilityGrants": 13,
}
EXPECTED_ENDPOINTS = {
    "publisher-session": ("tcp", "127.0.0.1", 7447, "connect"),
    "subscriber-session": ("tcp", "127.0.0.1", 7447, "listen"),
}
EXPECTED_QOS = {
    "history": "keep-last",
    "historyDepth": 4,
    "reliability": "reliable",
    "durability": "volatile",
    "liveliness": "automatic",
    "deadlineNs": 1_000_000_000,
    "lifespanNs": 5_000_000_000,
    "leaseNs": 2_000_000_000,
}
EXPECTED_RECORD_KINDS = [
    "contract-admitted",
    "board-admitted",
    "generation-admitted",
    "session-open",
    "declaration-matched",
    "wire-put-sent",
    "payload-decoded",
    "subscriber-sample-validated",
    "session-closed",
    "teardown-complete",
    "failure",
]
EXPECTED_MARKERS = {
    "success": "[rpi5-ros2-demo] success profile=rpi5-ros2-demo-v2 samples=4",
    "denial": "[rpi5-ros2-demo] failure class=denied",
    "timeout": "[rpi5-ros2-demo] failure class=timeout",
    "wrongBoard": "[rpi5-ros2-demo] failure class=wrong-board",
    "wrongTarget": "[rpi5-ros2-demo] failure class=wrong-target",
    "wrongTransport": "[rpi5-ros2-demo] failure class=wrong-transport",
    "malformedWire": "[rpi5-ros2-demo] failure class=malformed-wire",
    "malformedPayload": "[rpi5-ros2-demo] failure class=malformed-payload",
    "malformedGeneration": "[rpi5-ros2-demo] failure class=malformed-generation",
}
EXPECTED_CAPABILITIES = {
    ("ros2-demo-publisher", "zenoh-publisher", "counter-publisher", ("publish",)),
    ("ros2-demo-publisher", "clock", "demo-monotonic-clock", ("read", "wait")),
    ("ros2-demo-publisher", "log-sink", "demo-serial-log", ("write",)),
    ("ros2-demo-subscriber", "zenoh-subscriber", "counter-subscriber", ("subscribe",)),
    ("ros2-demo-subscriber", "clock", "demo-monotonic-clock", ("read", "wait")),
    ("ros2-demo-subscriber", "log-sink", "demo-serial-log", ("write",)),
    (
        "slime-zenoh-profile-0",
        "stream-connect",
        "tcp/127.0.0.1:7447",
        ("connect", "send", "receive"),
    ),
    (
        "slime-zenoh-profile-0",
        "stream-listen",
        "tcp/127.0.0.1:7447",
        ("listen", "send", "receive"),
    ),
    ("slime-zenoh-profile-0", "session", "publisher-session", ("open-static", "write")),
    ("slime-zenoh-profile-0", "session", "subscriber-session", ("open-static", "read")),
    ("slime-zenoh-profile-0", "trace-sink", "rpi5-ros2-demo-trace", ("append", "seal")),
    ("rpi5-board-service", "device", "uart10-pl011", ("configure", "write")),
    ("rpi5-board-service", "storage", "boot-microsd", ("read",)),
}

# `type_description_interfaces/msg/FieldType.msg`. Only the scalar types the
# admitted interface subset allows are listed: an unlisted ROS field kind is a
# gate failure, not a silently defaulted type id.
FIELD_TYPE_IDS = {
    "int8": 2,
    "uint8": 3,
    "int16": 4,
    "uint16": 5,
    "int32": 6,
    "uint32": 7,
    "int64": 8,
    "uint64": 9,
    "float": 10,
    "double": 11,
    "boolean": 15,
    "byte": 16,
    "string": 17,
}

# Fixed CDR width per admitted scalar, for re-encoding the golden sample bytes.
CDR_FORMATS = {
    "int8": "<b",
    "uint8": "<B",
    "int16": "<h",
    "uint16": "<H",
    "int32": "<i",
    "uint32": "<I",
    "int64": "<q",
    "uint64": "<Q",
}

# `rmw_zenoh_cpp/src/detail/cdr.cpp` selects classic `DDS_CDR`, whose
# encapsulation is a 2-byte representation identifier plus 2 option bytes.
# CDR_LE with no options.
CDR_ENCAPSULATION = bytes([0x00, 0x01, 0x00, 0x00])

# `RMW_GID_STORAGE_SIZE`.
GID_BYTES = 16

# The upstream cross-check: `rcl/test/rcl/test_type_hash.cpp` embeds this value
# as "Copied directly from generated code" for sensor_msgs/msg/PointCloud2.
POINTCLOUD2_TYPE_HASH = (
    "RIHS01_9198cabf7da3796ae6fe19c4cb3bdd3525492988c70522628af5daa124bae2b5"
)


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rpi5 ROS 2 demo contract v2 check: {message}")


def zutai(*arguments: str, fixture: _Path | None = None) -> str:
    environment = os.environ.copy()
    environment["ZUTAI_STDLIB_ROOT"] = str(STDLIB)
    if fixture is not None:
        environment["SLIME_RPI5_ROS2_DEMO_CONTRACT_PATH"] = str(fixture)
    process = subprocess.run(
        [str(binary()), *arguments],
        cwd=ROOT,
        env=environment,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        _sys.stderr.write(process.stdout)
        _sys.stderr.write(process.stderr)
        raise SystemExit(process.returncode)
    return process.stdout


def typed_fixture(path: _Path) -> None:
    output = zutai("run", str(CHECK), fixture=path)
    if not output.startswith("#valid"):
        fail(f"{path.name} failed structural decoding: {output.strip()}")


def load_fixture(path: _Path) -> dict:
    output = zutai("json", str(path))
    try:
        value = json.loads(output)
    except json.JSONDecodeError as error:
        fail(f"{path.name} did not project to JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{path.name} is not a record")
    return value


def exact(value, expected, path: str) -> None:
    if value != expected:
        fail(f"{path} must equal {expected!r}, got {value!r}")


def bounded_text(value, limit: int, path: str) -> None:
    if not isinstance(value, str) or not value or len(value.encode("utf-8")) > limit:
        fail(f"{path} is empty or exceeds {limit} UTF-8 bytes")


def unique(values: list[str], path: str) -> None:
    if len(values) != len(set(values)):
        fail(f"{path} contains duplicates")


def positive(value: int, path: str, *, zero_allowed: bool = False) -> None:
    minimum = 0 if zero_allowed else 1
    if not isinstance(value, int) or isinstance(value, bool) or value < minimum:
        fail(f"{path} must be an integer >= {minimum}")


def hashable_type_description(description: dict) -> str:
    """Render a TypeDescription the way `rcl_type_description_to_hashable_json` does.

    libyaml's flow style with width and break both set to -1 yields `", "` and
    `": "` separators, double-quoted keys and string scalars, plain integers, no
    document markers, and no trailing break. `rosidl`'s own Python hashing path
    uses these exact `json.dumps` separators and says in a comment that they are
    libyaml's builtin behaviour.
    """
    return json.dumps(
        description, separators=(", ", ": "), sort_keys=False, ensure_ascii=False
    )


def rihs01(description: dict) -> str:
    """RIHS01 of a TypeDescription: SHA-256 of the hashable text, no trailing NUL."""
    text = hashable_type_description(description)
    return "RIHS01_" + hashlib.sha256(text.encode("utf-8")).hexdigest()


def field_type(type_id: int, *, capacity: int = 0, string_capacity: int = 0, nested: str = "") -> dict:
    return {
        "type_id": type_id,
        "capacity": capacity,
        "string_capacity": string_capacity,
        "nested_type_name": nested,
    }


def validate_rihs01_implementation() -> None:
    """Prove the hash implementation against the upstream PointCloud2 fixture.

    Without this the gate would assert a digest no reader could check. The
    fixture exercises nested types, an unbounded nested sequence, an unbounded
    primitive sequence, booleans, and the alphabetical ordering of
    `referenced_type_descriptions`.
    """
    nested, string, uint8, int32, uint32, boolean = 1, 17, 3, 6, 7, 15
    nested_unbounded_sequence, uint8_unbounded_sequence = 145, 147

    def field(name: str, ftype: dict) -> dict:
        return {"name": name, "type": ftype}

    point_cloud2 = {
        "type_description": {
            "type_name": "sensor_msgs/msg/PointCloud2",
            "fields": [
                field("header", field_type(nested, nested="std_msgs/msg/Header")),
                field("height", field_type(uint32)),
                field("width", field_type(uint32)),
                field(
                    "fields",
                    field_type(
                        nested_unbounded_sequence, nested="sensor_msgs/msg/PointField"
                    ),
                ),
                field("is_bigendian", field_type(boolean)),
                field("point_step", field_type(uint32)),
                field("row_step", field_type(uint32)),
                field("data", field_type(uint8_unbounded_sequence)),
                field("is_dense", field_type(boolean)),
            ],
        },
        "referenced_type_descriptions": [
            {
                "type_name": "builtin_interfaces/msg/Time",
                "fields": [
                    field("sec", field_type(int32)),
                    field("nanosec", field_type(uint32)),
                ],
            },
            {
                "type_name": "sensor_msgs/msg/PointField",
                "fields": [
                    field("name", field_type(string)),
                    field("offset", field_type(uint32)),
                    field("datatype", field_type(uint8)),
                    field("count", field_type(uint32)),
                ],
            },
            {
                "type_name": "std_msgs/msg/Header",
                "fields": [
                    field(
                        "stamp",
                        field_type(nested, nested="builtin_interfaces/msg/Time"),
                    ),
                    field("frame_id", field_type(string)),
                ],
            },
        ],
    }
    computed = rihs01(point_cloud2)
    if computed != POINTCLOUD2_TYPE_HASH:
        fail(
            "RIHS01 implementation disagrees with the upstream PointCloud2 fixture: "
            f"computed {computed}, expected {POINTCLOUD2_TYPE_HASH}"
        )


def derive_type_description(message: dict) -> dict:
    """Build the TypeDescription for an admitted flat message.

    `capacity`, `string_capacity`, and `nested_type_name` are emitted explicitly
    as 0/0/"" for plain scalars, matching `serialize_field_type`.
    """
    fields = []
    for field in message["fields"]:
        kind = field["kind"]
        type_id = FIELD_TYPE_IDS.get(kind)
        if type_id is None:
            fail(f"message field {field['name']!r} uses unadmitted kind {kind!r}")
        if field["bound"] != 1:
            fail(f"message field {field['name']!r} is not a plain scalar")
        fields.append({"name": field["name"], "type": field_type(type_id)})
    return {
        "type_description": {"type_name": message["rosName"], "fields": fields},
        "referenced_type_descriptions": [],
    }


def derive_dds_type_name(ros_name: str) -> str:
    """`_create_type_name`: namespace + "::" + "dds_::" + name + "_"."""
    parts = ros_name.split("/")
    if len(parts) != 3:
        fail(f"message rosName {ros_name!r} is not pkg/msg/Type")
    package, middle, name = parts
    return f"{package}::{middle}::dds_::{name}_"


def derive_keyexpr(domain_id: int, topic: str, type_name: str, type_hash: str) -> str:
    """`liveliness::TopicInfo::TopicInfo` composition."""
    return f"{domain_id}/{topic.strip('/')}/{type_name}/{type_hash}"


def derive_cdr(message: dict, values: dict[str, int]) -> bytes:
    """Classic DDS_CDR encapsulation plus the little-endian field values.

    Every admitted field is a fixed-width 4-byte-or-smaller scalar whose natural
    alignment the declaration order already satisfies, so no padding is inserted.
    A wider or string field would need alignment handling and is rejected by
    `derive_type_description` before reaching here.
    """
    payload = bytearray(CDR_ENCAPSULATION)
    for field in message["fields"]:
        kind = field["kind"]
        fmt = CDR_FORMATS.get(kind)
        if fmt is None:
            fail(f"message field {field['name']!r} has no fixed CDR encoding")
        payload.extend(struct.pack(fmt, values[field["name"]]))
    return bytes(payload)


def leb128_unsigned(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def derive_attachment_bytes() -> int:
    """`zenoh::ext::Serializer` over (i64, i64, [u8; 16]).

    Integers are fixed-width little-endian with no tag; a fixed-size array is
    still serialized as a variable-length sequence, so its LEB128 length prefix
    is present even though both ends know the size. `finish()` adds no container
    header.
    """
    return 8 + 8 + len(leb128_unsigned(GID_BYTES)) + GID_BYTES


def validate_transport_selection(contract: dict) -> None:
    """Check the discriminator structurally, then check which family was chosen.

    The structural rules -- the family is admitted, its profile is present, and
    no other family's profile is -- hold for every format-2 contract. Which
    family *this* fixture selects is a separate, narrower assertion, so it comes
    last: checking it first would mask a genuine discriminator/profile
    disagreement behind a "wrong family" complaint.
    """
    transport = contract["transport"]
    if transport not in TRANSPORT_PROFILES:
        fail(f"transport {transport!r} is not an admitted family")
    selected = TRANSPORT_PROFILES[transport]
    if selected not in contract:
        fail(f"transport {transport!r} names absent profile {selected!r}")
    for family, field in TRANSPORT_PROFILES.items():
        if field != selected and field in contract:
            fail(f"transport {transport!r} carries unrelated {family!r} profile")
    exact(transport, EXPECTED_TRANSPORT, "transport")


def validate_identifiers(contract: dict) -> None:
    exact(contract["formatVersion"], 2, "formatVersion")
    exact(contract["id"], EXPECTED_CONTRACT_ID, "id")
    exact(contract["targetProfile"], EXPECTED_TARGET, "targetProfile")

    board = contract["board"]
    exact(board["id"], EXPECTED_BOARD, "board.id")
    exact(board["model"], "Raspberry Pi 5 Model B", "board.model")
    exact(board["acceptedRevisionCodes"], EXPECTED_REVISIONS, "board.acceptedRevisionCodes")
    exact(board["soc"], "bcm2712c1", "board.soc")
    exact(board["architecture"], "aarch64", "board.architecture")
    exact(board["abi"], "slime-aarch64-v1", "board.abi")
    exact(board["pageProfile"], "aarch64-4k", "board.pageProfile")
    exact(board["firmwareId"], EXPECTED_FIRMWARE, "board.firmwareId")
    exact(board["bootFlow"], "bcm2712-rom-to-spi-eeprom-to-fat32", "board.bootFlow")
    exact(board["kernelPath"], "kernel8.img", "board.kernelPath")
    exact(board["configPath"], "config.txt", "board.configPath")
    exact(board["deviceTreePath"], "bcm2712-rpi-5-b.dtb", "board.deviceTreePath")
    exact(board["mediaKind"], "removable-microsd", "board.mediaKind")
    exact(
        board["mediaLayout"],
        "mbr-fat32-boot-plus-slime-generation-store",
        "board.mediaLayout",
    )
    exact(board["memoryMapSource"], "firmware-final-device-tree", "board.memoryMapSource")
    exact(
        board["interruptController"],
        "arm-gic-400-gicv2-from-device-tree",
        "board.interruptController",
    )
    exact(board["genericTimer"], "arm-armv8-timer-from-device-tree", "board.genericTimer")
    exact(
        board["serialPath"],
        "uart10-debug-header-pl011-0x107d001000",
        "board.serialPath",
    )
    exact(board["serialBaud"], 115200, "board.serialBaud")
    exact(board["requiredDevices"], EXPECTED_BOARD_DEVICES, "board.requiredDevices")

    ros = contract["ros"]
    exact(ros["id"], EXPECTED_ROS_PROFILE, "ros.id")
    exact(ros["distribution"], EXPECTED_DISTRIBUTION, "ros.distribution")
    exact(ros["apiSubset"], EXPECTED_ROS_API, "ros.apiSubset")
    exact(ros["nodeRoute"], "slime-native-ros-compatible-components", "ros.nodeRoute")
    exact(ros["rmwBoundary"], "slime-rmw-profile-0-over-bounded-zenoh", "ros.rmwBoundary")
    exact(ros["transportRuntime"], "slime-zenoh-profile-0", "ros.transportRuntime")
    exact(ros["representation"], "cdr-le", "ros.representation")
    exact(ros["endianness"], "little", "ros.endianness")
    exact(ros["securityEnabled"], False, "ros.securityEnabled")


def validate_zenoh_profile(contract: dict) -> None:
    zenoh = contract["zenoh"]
    exact(zenoh["id"], EXPECTED_ZENOH_PROFILE, "zenoh.id")
    exact(zenoh["protocolVersion"], EXPECTED_ZENOH_PROTOCOL_VERSION, "zenoh.protocolVersion")
    exact(zenoh["sessionMode"], "peer", "zenoh.sessionMode")
    exact(zenoh["linkProtocol"], "tcp", "zenoh.linkProtocol")
    exact(zenoh["batchLengthBytes"], 2, "zenoh.batchLengthBytes")
    exact(zenoh["sessionMessages"], EXPECTED_SESSION_MESSAGES, "zenoh.sessionMessages")
    exact(
        zenoh["declarationMessages"],
        EXPECTED_DECLARATION_MESSAGES,
        "zenoh.declarationMessages",
    )
    exact(zenoh["discoveryMode"], "static-generation-declared", "zenoh.discoveryMode")
    exact(zenoh["domainId"], 0, "zenoh.domainId")
    exact(zenoh["attachmentFields"], EXPECTED_ATTACHMENT_FIELDS, "zenoh.attachmentFields")
    exact(zenoh["qos"], EXPECTED_QOS, "zenoh.qos")
    exact(
        zenoh["teardown"],
        "undeclare-subscriber-then-close-session",
        "zenoh.teardown",
    )

    # No router, no multicast, no gossip, and no liveliness tokens: a bounded
    # static peer must not depend on any discovery mechanism, and rmw_zenoh's
    # own default discovery path (router gossip) is exactly what a
    # generation-declared graph replaces.
    for name in ("routerRequired", "multicastScouting", "gossipScouting", "livelinessTokens"):
        if zenoh[name] is not False:
            fail(f"zenoh.{name} must be false for a static bounded profile")

    # The Zenoh stream transport prefixes each batch with a 2-byte little-endian
    # length, so no admitted batch may exceed what that field can express.
    if zenoh["maxBatchBytes"] > 0xFFFF:
        fail("zenoh.maxBatchBytes exceeds the 2-byte batch length field")

    endpoint_rows = {}
    for endpoint in zenoh["endpoints"]:
        name = endpoint["name"]
        if name in endpoint_rows:
            fail(f"duplicate endpoint {name}")
        endpoint_rows[name] = (
            endpoint["protocol"],
            endpoint["address"],
            endpoint["port"],
            endpoint["direction"],
        )
        if endpoint["address"] in {"0.0.0.0", "::", "*"} or "*" in endpoint["address"]:
            fail(f"endpoint {name} grants a wildcard destination")
        if not 1 <= endpoint["port"] <= 65535:
            fail(f"endpoint {name} has an invalid port")
    exact(endpoint_rows, EXPECTED_ENDPOINTS, "zenoh.endpoints")

    if len(zenoh["endpoints"]) > contract["bounds"]["maxSessions"]:
        fail("zenoh.endpoints exceed maxSessions")


def validate_derived_wire_values(contract: dict) -> None:
    """Recompute every wire constant and compare it to the frozen literal."""
    validate_rihs01_implementation()

    workload = contract["workload"]
    message = workload["message"]
    zenoh = contract["zenoh"]

    description = derive_type_description(message)
    derived_input = hashable_type_description(description)
    derived_hash = rihs01(description)

    exact(message["typeHashInput"], derived_input, "workload.message.typeHashInput")
    exact(message["typeHash"], derived_hash, "workload.message.typeHash")
    # The keyexpr embeds the hash, so the two records cannot disagree.
    exact(zenoh["typeHash"], derived_hash, "zenoh.typeHash")

    derived_type_name = derive_dds_type_name(message["rosName"])
    exact(zenoh["typeNameOnWire"], derived_type_name, "zenoh.typeNameOnWire")
    if "/" in derived_type_name:
        fail("the on-wire type name must be DDS-mangled, not the ROS slash form")

    derived_keyexpr = derive_keyexpr(
        zenoh["domainId"], workload["topic"], derived_type_name, derived_hash
    )
    exact(zenoh["dataKeyexpr"], derived_keyexpr, "zenoh.dataKeyexpr")
    exact(
        zenoh["keyexprFormat"],
        "<domainId>/<topicWithoutOuterSlashes>/<typeNameOnWire>/<typeHash>",
        "zenoh.keyexprFormat",
    )
    for wildcard in ("*", "**"):
        if wildcard in derived_keyexpr:
            fail("the data key expression must not contain a Zenoh wildcard")
    keyexpr_bytes = len(derived_keyexpr.encode("utf-8"))
    if keyexpr_bytes > contract["bounds"]["maxKeyexprBytes"]:
        fail("the data key expression exceeds maxKeyexprBytes")
    exact(contract["bounds"]["maxKeyexprBytes"], keyexpr_bytes, "bounds.maxKeyexprBytes")

    derived_attachment = derive_attachment_bytes()
    exact(zenoh["attachmentBytes"], derived_attachment, "zenoh.attachmentBytes")
    exact(
        contract["bounds"]["maxAttachmentBytes"],
        derived_attachment,
        "bounds.maxAttachmentBytes",
    )

    samples = workload["samples"]
    exact(len(samples), workload["publishCount"], "workload.samples length")
    for index, sample in enumerate(samples):
        exact(sample["sequence"], index, f"workload.samples[{index}].sequence")
        derived = derive_cdr(
            message, {"sequence": sample["sequence"], "value": sample["value"]}
        )
        exact(sample["cdrHex"], derived.hex(), f"workload.samples[{index}].cdrHex")
        if len(derived) != message["maxSerializedBytes"]:
            fail(f"workload.samples[{index}] does not match maxSerializedBytes")


def validate_bounds(contract: dict) -> None:
    bounds = contract["bounds"]
    zero_allowed = {"maxFragmentBytes", "maxFragmentsPerSample"}
    for name, value in bounds.items():
        positive(value, f"bounds.{name}", zero_allowed=name in zero_allowed)
    exact(bounds, EXPECTED_BOUNDS, "bounds")

    text_limit = EXPECTED_BOUNDS["maxTextBytes"]
    sequence_limit = EXPECTED_BOUNDS["maxSequenceItems"]

    def walk(value, path: str) -> None:
        if isinstance(value, str):
            bounded_text(value, text_limit, path)
        elif isinstance(value, list):
            if len(value) > sequence_limit and path not in {
                "capabilities",
                "trace.requiredSuccessSequence",
            }:
                fail(f"{path} exceeds maxSequenceItems")
            for index, item in enumerate(value):
                walk(item, f"{path}[{index}]")
        elif isinstance(value, dict):
            for name, item in value.items():
                walk(item, f"{path}.{name}" if path else name)

    walk(contract, "")

    workload = contract["workload"]
    message = workload["message"]
    exact(message["maxSerializedBytes"], 12, "workload.message.maxSerializedBytes")
    if message["maxSerializedBytes"] > bounds["maxMessageBytes"]:
        fail("message maxSerializedBytes exceeds maxMessageBytes")
    if bounds["maxMessageBytes"] > bounds["maxPayloadBytes"]:
        fail("maxMessageBytes exceeds maxPayloadBytes")
    # A transport message carries the payload, its attachment, and the key
    # expression naming its resource, so the framing ceiling must cover all three.
    if (
        bounds["maxPayloadBytes"] + bounds["maxAttachmentBytes"] + bounds["maxKeyexprBytes"]
        > bounds["maxTransportMessageBytes"]
    ):
        fail("payload, attachment, and key expression exceed maxTransportMessageBytes")
    if bounds["maxTransportMessageBytes"] > contract["zenoh"]["maxBatchBytes"]:
        fail("maxTransportMessageBytes exceeds the admitted batch ceiling")
    exact(bounds["maxSessions"], 2, "bounds.maxSessions")
    exact(bounds["maxPublishers"], 1, "bounds.maxPublishers")
    exact(bounds["maxSubscribers"], 1, "bounds.maxSubscribers")
    if workload["publishCount"] > bounds["maxOutstandingSamples"]:
        fail("workload.publishCount exceeds maxOutstandingSamples")
    if len(contract["capabilities"]) > bounds["maxCapabilityGrants"]:
        fail("capability inventory exceeds maxCapabilityGrants")
    if len(contract["capabilities"]) != bounds["maxCapabilityGrants"]:
        fail("capability inventory does not fill maxCapabilityGrants")
    if len(contract["trace"]["requiredSuccessSequence"]) > bounds["maxTraceRecords"]:
        fail("required success trace exceeds maxTraceRecords")
    if len(contract["trace"]["recordKinds"]) > bounds["maxSequenceItems"]:
        fail("trace record kinds exceed maxSequenceItems")
    if bounds["maxTraceRecordBytes"] * bounds["maxTraceRecords"] > bounds["maxTraceBytes"]:
        fail("trace record/count ceilings exceed maxTraceBytes")
    if bounds["maxFragmentBytes"] == 0 and bounds["maxFragmentsPerSample"] != 0:
        fail("fragment count must be zero when fragmentation is disabled")
    if contract["zenoh"]["qos"]["historyDepth"] > bounds["maxHistoryDepth"]:
        fail("zenoh.qos.historyDepth exceeds maxHistoryDepth")


def validate_qos_ordering(contract: dict) -> None:
    qos = contract["zenoh"]["qos"]
    for name in ("deadlineNs", "lifespanNs", "leaseNs"):
        positive(qos[name], f"zenoh.qos.{name}")
    if contract["workload"]["publishPeriodNs"] > qos["deadlineNs"]:
        fail("workload.publishPeriodNs exceeds zenoh.qos.deadlineNs")
    if qos["deadlineNs"] > qos["lifespanNs"]:
        fail("zenoh.qos.deadlineNs exceeds zenoh.qos.lifespanNs")


def validate_workload(contract: dict) -> None:
    workload = contract["workload"]
    exact(workload["publisher"], EXPECTED_PUBLISHER, "workload.publisher")
    exact(workload["subscriber"], EXPECTED_SUBSCRIBER, "workload.subscriber")
    exact(workload["topic"], "/slime_demo/counter", "workload.topic")
    message = workload["message"]
    exact(message["rosName"], EXPECTED_MESSAGE_TYPE, "workload.message.rosName")
    descriptor = f"{message['rosName']}\nsequence:uint32\nvalue:int32\n"
    identity = f"sha256:{hashlib.sha256(descriptor.encode()).hexdigest()}"
    exact(message["typeIdentity"], identity, "workload.message.typeIdentity")
    exact(
        message["idl"],
        (
            "module slime_demo_msgs { module msg { struct Counter { "
            "uint32 sequence; int32 value; }; }; };"
        ),
        "workload.message.idl",
    )
    exact(
        message["fields"],
        [
            {"name": "sequence", "kind": "uint32", "bound": 1},
            {"name": "value", "kind": "int32", "bound": 1},
        ],
        "workload.message.fields",
    )
    positive(workload["publishCount"], "workload.publishCount")
    positive(workload["publishPeriodNs"], "workload.publishPeriodNs")
    exact(workload["publishCount"], 4, "workload.publishCount")
    exact(workload["publishPeriodNs"], 250_000_000, "workload.publishPeriodNs")
    exact(
        [sample["value"] for sample in workload["samples"]],
        [10, 20, 30, 40],
        "workload.samples values",
    )
    exact(
        workload["expectedSubscriberLine"],
        "[rpi5-ros2-demo] received count=4 sequences=0,1,2,3 values=10,20,30,40",
        "workload.expectedSubscriberLine",
    )
    if len(workload["expectedSubscriberLine"].encode("utf-8")) > contract["bounds"]["maxSerialLineBytes"]:
        fail("workload.expectedSubscriberLine exceeds maxSerialLineBytes")


def validate_capabilities(contract: dict) -> None:
    # The general rules run before the exact inventory: no component may hold a
    # router, gossip, multicast, scouting, or discovery grant, because the graph
    # is generation data and there is nothing for such authority to do. Comparing
    # the whole inventory first would report "the set differs" for a grant that
    # violates a named rule.
    for grant in contract["capabilities"]:
        for forbidden in ("router", "gossip", "multicast", "scouting", "discovery"):
            if forbidden in grant["kind"] or forbidden in grant["object"]:
                fail(f"capability {grant['kind']}/{grant['object']} grants {forbidden}")

    actual = set()
    for index, grant in enumerate(contract["capabilities"]):
        rights = grant["rights"]
        if not rights:
            fail(f"capabilities[{index}].rights is empty")
        unique(rights, f"capabilities[{index}].rights")
        for value in (grant["holder"], grant["kind"], grant["object"], *rights):
            if "*" in value or value in {"any", "all", "ambient", "wildcard"}:
                fail(f"capabilities[{index}] contains ambient or wildcard authority")
        row = (grant["holder"], grant["kind"], grant["object"], tuple(rights))
        if row in actual:
            fail(f"duplicate capability grant {row}")
        actual.add(row)
    exact(actual, EXPECTED_CAPABILITIES, "capabilities")


def validate_trace_and_markers(contract: dict) -> None:
    trace = contract["trace"]
    exact(trace["id"], "rpi5-ros2-demo-trace-v2", "trace.id")
    exact(trace["version"], 2, "trace.version")
    unique(trace["recordKinds"], "trace.recordKinds")
    exact(trace["recordKinds"], EXPECTED_RECORD_KINDS, "trace.recordKinds")

    workload = contract["workload"]
    expected = [
        "contract-admitted",
        "board-admitted",
        "generation-admitted",
        "session-open:publisher",
        "session-open:subscriber",
        "declaration-matched",
    ]
    for sample in workload["samples"]:
        sequence, value = sample["sequence"], sample["value"]
        expected.extend(
            (
                f"wire-put-sent:{sequence}",
                f"payload-decoded:{sequence}",
                f"subscriber-sample-validated:{sequence}:{value}",
            )
        )
    expected.extend(
        (
            f"session-closed:{workload['publishCount']}",
            "teardown-complete",
        )
    )
    for index, record in enumerate(trace["requiredSuccessSequence"]):
        kind = record.split(":", 1)[0]
        if kind not in trace["recordKinds"]:
            fail(f"trace.requiredSuccessSequence[{index}] uses undeclared kind {kind}")
    exact(trace["requiredSuccessSequence"], expected, "trace.requiredSuccessSequence")

    marker_values = list(contract["markers"].values())
    unique(marker_values, "markers")
    failure_markers = {
        name: marker for name, marker in contract["markers"].items() if name != "success"
    }
    if any(" success " in f" {marker} " for marker in failure_markers.values()):
        fail("markers failure value contains the success token")
    exact(contract["markers"], EXPECTED_MARKERS, "markers")


def validate(contract: dict) -> None:
    validate_transport_selection(contract)
    validate_identifiers(contract)
    validate_zenoh_profile(contract)
    validate_bounds(contract)
    validate_derived_wire_values(contract)
    validate_qos_ordering(contract)
    validate_workload(contract)
    validate_capabilities(contract)
    validate_trace_and_markers(contract)


def wildcard_capability(value: dict) -> None:
    value["capabilities"][-1]["object"] = "*:*."


def rejected(label: str, expected: str, mutate, validator=validate) -> None:
    candidate = copy.deepcopy(VALID)
    try:
        mutate(candidate)
    except (KeyError, TypeError, ValueError) as error:
        fail(f"{label} fixture mutation failed: {type(error).__name__}: {error}")
    try:
        validator(candidate)
    except SystemExit as error:
        message = str(error).removeprefix("rpi5 ROS 2 demo contract v2 check: ")
        if not message.startswith(expected):
            fail(f"{label} rejected for the wrong reason: {message}")
        return
    fail(f"{label} was accepted")


zutai("check", str(SCHEMA))
zutai("check", str(CHECK))
zutai("check", str(INVALID_CHECK))
invalid = zutai("run", str(INVALID_CHECK))
if not invalid.startswith("#invalid") or "formatVersion" not in invalid:
    fail("invalid fixture did not reject formatVersion structurally")
typed_fixture(FIXTURE)
VALID = load_fixture(FIXTURE)
validate(VALID)

first_projection = zutai("json", str(FIXTURE))
with tempfile.TemporaryDirectory(prefix="slime-rpi5-ros2-contract-v2-") as temporary:
    equivalent = _Path(temporary) / "equivalent.zti"
    equivalent.write_text(
        "\n\n" + FIXTURE.read_text(encoding="utf-8"),
        encoding="utf-8",
    )
    second_projection = zutai("json", str(equivalent))
if first_projection != second_projection:
    fail("equivalent fixture projections were not byte deterministic")

rejected(
    "targetProfile",
    "targetProfile must equal",
    lambda value: value.__setitem__("targetProfile", "aarch64-qemu-virt"),
)
rejected(
    "board.id",
    "board.id must equal",
    lambda value: value["board"].__setitem__("id", "raspberry-pi-5-nearby"),
)
rejected(
    "board.firmwareId",
    "board.firmwareId must equal",
    lambda value: value["board"].__setitem__("firmwareId", "rpi-eeprom-2712-latest"),
)
rejected(
    "ros.id",
    "ros.id must equal",
    lambda value: value["ros"].__setitem__("id", "ros2-profile-default"),
)
rejected(
    "ros.distribution",
    "ros.distribution must equal",
    lambda value: value["ros"].__setitem__("distribution", "jazzy-jalisco"),
)
rejected(
    "zenoh.id",
    "zenoh.id must equal",
    lambda value: value["zenoh"].__setitem__("id", "zenoh-profile-nearby"),
)
# An unrecognized transport family must fail closed rather than resolve to the
# only profile the fixture happens to carry.
rejected(
    "transport",
    "transport 'zenoh-next' is not an admitted family",
    lambda value: value.__setitem__("transport", "zenoh-next"),
)
# The discriminator must actually select: naming a family whose profile is
# absent is a rejection, not a fallback to the present one.
rejected(
    "transport/profile mismatch",
    "transport 'ddsi-rtps' names absent profile 'ddsiRtps'",
    lambda value: value.__setitem__("transport", "ddsi-rtps"),
)
# Two profiles present at once would make the wire format ambiguous.
rejected(
    "two transport profiles",
    "transport 'zenoh' carries unrelated 'ddsi-rtps' profile",
    lambda value: value.__setitem__("ddsiRtps", {"id": "ddsi-rtps-profile-0"}),
)
rejected(
    "zenoh.protocolVersion",
    "zenoh.protocolVersion must equal",
    lambda value: value["zenoh"].__setitem__("protocolVersion", 8),
)
rejected(
    "zenoh.routerRequired",
    "zenoh.routerRequired must be false",
    lambda value: value["zenoh"].__setitem__("routerRequired", True),
)
rejected(
    "zenoh.multicastScouting",
    "zenoh.multicastScouting must be false",
    lambda value: value["zenoh"].__setitem__("multicastScouting", True),
)
rejected(
    "zenoh.livelinessTokens",
    "zenoh.livelinessTokens must be false",
    lambda value: value["zenoh"].__setitem__("livelinessTokens", True),
)
# The type hash is derived, so a hand-edited digest cannot survive.
rejected(
    "workload.message.typeHash",
    "workload.message.typeHash must equal",
    lambda value: value["workload"]["message"].__setitem__(
        "typeHash", "RIHS01_" + "0" * 64
    ),
)
# So is the key expression it is embedded in.
rejected(
    "zenoh.dataKeyexpr",
    "zenoh.dataKeyexpr must equal",
    lambda value: value["zenoh"].__setitem__(
        "dataKeyexpr", "0/slime_demo/counter/Counter/RIHS01_" + "0" * 64
    ),
)
# The ROS slash form on the wire would not reach an rmw_zenoh peer.
rejected(
    "zenoh.typeNameOnWire",
    "zenoh.typeNameOnWire must equal",
    lambda value: value["zenoh"].__setitem__(
        "typeNameOnWire", "slime_demo_msgs/msg/Counter"
    ),
)
# A wildcard key expression would subscribe beyond the declared route.
rejected(
    "zenoh.dataKeyexpr wildcard",
    "zenoh.dataKeyexpr must equal",
    lambda value: value["zenoh"].__setitem__("dataKeyexpr", "0/slime_demo/**"),
)
# The attachment length is derived from the serializer's own rules; dropping the
# LEB128 length prefix a fixed-size array still carries is the exact off-by-one
# a naive implementation makes.
rejected(
    "zenoh.attachmentBytes",
    "zenoh.attachmentBytes must equal",
    lambda value: value["zenoh"].__setitem__("attachmentBytes", 32),
)
# So are the golden CDR bytes.
rejected(
    "workload.samples cdrHex",
    "workload.samples[0].cdrHex must equal",
    lambda value: value["workload"]["samples"][0].__setitem__(
        "cdrHex", "00000000000000000a000000"
    ),
)
rejected(
    "capabilities[12]",
    "capabilities[12] contains ambient or wildcard authority",
    wildcard_capability,
)
rejected(
    "discovery capability",
    "capability zenoh-router/gossip-relay grants router",
    lambda value: value["capabilities"].__setitem__(
        -1,
        {
            "holder": "slime-zenoh-profile-0",
            "kind": "zenoh-router",
            "object": "gossip-relay",
            "rights": ["connect"],
        },
    ),
)
rejected(
    "trace.requiredSuccessSequence",
    "trace.requiredSuccessSequence must equal",
    lambda value: value["trace"].__setitem__(
        "requiredSuccessSequence", value["trace"]["requiredSuccessSequence"][:-1]
    ),
)
rejected(
    "markers",
    "markers contains duplicates",
    lambda value: value["markers"].__setitem__("timeout", value["markers"]["denial"]),
)

print(
    "RP0 format-2 Raspberry Pi 5 / ROS 2 Kilted / bounded Zenoh contract passed: "
    "transport selection, derived RIHS01 type hash (validated against the upstream "
    "PointCloud2 fixture), derived key expression, derived CDR bytes, derived "
    "attachment length, bounds, authority inventory, trace, and rejection corpus"
)
