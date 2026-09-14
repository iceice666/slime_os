#!/usr/bin/env python3

"""P6.6: the Framework's removable-media CPU boot, and its evidence.

This gate owns three things no emulated plane can own:

* **Writing.** It validates the P6.5 image and its identity, then drives the
  existing guarded writer onto one removable disk and re-reads the device to
  prove the bytes arrived. It never grows a second writer.
* **Protecting.** It digests a fixed region of the named internal device before
  and after the boots, with the page cache evicted, so the no-write claim rests
  on the disk rather than on a marker the root printed about itself.
* **Judging.** It validates the operator's observation record against the
  approved medium, the pinned target profile, and the panel lines actually
  rendered, and it refuses anything self-inconsistent.

It cannot *perform* the boots: the machine has to be power-cycled from the
medium, which is the operator's part. So the gate is split into subcommands —
`prepare` before the boots, `verify` after — and the milestone is closed by
`verify` passing over a record `prepare` made possible.

`controls` is the part that runs anywhere: it mutates a known-good record and
requires every mutation to be refused, which is what shows the judging has
teeth. `just framework_cpu_boot_check` runs `controls` plus whatever evidence is
committed, so the gate is meaningful on a machine that is not the Framework.
"""

from __future__ import annotations

import argparse
import copy
import json
import subprocess
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))

import boot_media_contract as media_contract  # noqa: E402
import cpu_boot_observation as observation  # noqa: E402
import cpu_boot_observation_contract as contract  # noqa: E402
from framework_media import validate_image  # noqa: E402
from harness import ROOT  # noqa: E402

IMAGE = ROOT / "build" / "framework-media" / "slime-framework.img"
MEDIA_RECORD = IMAGE.with_name(media_contract.RECORD_FILE_NAME)
SOURCE_MANIFEST = ROOT / "build" / "slime-sel4-graph-framework13-ai300.identity.json"
WRITER = ROOT / "scripts" / "build" / "write-removable-image.py"

#: Committed evidence, when a physical observation has been recorded.
EVIDENCE = ROOT / "evidence" / "framework-cpu-boot" / contract.RECORD_FILE_NAME


def fail(message: str) -> NoReturn:
    raise SystemExit(f"framework cpu boot: {message}")


def load_media_record() -> dict[str, object]:
    """The approved medium, revalidated rather than trusted.

    Every subcommand goes through this: a record that binds to an image whose
    bytes have since changed is not evidence about anything.
    """
    if not IMAGE.is_file():
        fail(f"missing boot media {IMAGE.relative_to(ROOT)}; run `just framework_media_image`")
    return validate_image(IMAGE, MEDIA_RECORD, fail=fail)


def generation_prefix() -> str:
    """The generation identity prefix the medium's root will render.

    Read from the build manifest the media identity already binds by digest, so
    this is the generation inside the approved image rather than whatever the
    tree would build today.
    """
    if not SOURCE_MANIFEST.is_file():
        fail(f"missing source manifest {SOURCE_MANIFEST.relative_to(ROOT)}")
    manifest = json.loads(SOURCE_MANIFEST.read_text(encoding="utf-8"))
    identity = manifest.get("generation", {}).get("identity")
    if not isinstance(identity, str) or len(identity) < 16:
        fail("source manifest carries no generation identity")
    return identity[:16].upper()


