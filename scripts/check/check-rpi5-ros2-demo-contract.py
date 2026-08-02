#!/usr/bin/env python3

"""RP0 target-qualified Raspberry Pi 5 ROS 2 demo contract gate."""

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import copy
import hashlib
import json
import os
import subprocess
import tempfile
from typing import NoReturn

from harness import ROOT
from zutai_cli import STDLIB, binary

CONTRACT = ROOT / "contracts" / "rpi5-ros2-demo" / "v1"
SCHEMA = CONTRACT / "schema.zt"
CHECK = CONTRACT / "check.zt"
INVALID_CHECK = CONTRACT / "check-invalid.zt"
FIXTURE = CONTRACT / "fixtures" / "valid.zti"

EXPECTED_CONTRACT_ID = "rpi5-ros2-demo-v1"
EXPECTED_TARGET = "aarch64-rpi5"
EXPECTED_BOARD = "raspberry-pi-5-model-b-rev1"
EXPECTED_REVISIONS = ["c04170", "d04170"]
EXPECTED_FIRMWARE = "rpi-eeprom-2712-2026-05-27-default"
EXPECTED_ROS_PROFILE = "ros2-profile-0-jazzy-native"
EXPECTED_DDS_PROFILE = "dds-profile-0-static-local-reliable"
EXPECTED_MESSAGE_TYPE = "slime_demo_msgs/msg/Counter"
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
    "maxTextBytes": 128,
    "maxSequenceItems": 16,
    "maxMessageBytes": 12,
    "maxXcdrBytes": 12,
    "maxRtpsMessageBytes": 512,
    "maxRtpsSubmessages": 4,
    "maxParameterListBytes": 128,
    "maxFragmentBytes": 0,
    "maxFragmentsPerSample": 0,
    "maxQueueDepth": 4,
    "maxHistoryDepth": 4,
    "maxParticipants": 2,
    "maxWriters": 1,
    "maxReaders": 1,
    "maxOutstandingSamples": 4,
    "maxRetries": 3,
    "maxTraceRecords": 32,
    "maxTraceRecordBytes": 192,
    "maxTraceBytes": 6144,
    "maxSerialLineBytes": 256,
    "maxLogBytes": 8192,
    "maxCapabilityGrants": 13,
}
EXPECTED_LOCATORS = {
    "publisher-data": ("127.0.0.1", 7410, "send"),
    "subscriber-data": ("127.0.0.1", 7411, "receive"),
}
EXPECTED_SUBMESSAGES = ["INFO_DST", "DATA", "HEARTBEAT", "ACKNACK"]
EXPECTED_RECORD_KINDS = [
    "contract-admitted",
    "board-admitted",
    "generation-admitted",
    "participant-ready",
    "endpoint-matched",
    "rtps-data-sent",
    "rtps-heartbeat-sent",
    "rtps-acknack-received",
    "xcdr-sample-decoded",
    "subscriber-sample-validated",
    "teardown-complete",
    "failure",
]
EXPECTED_MARKERS = {
    "success": "[rpi5-ros2-demo] success profile=rpi5-ros2-demo-v1 samples=4",
    "denial": "[rpi5-ros2-demo] failure class=denied",
    "timeout": "[rpi5-ros2-demo] failure class=timeout",
    "wrongBoard": "[rpi5-ros2-demo] failure class=wrong-board",
    "wrongTarget": "[rpi5-ros2-demo] failure class=wrong-target",
    "malformedRtps": "[rpi5-ros2-demo] failure class=malformed-rtps",
    "malformedXcdr": "[rpi5-ros2-demo] failure class=malformed-xcdr",
    "malformedGeneration": "[rpi5-ros2-demo] failure class=malformed-generation",
}
EXPECTED_CAPABILITIES = {
    ("ros2-demo-publisher", "dds-writer", "counter-writer", ("publish",)),
    (
        "ros2-demo-publisher",
        "clock",
        "demo-monotonic-clock",
        ("read", "wait"),
    ),
    ("ros2-demo-publisher", "log-sink", "demo-serial-log", ("write",)),
    ("ros2-demo-subscriber", "dds-reader", "counter-reader", ("subscribe",)),
    (
        "ros2-demo-subscriber",
        "clock",
        "demo-monotonic-clock",
        ("read", "wait"),
    ),
    ("ros2-demo-subscriber", "log-sink", "demo-serial-log", ("write",)),
    (
        "slime-dds-profile-0",
        "datagram-send",
        "127.0.0.1:7410-to-127.0.0.1:7411",
        ("send",),
    ),
    (
        "slime-dds-profile-0",
        "datagram-receive",
        "127.0.0.1:7411",
        ("receive",),
    ),
    (
        "slime-dds-profile-0",
        "participant",
        "publisher-participant",
        ("announce-static", "write"),
    ),
    (
        "slime-dds-profile-0",
        "participant",
        "subscriber-participant",
        ("announce-static", "read"),
    ),
    (
        "slime-dds-profile-0",
        "trace-sink",
        "rpi5-ros2-demo-trace",
        ("append", "seal"),
    ),
    ("rpi5-board-service", "device", "uart10-pl011", ("configure", "write")),
    ("rpi5-board-service", "storage", "boot-microsd", ("read",)),
}


