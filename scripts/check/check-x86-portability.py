#!/usr/bin/env python3
# P1: x86-64 architecture-boundary allowlist and neutral-core cross build.
#
# Two independent halves:
#
#   1. A source allowlist. x86 instructions, registers, ELF/linker constants,
#      and `qemu-system-x86_64` assumptions may appear only in the admitted
#      architecture, platform, and build files listed below. A new leak into
#      architecture-neutral kernel, component-runtime, contract, or generation
#      code fails here with the offending file, line, and token.
#
#   2. A real cross build. The allowlist is a text scan and can be evaded, so
#      this also builds the kernel library and the component runtime for
#      `aarch64-unknown-none`. `cargo build` (not `cargo check`) is required:
#      inline-assembly operands and mnemonics are only validated during codegen,
#      so `cargo check` accepts x86 assembly on an AArch64 target. That is the
#      half that actually proves neutral code carries no x86 mechanism.
#
# This does not claim AArch64 boots. It proves the boundary holds; P2 brings up
# the architecture.

from __future__ import annotations
import sys as _sys
from pathlib import Path as _Path

_sys.path.insert(0, str(_Path(__file__).resolve().parents[1] / "lib"))

import os
import re
import subprocess
import sys

from harness import ROOT

# The target the neutral core is cross-built for, and the profile that names it.
CROSS_TARGET = "aarch64-unknown-none"
CROSS_PROFILE = "aarch64-qemu-virt"

# Trees scanned for x86 mechanism. Everything here is architecture-neutral
# unless its path appears in ADMITTED below.
SCANNED_TREES = (
    ROOT / "kernel" / "src",
    ROOT / "components" / "runtime" / "src",
    ROOT / "components" / "bins" / "src",
    ROOT / "components" / "proto" / "src",
    ROOT / "boot-contracts" / "src",
)

# Files admitted to contain x86 mechanism, repository-relative.
#
# `arch/x86_64/**` is the architecture implementation. `platform/**` is PC-class
# machine assembly (ACPI, PCI ECAM, i8042, ACPI power) that is selected by
# target profile rather than shared. `stage0` is the x86-64 UEFI loader, which
# gets its own profile-specific loader under P2. The component runtime's
# `arch/x86_64.rs` is the userspace trap stub.
ADMITTED_PREFIXES = (
    "kernel/src/arch/x86_64/",
    "kernel/src/platform/",
    "components/runtime/src/arch/x86_64.rs",
)

# The architecture modules for *other* targets. They legitimately contain their
# own ISA's assembly, which is not an x86 leak; the assembly patterns below are
# x86-specific but `core::arch::asm!` itself is not, so these are scanned for
# every rule except the bare-assembly ones.
OTHER_ARCH_PREFIXES = (
    "kernel/src/arch/aarch64/",
    "components/runtime/src/arch/aarch64.rs",
)

# Rules that flag assembly regardless of ISA. Skipped inside another
# architecture's own module, which is exactly where its assembly belongs.
ASSEMBLY_RULES = frozenset(
    {
        "inline assembly outside the architecture boundary",
        "module-level assembly outside the boundary",
    }
)

# Files admitted to *name* the x86 target profile in a `cfg`, without containing
# x86 mechanism. These are the profile dispatch points the boundary is allowed
# to have: module selection and the neutral fallbacks beside it.
CFG_DISPATCH_ALLOWED = {
    "kernel/src/arch/mod.rs",
    # P0's target-profile resolution: binds the compiled ISA to exactly one
    # admitted profile and fails closed on a mismatch. This is the contract
    # that makes profile dispatch safe, not a consumer of it.
    "boot-contracts/src/target_profile.rs",
    "kernel/src/lib.rs",
    "kernel/src/drivers/mod.rs",
    "kernel/src/drivers/device_discovery.rs",
    "kernel/src/storage/block_device.rs",
    "kernel/src/platform/mod.rs",
    "components/runtime/src/arch/mod.rs",
}