def command_prepare(arguments: argparse.Namespace) -> None:
    """Write the approved image and record the pre-boot protected digest."""
    media_record = load_media_record()
    protected = observation.describe_disk(arguments.protect, fail)
    if protected["removable"]:
        fail(
            f"{arguments.protect} is removable, so protecting it proves nothing "
            "about internal storage"
        )
    if arguments.protect == arguments.device:
        fail("the protected device and the boot medium must be different devices")

    before = observation.hash_protected_region(arguments.protect, fail)
    print(
        f"Protected region: {protected['devicePath']} "
        f"({protected['model']}, serial {protected['serial']}) "
        f"first {contract.COMPARISON_REGION_BYTES} bytes sha256:{before}"
    )

    # The existing guarded writer, not a second one: it owns the partition,
    # removable, read-only, and mounted refusals, the re-resolution after
    # confirmation, and the full-length read-back comparison.
    result = subprocess.run(
        [
            sys.executable,
            str(WRITER),
            str(IMAGE),
            str(arguments.device),
            "--identity",
            str(MEDIA_RECORD),
            *(["--yes"] if arguments.yes else []),
        ],
        cwd=ROOT,
        check=False,
    )
    if result.returncode != 0:
        fail(f"the guarded writer refused or failed with exit status {result.returncode}")

    target = observation.describe_disk(arguments.device, fail)
    image_identity = media_record["image"]
    assert isinstance(image_identity, dict)
    written = observation.hash_protected_region(
        arguments.device, fail, length=int(image_identity["bytes"])
    )
    if written != image_identity["sha256"]:
        fail("the removable device does not read back as the approved image")

    state = {
        "bootMediaIdentity": media_record["identity"],
        "bootMediaImageSha256": image_identity["sha256"],
        "generationIdentityPrefix": generation_prefix(),
        "writeTarget": {**target, "sha256": written},
        "protectedRegion": {
            "devicePath": protected["devicePath"],
            "model": protected["model"],
            "serial": protected["serial"],
            "regionBytes": contract.COMPARISON_REGION_BYTES,
            "sha256Before": before,
            "sha256After": "",
        },
    }
    arguments.state.parent.mkdir(parents=True, exist_ok=True)
    arguments.state.write_text(json.dumps(state, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"Wrote pre-boot state to {arguments.state}")
    print(
        "\nNow, without booting this host from the medium:\n"
        "  1. Power the Framework off completely.\n"
        "  2. Insert the USB device and cold-boot it, selecting the USB device\n"
        "     in the firmware boot menu. Do not type anything afterwards.\n"
        "  3. Record the panel lines, then power off and repeat once.\n"
        f"  4. Run this gate's `verify` with --state {arguments.state}.\n"
    )


def command_verify(arguments: argparse.Namespace) -> None:
    """Digest the protected region again and judge the observation record."""
    media_record = load_media_record()
    if not arguments.state.is_file():
        fail(f"missing pre-boot state {arguments.state}; run `prepare` first")
    state = json.loads(arguments.state.read_text(encoding="utf-8"))
    region = state["protectedRegion"]
    protect = Path(region["devicePath"])

    after = observation.hash_protected_region(protect, fail)
    print(f"Protected region after the boots: sha256:{after}")
    if after != region["sha256Before"]:
        fail(
            "the protected internal-storage region changed across the observed "
            f"boots: {region['sha256Before']} before, {after} after"
        )

    record = observation.build_record(
        operator=arguments.operator,
        observed_on=arguments.observed_on,
        media_record=media_record,
        write_target=state["writeTarget"],
        machine={
            "vendor": arguments.machine_vendor,
            "product": arguments.machine_product,
            "cpu": arguments.machine_cpu,
            "firmwareVendor": arguments.firmware_vendor,
            "firmwareVersion": arguments.firmware_version,
            "secureBoot": arguments.secure_boot,
        },
        protected_region={**region, "sha256After": after},
        boots=json.loads(arguments.boots.read_text(encoding="utf-8")),
    )
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    observation.validate_record(arguments.output, media_record, state["generationIdentityPrefix"], fail)
    print(f"Wrote and validated {arguments.output.relative_to(ROOT)}")


def expect_rejected(label: str, record: dict[str, object], prefix: str, media: dict[str, object]) -> None:
    """A mutated record must be refused, not merely reported."""

    def reject(message: str) -> NoReturn:
        raise ValueError(message)

    with tempfile.TemporaryDirectory(prefix="slime-cpu-boot-control-") as temporary:
        path = Path(temporary) / contract.RECORD_FILE_NAME
        path.write_text(json.dumps(record, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        try:
            observation.validate_record(path, media, prefix, reject)
        except ValueError:
            print(f"Framework CPU boot negative: {label} refused")
            return
        except SystemExit as error:
            fail(f"{label} was refused through the wrong path: {error}")
    fail(f"{label} was accepted; the observation gate has no teeth")


def synthetic_record(media_record: dict[str, object], prefix: str) -> dict[str, object]:
    """A known-good record over the approved medium, for the control cases.

    Synthetic on purpose: the controls must prove the validator rejects broken
    evidence, and they must run on a host that is not the Framework. This record
    is never written to `evidence/` and never satisfies the milestone — only a
    real `verify` run does.
    """
    image = media_record["image"]
    assert isinstance(image, dict)
    lines = [
        "SLIME OS",
        f"TARGET {contract.TARGET_PROFILE.upper()}",
        f"GENERATION 1 ID {prefix}",
        "DISPLAY 800X600X24",
        "ROOT READY",
        "IDLE - SAFE TO POWER OFF",
    ]
    boot = {
        "index": 0,
        "outcome": "ready",
        "coldStart": True,
        "keyboardUsed": False,
        "targetProfile": contract.TARGET_PROFILE,
        "generationNumber": 1,
        "generationIdentityPrefix": prefix,
        "displayWidth": 800,
        "displayHeight": 600,
        "displayBitsPerPixel": 24,
        "recordLines": lines,
        "panelSha256": "",
    }
    return observation.build_record(
        operator="control",
        observed_on="2026-09-13",
        media_record=media_record,
        write_target={
            "devicePath": "/dev/sdz",
            "model": "CONTROL",
            "serial": "CONTROL0",
            "sizeBytes": int(image["bytes"]),
            "removable": True,
            "sha256": str(image["sha256"]),
        },
        machine={
            "vendor": "CONTROL",
            "product": "CONTROL",
            "cpu": "CONTROL",
            "firmwareVendor": "CONTROL",
            "firmwareVersion": "control",
            "secureBoot": False,
        },
        protected_region={
            "devicePath": "/dev/nvme0n1",
            "model": "CONTROL NVME",
            "serial": "CONTROLNVME",
            "regionBytes": contract.COMPARISON_REGION_BYTES,
            "sha256Before": "0" * 64,
            "sha256After": "0" * 64,
        },
        boots=[boot, {**boot, "index": 1}],
    )


def restamp(record: dict[str, object]) -> dict[str, object]:
    """Re-stamp a mutated record's identity.

    Without this the controls would all fail on the identity digest, and would
    prove only that the digest works — not that each individual invariant is
    checked. A forger with the tooling can re-stamp too, which is exactly the
    adversary each case below has to be refused against.
    """
    record["identity"] = observation.record_identity(record)
    return record


def command_controls(_: argparse.Namespace) -> None:
    media_record = load_media_record()
    prefix = generation_prefix()
    good = synthetic_record(media_record, prefix)

    # The known-good record must pass, or every refusal below is vacuous.
    with tempfile.TemporaryDirectory(prefix="slime-cpu-boot-good-") as temporary:
        path = Path(temporary) / contract.RECORD_FILE_NAME
        path.write_text(json.dumps(good, indent=2, sort_keys=True) + "\n", encoding="utf-8")
        observation.validate_record(path, media_record, prefix, fail)
    print("Framework CPU boot control: the reference record is accepted")

    cases: list[tuple[str, dict[str, object]]] = []

    def mutate(label: str, apply: Callable[[dict[str, object]], object]) -> None:
        candidate = copy.deepcopy(good)
        apply(candidate)
        cases.append((label, restamp(candidate)))

    mutate("one boot", lambda r: r["boots"].pop())
    mutate("three boots", lambda r: r["boots"].append({**r["boots"][0], "index": 2}))
    mutate("a boot that did not reach ready", lambda r: r["boots"][1].update(outcome="no-boot"))
    mutate("a warm restart", lambda r: r["boots"][1].update(coldStart=False))
    mutate("keyboard input", lambda r: r["boots"][0].update(keyboardUsed=True))
    mutate(
        "a wrong-profile boot",
        lambda r: r["boots"][0].update(targetProfile="x86_64-sel4-qemu-pc99"),
    )
    mutate(
        "a panel that names another profile",
        lambda r: r["boots"][0].update(
            recordLines=["SLIME OS", "TARGET X86-64-SEL4-QEMU-PC99", f"GENERATION 1 ID {prefix}"]
        ),
    )
    mutate(
        "a panel that does not show the recorded generation",
        lambda r: r["boots"][0].update(
            recordLines=["SLIME OS", f"TARGET {contract.TARGET_PROFILE.upper()}", "ROOT READY"]
        ),
    )
    mutate(
        "a generation the medium does not carry",
        lambda r: [
            b.update(
                generationIdentityPrefix="DEADBEEFDEADBEEF",
                recordLines=[
                    "SLIME OS",
                    f"TARGET {contract.TARGET_PROFILE.upper()}",
                    "GENERATION 1 ID DEADBEEFDEADBEEF",
                ],
            )
            for b in r["boots"]
        ],
    )
    mutate("a changed protected region", lambda r: r["protectedRegion"].update(sha256After="1" * 64))
    mutate(
        "a protected region of the wrong size",
        lambda r: r["protectedRegion"].update(regionBytes=4096),
    )
    mutate(
        "a protected region that is the boot medium",
        lambda r: r["protectedRegion"].update(devicePath=r["writeTarget"]["devicePath"]),
    )
    mutate(
        "a non-removable write target",
        lambda r: r["writeTarget"].update(removable=False),
    )
    mutate(
        "a write target carrying other bytes",
        lambda r: r["writeTarget"].update(sha256="2" * 64),
    )
    mutate("another medium's identity", lambda r: r.update(bootMediaIdentity="3" * 64))
    mutate("another image digest", lambda r: r.update(bootMediaImageSha256="4" * 64))
    mutate("a wrong record target profile", lambda r: r.update(targetProfile="x86_64-sel4-qemu-pc99"))
    mutate("an unknown field", lambda r: r.update(emulated=True))

    for label, candidate in cases:
        expect_rejected(label, candidate, prefix, media_record)

    # A tampered record whose identity was *not* re-stamped must also fail,
    # which is the cheap forgery the digest exists to catch.

    unstamped = copy.deepcopy(good)
    unstamped["operator"] = "someone else"
    expect_rejected("an unstamped edit", unstamped, prefix, media_record)

    print(
        f"Framework CPU boot controls: {len(cases) + 1} mutations refused over "
        "the approved medium"
    )


def command_check(_: argparse.Namespace) -> None:
    """The public gate: the controls always, the physical claim when recorded."""
    command_controls(_)
    media_record = load_media_record()
    if not EVIDENCE.is_file():
        print(
            "\nframework_cpu_boot_check: no physical observation is recorded yet.\n"
            f"  P6.6 is OPEN. The validator is proven, the medium is approved, and\n"
            f"  {EVIDENCE.relative_to(ROOT)} does not exist.\n"
            "  Record one with `prepare` and `verify`; QEMU cannot close this gate."
        )
        return
    observation.validate_record(EVIDENCE, media_record, generation_prefix(), fail)
    record = json.loads(EVIDENCE.read_text(encoding="utf-8"))
    machine = record["machine"]
    print(
        "framework_cpu_boot_check: "
        f"{len(record['boots'])} cold boots of image {record['bootMediaImageSha256'][:16]}… "
        f"observed on {machine['vendor']} {machine['product']} "
        f"(firmware {machine['firmwareVersion']}), "
        f"internal region {record['protectedRegion']['sha256Before'][:16]}… unchanged"
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=False)

    prepare = sub.add_parser("prepare", help="write the approved image and hash internal storage")
    prepare.add_argument("--device", type=Path, required=True, help="removable whole disk")
    prepare.add_argument("--protect", type=Path, required=True, help="internal disk to protect")
    prepare.add_argument("--state", type=Path, default=ROOT / "build" / "framework-cpu-boot.state.json")
    prepare.add_argument("--yes", action="store_true", help="confirm the destructive write")
    prepare.set_defaults(handler=command_prepare)

    verify = sub.add_parser("verify", help="hash internal storage again and judge the record")
    verify.add_argument("--state", type=Path, default=ROOT / "build" / "framework-cpu-boot.state.json")
    verify.add_argument("--boots", type=Path, required=True, help="JSON array of observations")
    verify.add_argument("--operator", required=True)
    verify.add_argument("--observed-on", required=True)
    verify.add_argument("--machine-vendor", required=True)
    verify.add_argument("--machine-product", required=True)
    verify.add_argument("--machine-cpu", required=True)
    verify.add_argument("--firmware-vendor", required=True)
    verify.add_argument("--firmware-version", required=True)
    verify.add_argument("--secure-boot", action="store_true")
    verify.add_argument("--output", type=Path, default=EVIDENCE)
    verify.set_defaults(handler=command_verify)

    controls = sub.add_parser("controls", help="prove the observation validator refuses bad evidence")
    controls.set_defaults(handler=command_controls)

    arguments = parser.parse_args()
    handler = getattr(arguments, "handler", command_check)
    handler(arguments)


if __name__ == "__main__":
    main()