def fail(message: str) -> NoReturn:
    raise SystemExit(f"rpi5 ROS 2 demo contract check: {message}")


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


def validate_identifiers(contract: dict) -> None:
    exact(contract["formatVersion"], 1, "formatVersion")
    exact(contract["id"], EXPECTED_CONTRACT_ID, "id")
    exact(contract["targetProfile"], EXPECTED_TARGET, "targetProfile")

    board = contract["board"]
    exact(board["id"], EXPECTED_BOARD, "board.id")
    exact(board["model"], "Raspberry Pi 5 Model B", "board.model")
    exact(
        board["acceptedRevisionCodes"],
        EXPECTED_REVISIONS,
        "board.acceptedRevisionCodes",
    )
    exact(board["soc"], "bcm2712c1", "board.soc")
    exact(board["architecture"], "aarch64", "board.architecture")
    exact(board["abi"], "slime-aarch64-v1", "board.abi")
    exact(board["pageProfile"], "aarch64-4k", "board.pageProfile")
    exact(board["firmwareId"], EXPECTED_FIRMWARE, "board.firmwareId")
    exact(
        board["bootFlow"],
        "bcm2712-rom-to-spi-eeprom-to-fat32",
        "board.bootFlow",
    )
    exact(board["kernelPath"], "kernel8.img", "board.kernelPath")
    exact(board["configPath"], "config.txt", "board.configPath")
    exact(
        board["deviceTreePath"],
        "bcm2712-rpi-5-b.dtb",
        "board.deviceTreePath",
    )
    exact(board["mediaKind"], "removable-microsd", "board.mediaKind")
    exact(
        board["mediaLayout"],
        "mbr-fat32-boot-plus-slime-generation-store",
        "board.mediaLayout",
    )
    exact(
        board["memoryMapSource"],
        "firmware-final-device-tree",
        "board.memoryMapSource",
    )
    exact(
        board["interruptController"],
        "arm-gic-400-gicv2-from-device-tree",
        "board.interruptController",
    )
    exact(
        board["genericTimer"],
        "arm-armv8-timer-from-device-tree",
        "board.genericTimer",
    )
    exact(
        board["serialPath"],
        "uart10-debug-header-pl011-0x107d001000",
        "board.serialPath",
    )
    exact(board["serialBaud"], 115200, "board.serialBaud")
    exact(board["requiredDevices"], EXPECTED_BOARD_DEVICES, "board.requiredDevices")

    ros = contract["ros"]
    exact(ros["id"], EXPECTED_ROS_PROFILE, "ros.id")
    exact(ros["distribution"], "jazzy-jalisco", "ros.distribution")
    exact(ros["apiSubset"], EXPECTED_ROS_API, "ros.apiSubset")
    exact(
        ros["nodeRoute"],
        "slime-native-ros-compatible-components",
        "ros.nodeRoute",
    )
    exact(
        ros["rmwBoundary"],
        "slime-rmw-profile-0-over-bounded-ddsi-rtps",
        "ros.rmwBoundary",
    )
    exact(ros["ddsRuntime"], "slime-dds-profile-0", "ros.ddsRuntime")
    exact(ros["ddsSecurity"], False, "ros.ddsSecurity")

    dds = contract["dds"]
    exact(dds["id"], EXPECTED_DDS_PROFILE, "dds.id")
    exact(dds["rtpsVersion"], "2.5", "dds.rtpsVersion")
    exact(dds["representation"], "xcdr1-cdr-le", "dds.representation")
    exact(dds["endianness"], "little", "dds.endianness")
    exact(dds["domainId"], 0, "dds.domainId")
    exact(
        dds["discoveryMode"],
        "static-generation-declared",
        "dds.discoveryMode",
    )
    exact(
        dds["publisherParticipantGuidPrefix"],
        "534c494d4500000000000001",
        "dds.publisherParticipantGuidPrefix",
    )
    exact(
        dds["subscriberParticipantGuidPrefix"],
        "534c494d4500000000000002",
        "dds.subscriberParticipantGuidPrefix",
    )
    exact(dds["writerEntityId"], "00000103", "dds.writerEntityId")
    exact(dds["readerEntityId"], "00000104", "dds.readerEntityId")
    exact(dds["requiredSubmessages"], EXPECTED_SUBMESSAGES, "dds.requiredSubmessages")
    exact(dds["history"], "keep-last", "dds.history")
    exact(dds["historyDepth"], 4, "dds.historyDepth")
    exact(dds["reliability"], "reliable", "dds.reliability")
    exact(dds["durability"], "volatile", "dds.durability")
    exact(dds["liveliness"], "automatic", "dds.liveliness")
    exact(dds["deadlineNs"], 1_000_000_000, "dds.deadlineNs")
    exact(dds["lifespanNs"], 5_000_000_000, "dds.lifespanNs")
    exact(dds["leaseNs"], 2_000_000_000, "dds.leaseNs")
    exact(dds["heartbeatPeriodNs"], 100_000_000, "dds.heartbeatPeriodNs")
    exact(dds["ackTimeoutNs"], 250_000_000, "dds.ackTimeoutNs")
    exact(dds["retryLimit"], 3, "dds.retryLimit")
    exact(dds["fragmentSizeBytes"], 0, "dds.fragmentSizeBytes")
    exact(
        dds["teardown"],
        "final-heartbeat-ack-then-dispose-endpoints-and-participants",
        "dds.teardown",
    )


