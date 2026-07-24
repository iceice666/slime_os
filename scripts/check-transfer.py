#!/usr/bin/env python3
from __future__ import annotations

import hashlib
import os
import shutil
from pathlib import Path

from harness import BOOT_TIMEOUT_SECONDS, RELEASE_KERNEL, ROOT, load_script, run_qemu

WORK = Path("/tmp/slime-os-transfer")
KERNEL = RELEASE_KERNEL
IMAGE = WORK / "receiver.img"
TRANSFER = WORK / "transfer.img"
BOOTSTORE = WORK / "boot-store.bin"
HEALTH_CONFIRM = WORK / "health-confirm.bin"
BOOTSTORE_TEMPLATE = WORK / "receiver-boot-store.bin"

RECEIVER_STORE = WORK / "receiver-store.bin"

CHECK = load_script("check_generation", "check-generation.py")
BUILD_GENERATION = load_script("build_generation", "build-generation.py")
BUILD_TRANSFER = load_script("build_transfer", "build-transfer.py")


def run(
    arguments: list[str],
    environment: dict[str, str] | None = None,
    cwd: Path = ROOT,
    timeout: int | None = BOOT_TIMEOUT_SECONDS,
) -> str:
    return run_qemu(
        arguments,
        environment=environment,
        cwd=cwd,
        timeout=timeout,
        echo="on-error",
    )


def extract_bootstore() -> bytes:
    run(["mcopy", "-o", "-i", str(IMAGE), "::/boot/boot-store.bin", str(BOOTSTORE)])
    return BOOTSTORE.read_bytes()


def release_from(image: bytes, identity: bytes) -> bytes:
    header = CHECK.BOOTSTORE_HEADER.unpack_from(image, CHECK.BOOTSTORE_DIRECTORY_OFFSET)
    for index in range(header[4]):
        entry = CHECK.BOOTSTORE_ENTRY.unpack_from(
            image,
            CHECK.BOOTSTORE_DIRECTORY_OFFSET
            + CHECK.BOOTSTORE_HEADER.size
            + index * CHECK.BOOTSTORE_ENTRY.size,
        )
        if entry[0] == identity:
            return image[entry[3] : entry[3] + entry[4]]
    raise SystemExit("source release missing")

def boot(transfer: bool, transfer_image: Path = TRANSFER) -> str:
    environment = os.environ.copy()
    environment["SLIME_BOOT_IMAGE"] = str(IMAGE)
    environment["SLIME_REUSE_BOOT_IMAGE"] = "1"
    if not transfer:
        environment.pop("SLIME_TRANSFER_RECEIVER", None)
        environment.pop("SLIME_GENERATION_CMD_CHECK", None)
        environment["SLIME_TRANSFER_NO_RECEIVER_DISK"] = "1"
        environment["SLIME_TRANSFER_NO_SOURCE_DISK"] = "1"
    environment["SLIME_QEMU_ACCEL"] = "tcg"
    return run(
        [
            "timeout",
            "60",
            str(ROOT / "kernel" / "scripts" / "run-kernel.sh"),
            str(KERNEL),
            "-display",
            "none",
            *(
                []
                if environment.get("SLIME_TRANSFER_NO_RECEIVER_DISK")
                else [
                    "-drive",
                    f"if=none,format=raw,cache=none,file={RECEIVER_STORE},id=receiver",
                    "-device",
                    (
                        "virtio-blk-pci,drive=receiver,disable-legacy=on,queue-size=8,"
                        "bus=pcie.0,addr=0x5"
                    ),
                ]
            ),
            *(
                []
                if environment.get("SLIME_TRANSFER_NO_SOURCE_DISK")
                else [
                    "-drive",
                    f"if=none,format=raw,readonly=on,file={transfer_image},id=transfer",
                    "-device",
                    (
                        "virtio-blk-pci,drive=transfer,disable-legacy=on,queue-size=8,"
                        "bus=pcie.0,addr=0x6"
                    ),
                ]
            ),
        ],
        environment,
    )


