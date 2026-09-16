"""Strip immutable ELF delivery copies with the explicitly selected Rust LLVM tools."""

from __future__ import annotations

import functools
import os
from pathlib import Path
import struct
import subprocess
import tempfile


@functools.cache
def _llvm_strip(toolchain: str) -> Path:
    if not toolchain or toolchain != toolchain.strip():
        raise ValueError("ELF delivery requires an explicit Rust toolchain")
    try:
        sysroot = subprocess.run(
            ["rustup", "run", toolchain, "rustc", "--print", "sysroot"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        version = subprocess.run(
            ["rustup", "run", toolchain, "rustc", "-vV"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError) as error:
        raise ValueError(
            f"cannot resolve LLVM tools for Rust toolchain {toolchain}: {error}"
        ) from error
    host = next(
        (line.removeprefix("host: ") for line in version.splitlines() if line.startswith("host: ")),
        None,
    )
    if not sysroot or not host:
        raise ValueError(f"Rust toolchain {toolchain} did not report its sysroot and host")
    path = Path(sysroot) / "lib" / "rustlib" / host / "bin" / "llvm-strip"
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError(
            f"missing llvm-strip for Rust toolchain {toolchain}: {path}; "
            f"install with `rustup component add llvm-tools-preview --toolchain {toolchain}`"
        )
    return path


def _program_headers(data: bytes) -> tuple[int, int, list[tuple[int, ...]]]:
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01":
        raise ValueError("ELF delivery requires a 64-bit little-endian ELF")
    phoff = struct.unpack_from("<Q", data, 32)[0]
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    if phentsize != 56 or not phnum or phoff < 64 or phoff + phnum * phentsize > len(data):
        raise ValueError("ELF delivery has an invalid program-header table")
    headers = [struct.unpack_from("<IIQQQQQQ", data, phoff + index * 56) for index in range(phnum)]
    for kind, _flags, offset, vaddr, _paddr, filesz, memsz, _align in headers:
        if filesz and (offset > len(data) or filesz > len(data) - offset):
            raise ValueError("ELF delivery has a truncated segment")
        if kind == 1 and (filesz > memsz or vaddr + memsz > 1 << 64):
            raise ValueError("ELF delivery has a malformed load segment")
    if not any(header[0] == 1 and header[6] for header in headers):
        raise ValueError("ELF delivery has no loadable segment")
    return phoff, phnum * phentsize, headers


def _verify_delivery(original: bytes, delivery: bytes) -> None:
    phoff, _phsize, headers = _program_headers(original)
    _, _, delivered_headers = _program_headers(delivery)
    # Section metadata and offsets of nonloaded metadata may move. Runtime
    # addresses, LOAD offsets, sizes, permissions and every segment's bytes stay.
    mutable = [(40, 48), (58, 64)]
    for index, (before, after) in enumerate(zip(headers, delivered_headers, strict=True)):
        if before == after:
            continue
        kind, _flags, offset, vaddr, paddr, filesz, _memsz, _align = before
        overlaps_load = any(
            h[0] == 1 and offset < h[2] + h[5] and h[2] < offset + filesz for h in headers
        )
        if (
            kind in (0, 1, 6)
            or vaddr != 0
            or paddr != 0
            or overlaps_load
            or before[:2] != after[:2]
            or before[3:] != after[3:]
        ):
            raise ValueError("stripping changed ELF program headers")
        mutable.append((phoff + index * 56 + 8, phoff + index * 56 + 16))
    mutable.sort()
    if original[:40] != delivery[:40] or original[48:58] != delivery[48:58]:
        raise ValueError("stripping changed the ELF execution header")
    for before, after in zip(headers, delivered_headers, strict=True):
        kind, _flags, offset, _vaddr, _paddr, filesz, _memsz, _align = before
        delivered_offset = after[2]
        if kind == 0 or not filesz:
            continue
        end = offset + filesz
        cursor = offset
        for start, stop in mutable:
            if stop <= cursor or start >= end:
                continue
            begin = delivered_offset + cursor - offset
            finish = delivered_offset + max(cursor, start) - offset
            if original[cursor : max(cursor, start)] != delivery[begin:finish]:
                raise ValueError("stripping changed ELF segment contents")
            cursor = min(end, stop)
        if (
            original[cursor:end]
            != delivery[delivered_offset + cursor - offset : delivered_offset + filesz]
        ):
            raise ValueError("stripping changed ELF segment contents")


def strip_elf_delivery(data: bytes, *, toolchain: str) -> bytes:
    """Return a stripped copy, preserving execution headers and segment contents.

    Callers must apply their own target/runtime admission to the original first;
    stripping must not repair an input their loader would have refused.
    """
    _program_headers(data)
    strip = _llvm_strip(toolchain)
    try:
        with tempfile.TemporaryDirectory(prefix="slime-elf-delivery-") as temporary:
            path = Path(temporary) / "delivery.elf"
            path.write_bytes(data)
            result = subprocess.run(
                [
                    str(strip),
                    "--strip-all",
                    # ChildImage::worker resolves this runtime ABI from symbols.
                    "--keep-symbol=__slime_rt_worker_entrypoint",
                    "--keep-symbol=__slime_rt_worker_stack",
                    str(path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            if result.returncode:
                raise ValueError(f"llvm-strip failed for {toolchain}: {result.stderr.strip()}")
            delivery = path.read_bytes()
    except OSError as error:
        raise ValueError(f"cannot strip ELF delivery copy: {error}") from error
    _verify_delivery(data, delivery)
    return delivery