def validate_retry_bound(contract: dict) -> None:
    if contract["dds"]["retryLimit"] > contract["bounds"]["maxRetries"]:
        fail("bounds.maxRetries is below dds.retryLimit")


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
    if bounds["maxMessageBytes"] > bounds["maxXcdrBytes"]:
        fail("maxMessageBytes exceeds maxXcdrBytes")
    if bounds["maxXcdrBytes"] > bounds["maxRtpsMessageBytes"]:
        fail("maxXcdrBytes exceeds maxRtpsMessageBytes")
    if len(contract["dds"]["publisherParticipantGuidPrefix"]) != 24:
        fail("DDS publisher participant GUID prefix is not 12 octets")
    if len(contract["dds"]["subscriberParticipantGuidPrefix"]) != 24:
        fail("DDS subscriber participant GUID prefix is not 12 octets")
    exact(bounds["maxParticipants"], 2, "bounds.maxParticipants")
    exact(bounds["maxWriters"], 1, "bounds.maxWriters")
    exact(bounds["maxReaders"], 1, "bounds.maxReaders")
    if workload["publishCount"] > bounds["maxOutstandingSamples"]:
        fail("workload.publishCount exceeds maxOutstandingSamples")
    validate_retry_bound(contract)
    if len(contract["dds"]["requiredSubmessages"]) > bounds["maxRtpsSubmessages"]:
        fail("required RTPS submessages exceed maxRtpsSubmessages")
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
    if contract["dds"]["fragmentSizeBytes"] != bounds["maxFragmentBytes"]:
        fail("DDS fragmentSizeBytes must equal the admitted fragment ceiling")
    if bounds["maxFragmentBytes"] == 0 and bounds["maxFragmentsPerSample"] != 0:
        fail("fragment count must be zero when fragmentation is disabled")
    dds = contract["dds"]
    for name in (
        "deadlineNs",
        "lifespanNs",
        "leaseNs",
        "heartbeatPeriodNs",
        "ackTimeoutNs",
    ):
        positive(dds[name], f"dds.{name}")
    if dds["heartbeatPeriodNs"] > dds["ackTimeoutNs"]:
        fail("dds.heartbeatPeriodNs exceeds dds.ackTimeoutNs")
    if dds["ackTimeoutNs"] > dds["deadlineNs"]:
        fail("dds.ackTimeoutNs exceeds dds.deadlineNs")
    if dds["heartbeatPeriodNs"] >= dds["leaseNs"]:
        fail("dds.heartbeatPeriodNs must be below dds.leaseNs")
    if workload["publishPeriodNs"] > dds["deadlineNs"]:
        fail("workload.publishPeriodNs exceeds dds.deadlineNs")