def main() -> None:
    shutil.rmtree(WORK, ignore_errors=True)
    WORK.mkdir(parents=True)
    for name in (
        "SLIME_PENDING_GENERATION",
        "SLIME_KNOWN_GOOD_FIRST",
        "SLIME_PENDING_RELEASE_SEQUENCE",
        "SLIME_ACCEPTED_RELEASE_SEQUENCE",
    ):
        os.environ.pop(name, None)
    environment = os.environ.copy()
    environment["SLIME_GENERATION_NUMBER"] = "9"
    environment["SLIME_GENERATION_CMD_SCENARIO"] = "transfer-receiver"
    environment["SLIME_TRANSFER_RECEIVER"] = "1"
    os.environ["SLIME_TRANSFER_RECEIVER"] = "1"
    os.environ["SLIME_GENERATION_CMD_SCENARIO"] = "transfer-receiver"
    run(["cargo", "build", "--release"], environment, ROOT / "kernel")
    generated = WORK / "generated"
    shutil.rmtree(
        ROOT / "components" / "target" / "generation-9-transfer-receiver",
        ignore_errors=True,
    )
    run(
        [str(ROOT / "scripts" / "build-generation.py"), str(KERNEL), str(generated)],
        environment,
    )
    generation1 = (generated / "generation-1.bin").read_bytes()
    receiver_store = BUILD_GENERATION.build_bootstore([generation1])
    receiver_state = CHECK.check_bootstore(receiver_store)["state"]
    source_environment = environment.copy()
    source_environment.pop("SLIME_TRANSFER_RECEIVER", None)
    source_environment.pop("SLIME_GENERATION_CMD_SCENARIO", None)
    source_environment["SLIME_TRANSFER_ACTIVATE"] = "1"
    source_environment["SLIME_GENERATION_PARENT"] = generation1[24:56].hex()
    source_generated = WORK / "source-generated"
    run(
        [str(ROOT / "scripts" / "build-generation.py"), str(KERNEL), str(source_generated)],
        source_environment,
    )
    generation2 = (source_generated / "generation-2.bin").read_bytes()
    full_store = (source_generated / "boot-store.bin").read_bytes()
    release = release_from(full_store, generation2[24:56])
    generation2_identity = generation2[24:56]
    bundle = BUILD_TRANSFER.build_bundle(
        generation1,
        generation2,
        release,
        receiver_state["state_root"],
    )
    if bundle != BUILD_TRANSFER.build_bundle(
        generation1,
        generation2,
        release,
        receiver_state["state_root"],
    ):
        raise SystemExit("transfer manifest was not deterministic")
    staged_root = hashlib.sha256(
        b"".join(sorted((generation1[24:56], generation2_identity)))
    ).digest()
    precommit_state = bytearray(receiver_store)
    precommit_state[512:1024] = BUILD_GENERATION.encode_bootstate(
        receiver_state["sequence"] + 1,
        receiver_state["known_good"],
        staged_root,
        pending=generation2_identity,
        accepted_release_sequence=receiver_state["accepted_release_sequence"],
        remaining_attempts=2,
        state_root=receiver_state["state_root"],
    )
    if CHECK.check_bootstore(bytes(precommit_state))["state"] != receiver_state:
        raise SystemExit("pre-directory transfer state did not fall back to known-good")
    committed_directory = bytearray(
        BUILD_GENERATION.build_bootstore([generation1, generation2])
    )
    committed_directory[:1024] = precommit_state[:1024]
    checksum_start = CHECK.BOOTSTORE_DIRECTORY_OFFSET + 48
    checksum_end = CHECK.BOOTSTORE_DIRECTORY_OFFSET + 80
    committed_directory[checksum_start:checksum_end] = bytes(32)
    checksum = BUILD_GENERATION.bootstore_checksum(committed_directory)
    committed_directory[checksum_start:checksum_end] = checksum
    committed = CHECK.check_bootstore(bytes(committed_directory))
    if committed["state"]["pending"] != generation2_identity:
        raise SystemExit("post-directory transfer state did not select pending generation")
    TRANSFER.write_bytes(bundle + bytes(32 * 1024 * 1024 - len(bundle)))

    environment["SLIME_GENERATION_DIR"] = str(generated)
    BOOTSTORE_TEMPLATE.write_bytes(receiver_store)
    environment["SLIME_BOOTSTORE_OVERRIDE"] = str(BOOTSTORE_TEMPLATE)
    RECEIVER_STORE.write_bytes(receiver_store)
    before_boot = CHECK.check_bootstore(RECEIVER_STORE.read_bytes())
    if len(before_boot["generations"]) != 1 or before_boot["state"]["pending"] is not None:
        raise SystemExit("receiver store was not initialized as known-good only")
    run(
        [str(ROOT / "kernel" / "scripts" / "build-iso.sh"), str(KERNEL), str(IMAGE), "64"],
        environment,
    )
    if RECEIVER_STORE.read_bytes() != receiver_store:
        raise SystemExit("receiver disk changed before QEMU boot")
    if extract_bootstore() != BOOTSTORE_TEMPLATE.read_bytes():
        raise SystemExit("boot image does not contain the known-good receiver store")
    malformed_transfers = []
    bad_closure = bytearray(bundle)
    object_offset = int.from_bytes(bad_closure[184:192], "little")
    bad_closure[object_offset] ^= 1
    bad_closure[248:280] = hashlib.sha256(
        bad_closure[:248] + bytes(32) + bad_closure[280:]
    ).digest()
    malformed_transfers.append(("bad-closure", bad_closure))
    bad_release = bytearray(bundle)
    release_offset = int.from_bytes(bad_release[200:208], "little")
    bad_release[release_offset] ^= 1
    bad_release[248:280] = hashlib.sha256(
        bad_release[:248] + bytes(32) + bad_release[280:]
    ).digest()
    malformed_transfers.append(("bad-release", bad_release))
    for name, malformed in malformed_transfers:
        malformed_path = WORK / f"{name}.img"
        malformed_path.write_bytes(
            bytes(malformed) + bytes(32 * 1024 * 1024 - len(malformed))
        )
        RECEIVER_STORE.write_bytes(receiver_store)
        image_before = extract_bootstore()
        disk_before = RECEIVER_STORE.read_bytes()
        try:
            boot(True, malformed_path)
        except SystemExit:
            pass
        else:
            raise SystemExit(f"{name} transfer unexpectedly succeeded")
        if extract_bootstore() != image_before or RECEIVER_STORE.read_bytes() != disk_before:
            raise SystemExit(f"{name} transfer changed receiver state")

    print(
        "receiver preboot:",
        [item["identity"].hex() for item in before_boot["generations"]],
        flush=True,
    )
    output = boot(True)
    for marker in ("[transfer] generation received", "[init] generation transfer installed"):
        if marker not in output:
            raise SystemExit(f"missing transfer marker: {marker}")
    run(["mcopy", "-o", "-i", str(IMAGE), str(RECEIVER_STORE), "::/boot/boot-store.bin"])
    installed = CHECK.check_bootstore(extract_bootstore())
    if installed["state"]["pending"] != generation2[24:56]:
        raise SystemExit("transferred generation was not selected pending")
    if installed["state"]["known_good"] != generation1[24:56]:
        raise SystemExit("known-good changed before health confirmation")
    first_disk = WORK / "receiver-after-install.bin"
    first_disk.write_bytes(RECEIVER_STORE.read_bytes())

    HEALTH_CONFIRM.write_bytes(generation2[24:56])
    run(["mcopy", "-o", "-i", str(IMAGE), str(HEALTH_CONFIRM), "::/boot/health-confirm.bin"])
    HEALTH_CONFIRM.unlink()
    receiver_before_activation = RECEIVER_STORE.read_bytes()
    output = boot(False)
    if RECEIVER_STORE.read_bytes() != receiver_before_activation:
        raise SystemExit("unnamed receiver disk changed during pending boot")
    promoted = CHECK.check_bootstore(extract_bootstore())
    if (
        promoted["state"]["known_good"] != generation2[24:56]
        or promoted["state"]["pending"] is not None
    ):
        raise SystemExit("transferred generation was not promoted")
    if generation1[24:56] not in {
        item["identity"] for item in promoted["generations"]
    }:
        raise SystemExit("rollback generation was not retained")
    replay_output = boot(False)
    replayed = CHECK.check_bootstore(extract_bootstore())
    if replayed["state"] != promoted["state"] or "action=promotion" in replay_output:
        raise SystemExit("consumed health confirmation was replayed")
    if "action=promotion commit=health-promotion" not in output:
        raise SystemExit("missing durable promotion trace")
    print(
        "generation transfer check: install, pending boot, promotion, "
        "and rollback retention passed"
    )


if __name__ == "__main__":
    main()
