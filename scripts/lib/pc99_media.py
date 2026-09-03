"""Shared pc99 boot media: the GRUB Multiboot2 EFI tree and its QEMU command.

P6.2's boot contract lives here rather than in one gate, because the file tree
that boots under QEMU/OVMF is the same tree P6.5 writes to removable media. A
second assembler would let the emulator prove one layout while the medium
carried another.

seL4 pc99 is on seL4's own Multiboot2 route: the kernel and root task stay
separate modules a bootloader supplies, so there is no packaged image to hash
the way the rust-sel4 loader platforms have one. What plays that role is the
tree digest — a path-sensitive digest over every file the boot reads — which is
what `boot_media` records and gates compare.

Nothing is loaded from the medium at run time. `BOOTX64.EFI` is built
standalone from the pinned module list with the search-and-configfile script
embedded, so the bootloader cannot acquire behavior the identity manifest does
not name.
"""

from __future__ import annotations

import hashlib
import os
import shutil
import subprocess
from collections.abc import Callable, Sequence
from pathlib import Path
from typing import NoReturn

ROOT = Path(__file__).resolve().parents[2]

PINS_SECTION = "qemu_pc99"
BOOT_PINS_SECTION = "qemu_pc99_boot"

# The bootloader locates its own configuration by searching for the kernel
# module rather than trusting a partition index or a UUID: the same tree is read
# through QEMU's synthetic FAT, a GPT/ESP raw image, and later a USB device, and
# those present different partition numbering. `search --file` is stable across
# all three.
#
# `configfile` is what runs the menu; `set prefix` first so relative module
# loads inside the configuration resolve on the medium.
EARLY_CONFIG = """\
search --file --no-floppy --set=root /{kernel_module}
set prefix=($root)/boot/grub
configfile ($root)/{grub_config}
"""

# One bounded entry with no timeout and no interactive path. `serial` plus
# `terminal_input`/`terminal_output` move the console to COM1 before the kernel
# starts, which is the only evidence channel a gate has; `gfxpayload=text`
# keeps GRUB from negotiating a video mode the machine may not offer, which
# otherwise fails with "no suitable video mode found" and leaves the kernel
# without a console.
GRUB_CONFIG = """\
set timeout=0
set default=0
serial --unit=0 --speed={serial_baud} --word=8 --parity=no --stop=1
terminal_output serial
terminal_input serial
set gfxpayload=text
menuentry "slime" {{
    multiboot2 /{kernel_module}
    module2 /{root_module} slime-root
    boot
}}
"""


def profile_strings(
    profile: dict[str, object],
    key: str,
    fail: Callable[[str], NoReturn],
    section: str = PINS_SECTION,
) -> tuple[str, ...]:
    value = profile.get(key)
    if not isinstance(value, list) or not value:
        fail(f"sel4/pins.toml [{section}].{key} must be a non-empty array")
    for entry in value:
        if not isinstance(entry, str) or not entry:
            fail(f"sel4/pins.toml [{section}].{key} must contain only non-empty strings")
    return tuple(value)


def _sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def _require_tool(directory: Path, name: str, fail: Callable[[str], NoReturn]) -> Path:
    path = directory / "bin" / name
    if not path.is_file():
        fail(f"{name} is not present in {directory}; enter `nix develop`")
    return path


def grub_prefix(fail: Callable[[str], NoReturn]) -> Path:
    """The exact GRUB build named by the development shell.

    Resolved from `SLIME_GRUB_PREFIX` rather than `PATH` for the same reason
    `X86_64_COMPILER_PREFIX` is: the pinned module digest describes one build's
    modules, and whichever `grub-mkimage` a host happens to expose is not
    necessarily that one.
    """
    value = os.environ.get("SLIME_GRUB_PREFIX")
    if not value:
        fail("SLIME_GRUB_PREFIX is unset; enter `nix develop` so GRUB is the pinned build")
    prefix = Path(value)
    if not prefix.is_dir():
        fail(f"SLIME_GRUB_PREFIX does not name a directory: {value}")
    return prefix


def ovmf_directory(fail: Callable[[str], NoReturn]) -> Path:
    """The exact OVMF firmware volume directory named by the development shell."""
    value = os.environ.get("SLIME_OVMF_DIR")
    if not value:
        fail("SLIME_OVMF_DIR is unset; enter `nix develop` so OVMF is the pinned build")
    directory = Path(value)
    if not directory.is_dir():
        fail(f"SLIME_OVMF_DIR does not name a directory: {value}")
    return directory