# x86 mechanism tokens. Each is (regex, description). Kept deliberately narrow:
# a false positive blocks the gate, so the patterns name mechanism rather than
# any string containing "x86".
FORBIDDEN = (
    (r"\bcore::arch::asm!", "inline assembly outside the architecture boundary"),
    (r"\bcore::arch::global_asm!", "module-level assembly outside the boundary"),
    (r"\bglobal_asm!", "module-level assembly outside the boundary"),
    (r"\bcore::arch::x86_64\b", "x86-64 intrinsics"),
    (r'"x86-interrupt"', "the x86-interrupt calling convention"),
    (r"\babi_x86_interrupt\b", "the x86-interrupt ABI feature"),
    (r"\bcr[034]\b", "an x86 control register"),
    (r"\b(?:rdmsr|wrmsr|invlpg|iretq|lgdt|lidt|ltr|retfq|cpuid)\b", "an x86 instruction"),
    (r"\bqemu-system-x86_64\b", "an x86-only QEMU launcher"),
    (r"\bEM_X86_64\b", "the x86-64 ELF machine constant"),
    (r"\bR_X86_64_\w+", "an x86-64 relocation type"),
)

# Register names are matched separately: they are short and collide with
# ordinary identifiers, so they only count as a violation when used as an inline
# assembly operand constraint (`in("rax")`) or a frame field access
# (`frame.rax`).
REGISTERS = (
    "rax", "rbx", "rcx", "rdx", "rsi", "rdi", "rbp", "rsp",
    "r8", "r9", "r10", "r11", "r12", "r13", "r14", "r15",
    "rip", "rflags", "eax", "ebx", "ecx", "edx", "cs", "ss", "ds", "es", "fs", "gs",
)
REGISTER_OPERAND = re.compile(
    r"""(?:in|out|inout|lateout|inlateout)\s*\(\s*"(?:""" + "|".join(REGISTERS) + r""")"\s*\)"""
)
# Any field access naming an x86 register: `frame.rax`, `f.rdi as u32`,
# `if frame.rdx != 0`. Deliberately not restricted to assignment or call
# contexts — a read is as much a boundary violation as a write, and the
# narrower form let `frame.rax + 1` through.
REGISTER_FIELD = re.compile(r"\.\s*(?:" + "|".join(REGISTERS) + r")\b(?!\s*\()")

# Any `cfg` naming the x86 target, in any nesting. Matched as two independent
# conditions on the same line rather than one shape, because `cfg(all(...))`,
# `cfg(any(...))`, and `cfg(not(...))` compose arbitrarily and a shape-matching
# regex silently admits the nested forms — which is precisely how a per-
# architecture semantic divergence would enter neutral code.
CFG_KEYWORD = re.compile(r"\bcfg(?:_attr)?\s*\(")
CFG_TARGET_X86 = re.compile(r'target_arch\s*=\s*"x86_64"')


def fail(message: str) -> None:
    raise SystemExit(f"x86 portability check: {message}")


def relative(path: _Path) -> str:
    return str(path.relative_to(ROOT))


def admitted(path: str) -> bool:
    return any(path.startswith(prefix) for prefix in ADMITTED_PREFIXES)


def strip_comments(line: str) -> str:
    """Drop a trailing `//` comment so prose may discuss x86 mechanism.

    Doc comments and ordinary comments are documentation, not mechanism; the
    boundary is about what the compiler emits. Naive but sufficient here: the
    scanned trees have no `//` inside a string literal on a line that also
    carries a forbidden token.
    """
    index = line.find("//")
    return line if index < 0 else line[:index]


# The one admitted x86 token outside the boundary, as (path, line-substring).
# Crate features must be declared at the crate root, so `abi_x86_interrupt`
# cannot be moved into `arch::x86_64` alongside the handlers that use it. It is
# `cfg`-gated on the target, so no other architecture enables it. Any *other*
# occurrence still fails.
ROOT_FEATURE_EXCEPTION = (
    "kernel/src/lib.rs",
    'cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))',
)


def scan_sources() -> list[str]:
    violations: list[str] = []
    for tree in SCANNED_TREES:
        if not tree.is_dir():
            fail(f"scanned tree is missing: {relative(tree)}")
        for path in sorted(tree.rglob("*.rs")):
            name = relative(path)
            if admitted(name):
                continue
            for number, raw in enumerate(path.read_text().splitlines(), start=1):
                line = strip_comments(raw)
                if not line.strip():
                    continue
                if name == ROOT_FEATURE_EXCEPTION[0] and ROOT_FEATURE_EXCEPTION[1] in line:
                    continue
                other_arch = name.startswith(OTHER_ARCH_PREFIXES)
                for pattern, description in FORBIDDEN:
                    if other_arch and description in ASSEMBLY_RULES:
                        continue
                    if re.search(pattern, line):
                        violations.append(f"{name}:{number}: {description}: {line.strip()}")
                if REGISTER_OPERAND.search(line):
                    violations.append(
                        f"{name}:{number}: an x86 register operand: {line.strip()}"
                    )
                if REGISTER_FIELD.search(line):
                    violations.append(
                        f"{name}:{number}: an x86 register frame field: {line.strip()}"
                    )
    return violations