def validate_dds_and_workload(contract: dict) -> None:
    dds = contract["dds"]
    prefixes = [
        dds["publisherParticipantGuidPrefix"],
        dds["subscriberParticipantGuidPrefix"],
    ]
    unique(prefixes, "DDS participant GUID prefixes")
    for path, value in (
        ("dds.publisherParticipantGuidPrefix", prefixes[0]),
        ("dds.subscriberParticipantGuidPrefix", prefixes[1]),
    ):
        if len(value) != 24 or any(character not in "0123456789abcdef" for character in value):
            fail(f"{path} must be 12 lowercase hexadecimal octets")
    for path in ("writerEntityId", "readerEntityId"):
        value = dds[path]
        if len(value) != 8 or any(character not in "0123456789abcdef" for character in value):
            fail(f"dds.{path} must be 4 lowercase hexadecimal octets")

    locator_rows = {}
    for locator in dds["locators"]:
        name = locator["name"]
        if name in locator_rows:
            fail(f"duplicate locator {name}")
        locator_rows[name] = (locator["address"], locator["port"], locator["direction"])
        if locator["address"] in {"0.0.0.0", "::", "*"} or "*" in locator["address"]:
            fail(f"locator {name} grants a wildcard destination")
        if not 1 <= locator["port"] <= 65535:
            fail(f"locator {name} has an invalid port")
    exact(locator_rows, EXPECTED_LOCATORS, "dds.locators")
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
    expected_count = workload["publishCount"]
    exact(
        workload["expectedSequences"],
        list(range(expected_count)),
        "workload.expectedSequences",
    )
    exact(workload["publishCount"], 4, "workload.publishCount")
    exact(workload["publishPeriodNs"], 250_000_000, "workload.publishPeriodNs")
    exact(workload["expectedValues"], [10, 20, 30, 40], "workload.expectedValues")
    exact(
        workload["expectedSubscriberLine"],
        "[rpi5-ros2-demo] received count=4 sequences=0,1,2,3 values=10,20,30,40",
        "workload.expectedSubscriberLine",
    )
    subscriber_line_bytes = len(workload["expectedSubscriberLine"].encode("utf-8"))
    if subscriber_line_bytes > contract["bounds"]["maxSerialLineBytes"]:
        fail("workload.expectedSubscriberLine exceeds maxSerialLineBytes")


def validate_capabilities(contract: dict) -> None:
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
    exact(trace["id"], "rpi5-ros2-demo-trace-v1", "trace.id")
    exact(trace["version"], 1, "trace.version")
    unique(trace["recordKinds"], "trace.recordKinds")
    exact(trace["recordKinds"], EXPECTED_RECORD_KINDS, "trace.recordKinds")

    expected = [
        "contract-admitted",
        "board-admitted",
        "generation-admitted",
        "participant-ready:publisher",
        "participant-ready:subscriber",
        "endpoint-matched",
    ]
    for sequence, value in zip(
        contract["workload"]["expectedSequences"],
        contract["workload"]["expectedValues"],
        strict=True,
    ):
        expected.extend(
            (
                f"rtps-data-sent:{sequence}",
                f"xcdr-sample-decoded:{sequence}",
                f"subscriber-sample-validated:{sequence}:{value}",
            )
        )
    expected.extend(
        (
            f"rtps-heartbeat-sent:{contract['workload']['publishCount']}",
            f"rtps-acknack-received:{contract['workload']['publishCount']}",
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
    validate_identifiers(contract)
    validate_bounds(contract)
    validate_dds_and_workload(contract)
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
        message = str(error).removeprefix("rpi5 ROS 2 demo contract check: ")
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
with tempfile.TemporaryDirectory(prefix="slime-rpi5-ros2-contract-") as temporary:
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
    "dds.id",
    "dds.id must equal",
    lambda value: value["dds"].__setitem__("id", "dds-profile-nearby"),
)
rejected(
    "capabilities[12]",
    "capabilities[12] contains ambient or wildcard authority",
    wildcard_capability,
)
rejected(
    "bounds.maxRetries",
    "bounds.maxRetries is below dds.retryLimit",
    lambda value: value["bounds"].__setitem__("maxRetries", 2),
    validate_retry_bound,
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
    "RP0 Raspberry Pi 5 / ROS 2 Jazzy / DDSI-RTPS 2.5 contract, bounds, "
    "authority inventory, trace, and rejection corpus passed"
)