def grub_module_digest(
    modules: Sequence[str],
    fail: Callable[[str], NoReturn],
    prefix: Path | None = None,
) -> str:
    """A digest over the pinned GRUB module list, in declaration order.

    The produced `BOOTX64.EFI` is not what gets pinned: `grub-mkimage` embeds
    build-derived data, so its digest describes a GRUB build rather than the
    behavior linked into it. The module bytes are exactly that behavior.

    Order-sensitive because `grub-mkimage` links in argument order and
    `assemble_media` passes this same list unsorted, so a reordering produces a
    different `BOOTX64.EFI`. A digest over the sorted set would be blind to
    exactly that edit.
    """
    directory = (prefix or grub_prefix(fail)) / "lib" / "grub" / "x86_64-efi"
    if not directory.is_dir():
        fail(
            f"{directory} is missing; SLIME_GRUB_PREFIX must name an EFI-format "
            "GRUB build (grub2_efi), not the BIOS-format one"
        )
    digest = hashlib.sha256()
    for module in modules:
        path = directory / f"{module}.mod"
        if not path.is_file():
            fail(f"pinned GRUB module {module!r} is absent from {directory}")
        digest.update(module.encode("utf-8"))
        digest.update(len(module).to_bytes(2, "little"))
        digest.update(path.read_bytes())
    return digest.hexdigest()


def verify_boot_inputs(
    boot_pins: dict[str, object],
    fail: Callable[[str], NoReturn],
    profile: dict[str, object],
) -> dict[str, str]:
    """Refuse a firmware or bootloader that is not the pinned one.

    Checked before assembling anything: an unpinned firmware or module set
    makes a boot claim describe an artifact the identity manifest does not name,
    which is worse than not booting.
    """
    from harness import profile_text  # noqa: PLC0415 — avoids a lib import cycle

    firmware = ovmf_directory(fail)
    records: dict[str, str] = {}
    for key, filename in (("firmware_code_sha256", "OVMF_CODE.fd"), ("firmware_vars_sha256", "OVMF_VARS.fd")):
        path = firmware / filename
        if not path.is_file():
            fail(f"pinned firmware file {filename} is absent from {firmware}")
        expected = profile_text(boot_pins, key, fail, BOOT_PINS_SECTION)
        actual = _sha256_bytes(path.read_bytes())
        if actual != expected:
            fail(
                f"{filename} SHA-256 is {actual}, but sel4/pins.toml "
                f"[{BOOT_PINS_SECTION}].{key} pins {expected}"
            )
        records[filename] = actual

    prefix = grub_prefix(fail)
    version_output = subprocess.run(
        [str(_require_tool(prefix, "grub-mkimage", fail)), "--version"],
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    ).stdout
    expected_version = profile_text(boot_pins, "bootloader_version", fail, BOOT_PINS_SECTION)
    if f"(GRUB) {expected_version}" not in version_output:
        fail(
            f"grub-mkimage reports {version_output.strip()!r}, but sel4/pins.toml "
            f"[{BOOT_PINS_SECTION}].bootloader_version pins {expected_version}"
        )

    modules = profile_strings(profile, "grub_modules", fail)
    expected_modules = profile_text(boot_pins, "grub_modules_sha256", fail, BOOT_PINS_SECTION)
    actual_modules = grub_module_digest(modules, fail, prefix)
    if actual_modules != expected_modules:
        fail(
            f"the pinned GRUB module set digest is {actual_modules}, but sel4/pins.toml "
            f"[{BOOT_PINS_SECTION}].grub_modules_sha256 pins {expected_modules}"
        )
    records["grub_modules"] = actual_modules
    return records