def scan_cfg_dispatch() -> list[str]:
    """Profile dispatch is allowed, but only at the declared dispatch points.

    A `cfg` naming `x86_64` anywhere else means neutral code grew a silent
    x86-only path instead of going through the boundary — a constant, bound, or
    syscall arm that differs by architecture is exactly the divergence the
    roadmap forbids.

    Matched over the whole file rather than line by line: a `cfg` attribute may
    wrap across lines (`#[cfg(all(\n    target_arch = "x86_64",`), and a
    per-line scan misses that. Attribute extents are found by brace-matching
    from each `cfg` keyword.
    """
    violations: list[str] = []
    for tree in SCANNED_TREES:
        for path in sorted(tree.rglob("*.rs")):
            name = relative(path)
            if admitted(name) or name in CFG_DISPATCH_ALLOWED:
                continue
            text = path.read_text()
            for match in CFG_KEYWORD.finditer(text):
                extent = cfg_extent(text, match.end() - 1)
                if extent is None or not CFG_TARGET_X86.search(extent):
                    continue
                line = text.count("\n", 0, match.start()) + 1
                violations.append(
                    f"{name}:{line}: x86 profile dispatch outside the declared "
                    f"dispatch points: {' '.join(extent.split())[:120]}"
                )
    return violations


def cfg_extent(text: str, open_index: int) -> str | None:
    """The full parenthesized body of a `cfg`, however many lines it spans."""
    if open_index >= len(text) or text[open_index] != "(":
        return None
    depth = 0
    for index in range(open_index, len(text)):
        character = text[index]
        if character == "(":
            depth += 1
        elif character == ")":
            depth -= 1
            if depth == 0:
                return text[open_index : index + 1]
    return None


def installed_targets() -> set[str]:
    process = subprocess.run(
        ["rustup", "target", "list", "--installed"],
        cwd=ROOT,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    if process.returncode != 0:
        return set()
    return {line.strip() for line in process.stdout.splitlines() if line.strip()}


def cross_build() -> None:
    """Build the neutral core for AArch64.

    `cargo build`, not `cargo check`: inline-assembly mnemonics and register
    constraints are validated during codegen, so a `check` accepts x86 assembly
    on an AArch64 target and would make this gate vacuous.
    """
    if CROSS_TARGET not in installed_targets():
        fail(
            f"the {CROSS_TARGET} Rust target is not installed, so the neutral "
            f"core cannot be cross-built. Install it with "
            f"`rustup target add {CROSS_TARGET}`. The source allowlist alone "
            f"does not close this gate."
        )
    environment = dict(os.environ)
    environment["SLIME_TARGET_PROFILE"] = CROSS_PROFILE
    for manifest, package in (
        (ROOT / "kernel" / "Cargo.toml", "slime_os-kernel"),
        (ROOT / "components" / "runtime" / "Cargo.toml", "slime-rt"),
    ):
        arguments = [
            "cargo",
            "build",
            "--lib",
            "--manifest-path",
            str(manifest),
            "-p",
            package,
            "--target",
            CROSS_TARGET,
        ]
        process = subprocess.run(
            arguments,
            cwd=ROOT,
            env=environment,
            check=False,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        if process.returncode != 0:
            sys.stdout.write(process.stdout)
            fail(
                f"{package} does not build for {CROSS_TARGET}; architecture-neutral "
                f"code still depends on x86 mechanism"
            )
        print(f"cross build: {package} builds for {CROSS_TARGET}")


def main() -> None:
    violations = scan_sources() + scan_cfg_dispatch()
    if violations:
        for violation in violations:
            print(violation, file=sys.stderr)
        fail(f"{len(violations)} x86 mechanism leak(s) outside the architecture boundary")
    scanned = sum(1 for tree in SCANNED_TREES for _ in tree.rglob("*.rs"))
    print(f"source allowlist: {scanned} Rust files scanned, no x86 mechanism outside the boundary")
    cross_build()
    print("x86 portability check passed")


if __name__ == "__main__":
    main()