def assemble_media(
    tree: Path,
    *,
    kernel: Path,
    root_task: Path,
    profile: dict[str, object],
    boot_pins: dict[str, object],
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    """Write the EFI file tree and return its identity records.

    The tree is rebuilt from empty rather than updated in place: a stale file
    left behind by an earlier layout would be read by the bootloader's `search`
    and would not appear in the digest as a difference from the intended tree.
    """
    from harness import profile_integer, profile_text  # noqa: PLC0415

    verify_boot_inputs(boot_pins, fail, profile)

    kernel_module = profile_text(profile, "kernel_module", fail, PINS_SECTION)
    root_module = profile_text(profile, "root_module", fail, PINS_SECTION)
    grub_config = profile_text(profile, "grub_config", fail, PINS_SECTION)
    efi_boot_file = profile_text(profile, "efi_boot_file", fail, PINS_SECTION)
    serial_baud = profile_integer(profile, "serial_baud", fail, PINS_SECTION)
    modules = profile_strings(profile, "grub_modules", fail)

    if tree.exists():
        shutil.rmtree(tree)
    for relative in (kernel_module, root_module, grub_config, efi_boot_file):
        (tree / relative).parent.mkdir(parents=True, exist_ok=True)

    for source, relative in ((kernel, kernel_module), (root_task, root_module)):
        if not source.is_file():
            fail(f"missing boot module source: {source}")
        shutil.copyfile(source, tree / relative)

    configuration = GRUB_CONFIG.format(
        serial_baud=serial_baud,
        kernel_module=kernel_module,
        root_module=root_module,
    )
    (tree / grub_config).write_text(configuration, encoding="utf-8")

    prefix = grub_prefix(fail)
    early = tree.parent / f".{tree.name}.early.cfg"
    early.write_text(
        EARLY_CONFIG.format(kernel_module=kernel_module, grub_config=grub_config),
        encoding="utf-8",
    )
    command = [
        str(_require_tool(prefix, "grub-mkimage", fail)),
        "-O",
        "x86_64-efi",
        "-d",
        str(prefix / "lib" / "grub" / "x86_64-efi"),
        "-c",
        str(early),
        "-p",
        "/boot/grub",
        "-o",
        str(tree / efi_boot_file),
        *modules,
    ]
    try:
        process = subprocess.run(
            command, check=False, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT
        )
    except OSError as error:
        fail(f"cannot run grub-mkimage: {error}")
    finally:
        early.unlink(missing_ok=True)
    if process.returncode != 0:
        fail(f"grub-mkimage failed with exit status {process.returncode}:\n{process.stdout}")

    return boot_media(tree, profile=profile, fail=fail)


def boot_media(
    tree: Path,
    *,
    profile: dict[str, object],
    fail: Callable[[str], NoReturn],
) -> dict[str, object]:
    """Identity records for an assembled EFI tree.

    `tree_sha256` folds each file's relative path in with its bytes, so moving
    a module to a path the configuration does not name changes the digest even
    though the bytes are unchanged. `files` is sorted so two builds of the same
    tree produce the same record order.
    """
    from harness import profile_text  # noqa: PLC0415

    expected = tuple(
        profile_text(profile, key, fail, PINS_SECTION)
        for key in ("efi_boot_file", "grub_config", "kernel_module", "root_module")
    )
    present = sorted(
        path.relative_to(tree).as_posix() for path in tree.rglob("*") if path.is_file()
    )
    if present != sorted(expected):
        fail(
            "the assembled EFI tree does not contain exactly the pinned boot files: "
            f"expected {sorted(expected)}, found {present}"
        )
    digest = hashlib.sha256()
    files: dict[str, object] = {}
    for relative in present:
        data = (tree / relative).read_bytes()
        digest.update(relative.encode("utf-8"))
        digest.update(len(data).to_bytes(8, "little"))
        digest.update(data)
        files[relative] = {"bytes": len(data), "sha256": _sha256_bytes(data)}
    return {"tree": tree.relative_to(ROOT).as_posix(), "tree_sha256": digest.hexdigest(), "files": files}


def qemu_command(
    *,
    tree: Path,
    profile: dict[str, object],
    fail: Callable[[str], NoReturn],
    vars_copy: Path,
    extra: Sequence[str] = (),
) -> list[str]:
    """The pinned q35/OVMF command line that boots one assembled EFI tree.

    `-cpu` carries the pinned model plus the exact feature deltas the kernel's
    boot path requires; naming them here rather than choosing a richer model
    keeps the emulated CPU the oldest one that can run this kernel.

    The variable store is a per-boot copy because OVMF writes to it: booting the
    pinned template directly would mutate a pinned artifact and make the next
    pin check fail on a file the boot changed.
    """
    from harness import profile_integer, profile_text  # noqa: PLC0415

    qemu = shutil.which("qemu-system-x86_64")
    if qemu is None:
        fail("qemu-system-x86_64 is not on PATH")
    firmware = ovmf_directory(fail)
    code = firmware / "OVMF_CODE.fd"
    template = firmware / "OVMF_VARS.fd"
    for path in (code, template):
        if not path.is_file():
            fail(f"pinned firmware file {path.name} is absent from {firmware}")
    vars_copy.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(template, vars_copy)
    vars_copy.chmod(0o600)

    features = profile_strings(profile, "cpu_features", fail)
    cpu = profile_text(profile, "cpu", fail, PINS_SECTION)
    for feature in features:
        cpu += f",+{feature}"
    return [
        qemu,
        "-machine",
        profile_text(profile, "machine", fail, PINS_SECTION),
        "-cpu",
        cpu,
        "-smp",
        str(profile_integer(profile, "cpus", fail, PINS_SECTION)),
        "-m",
        f"size={profile_integer(profile, 'memory_mib', fail, PINS_SECTION)}M",
        "-nographic",
        "-serial",
        "mon:stdio",
        "-drive",
        f"if=pflash,format=raw,unit=0,readonly=on,file={code}",
        "-drive",
        f"if=pflash,format=raw,unit=1,file={vars_copy}",
        "-drive",
        f"format=raw,file=fat:rw:{tree}",
        # A guest that resets must not silently start over and emit a second
        # copy of the marker chain: a gate reading the transcript could not tell
        # one boot from two.
        "-no-reboot",
        *extra,
    ]
