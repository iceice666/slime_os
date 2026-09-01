"""CP6-CP10 component SDK exporter, release record, and compatibility policy.

One module owns everything about an SDK release: which files leave a `slime_os`
checkout, how the exported tree is digested, what the
`contracts/component-sdk-release/v1` record says about it, how two releases are
classified against each other, and what a consumer needs to verify and build.

CP5 proved out-of-tree development but constructed its SDK inside its own gate,
so the bundle's shape lived in test-local Python and nothing described the
result. That is the coupling this module removes: every CP6-CP10 script is a
caller, not a second exporter. If a gate held its own copy of the file set, the
manifest text, or the identity rule, a candidate export and a published release
could differ while both passed.

Every entry point takes the source tree explicitly rather than assuming the
checkout this file lives in. That is what makes CP7's reverse-drift check
possible at all: it exports the *recorded* source commit from a separate
worktree and compares, which an exporter hard-wired to its own checkout could
not do.

Determinism rules, all load-bearing:

* Every digest is domain-separated and computed over an explicit canonical
  encoding, never over `tar` bytes whose metadata a host controls.
* Archives are uncompressed `tar` with fixed member metadata. A compressor is a
  second implementation in the reproducibility surface, and 1.7 MiB per prefix
  does not buy one.
* Exported crate manifests are copied byte-for-byte. The generated SDK
  workspace supplies the inherited lint and release-profile context instead of
  the exporter rewriting `publish = false` or `[lints] workspace = true` out of
  a copied file, which is what CP5's test-local recipe did.
* The two seL4-generated headers that record their `.bf` source by absolute
  path are canonicalized to the same `/slime/sel4` logical prefix the kernel
  build already maps its own debug paths to. Any *other* host path in an
  exported byte is a refusal, not a rewrite: a silent fixup is how a checkout
  path reaches a consumer.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import tarfile
import tomllib
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

import component_sdk_release_contract as default_contract
from harness import ROOT

CONTRACT_ROOT = ROOT / "contracts" / "component-sdk-release" / "v1"
MATRIX_ROOT = ROOT / "sdk"
MATRIX_PATH = MATRIX_ROOT / "compatibility-matrix.zti"

# The exported crates, in workspace-member order, as `(source path, package
# name)`. The SDK keeps the source layout because the crates' relative path
# dependencies are part of the bundle rather than something the exporter
# rewrites into a registry shape.
EXPORT_CRATES: tuple[tuple[str, str], ...] = (
    ("boot-contracts", "boot-contracts"),
    ("components/build-support", "slime-build-support"),
    ("components/proto", "slime-proto"),
    ("components/lib", "slime-components"),
    ("components/runtime", "slime-rt"),
)

# Vendored source the exported crates resolve by relative path. Not a crate the
# SDK owns: it is the pinned `rust-sel4` checkout `slime-rt` compiles against,
# and its provenance is recorded through the `[rust_sel4]` pin.
VENDORED = ("deps/rust-sel4",)

# The target specifications a component builds against, copied out of the
# vendored source into stable SDK locations so a consumer names one path per
# profile. Keyed by file stem because that is what Cargo calls a JSON target and
# what `slime-build-support` matches on.
#
# Per-profile rather than one constant: the RV64 profiles build against
# `riscv64imac-sel4-minimal.json`, so a single exported specification would
# either omit them or bind every profile's `targetSpecHash` to one unrelated
# file. `aarch64-rpi5` needs none of these -- it is a bare triple.
TARGET_SPEC_SOURCE_DIR = "deps/rust-sel4/support/targets"
TARGET_SPEC_SDK_DIR = "targets"
TARGET_SPECS = ("aarch64-sel4-minimal", "riscv64imac-sel4-minimal")


def target_spec_sdk_path(stem: str) -> str:
    return f"{TARGET_SPEC_SDK_DIR}/{stem}.json"


def target_spec_source_paths() -> tuple[str, ...]:
    """Every target specification the export reads, as source-relative paths.

    Exists so a gate mirroring the export's inputs derives them from here
    instead of restating one path. That restatement is exactly how the linker
    scripts once became an export input a hand-written list did not know about.
    """
    return tuple(f"{TARGET_SPEC_SOURCE_DIR}/{stem}.json" for stem in TARGET_SPECS)


# The component linker scripts. Repository-level build inputs rather than crate
# sources: an `aarch64-unknown-none` component links at the fixed component base
# `contracts/target-profile/v1` declares, and `slime-build-support` passes the
# matching script with `-T`. They ship in the SDK because an out-of-tree crate
# cannot find them relative to its own manifest, and `tools/sdk-build.py` points
# `SLIME_COMPONENT_LINKER_DIR` at the exported copies.
LINKER_SCRIPTS = ("components/component.ld", "components/component-aarch64.ld")
LINKER_SCRIPT_SDK = "linker"

# What the exporter writes rather than copies.
GENERATED_FILES = (
    "Cargo.toml",
    "README.md",
    "tools/sdk-build.py",
    "tools/sdk-update.py",
    "template/Cargo.toml",
    "template/component/Cargo.toml",
    "template/component/build.rs",
    "template/component/src/main.rs",
)

# What a copied directory never carries out of the source repository.
COPY_IGNORE = shutil.ignore_patterns("target", ".git", "*.rs.bk")

# Domain separators. Distinct constants rather than one reused string: a tree
# digest, a set digest, and a scalar axis digest must not be able to collide by
# construction.
TREE_DOMAIN = b"slime-component-sdk-tree-v1\0"
SET_DOMAIN = b"slime-component-sdk-set-v1\0"
AXIS_DOMAIN = b"slime-component-sdk-axis-v1\0"

# Everything an SDK profile needs beyond its prefix bytes, keyed by the target
# profile name `contracts/target-profile/v1` declares.
#
# `platform` is the `scripts/build/build-sel4.py` platform that produces the
# prefix, kept distinct on purpose: `aarch64-rpi5` is the *profile* for the
# `bcm2712-rpi5` *platform*, and a consumer that conflated them would export the
# wrong `SLIME_TARGET_PROFILE` beside a correct prefix.
#
# `cargo_target`, `rust_flags`, and `cargo_flags` are read from the same places
# the product build reads them -- `contracts/target-profile/v1`'s `cargoTarget`
# and `components/.cargo/config.toml`'s per-triple `rustflags` -- rather than
# invented here, and they genuinely differ per profile. The seL4 JSON target
# needs `-Z json-target-spec` plus a `build-std`, inherits no config rustflags,
# and links a component at its own addresses. The `aarch64-unknown-none` triple
# has a prebuilt `core`, needs no unstable flag, and must link at the profile's
# fixed component base -- without `relocation-model=static`, `code-model=small`,
# and the 4 KiB max page size, the resulting ELF is refused by the generation
# builder with "invalid component load layout".
PROFILE_PLATFORMS: dict[str, dict[str, object]] = {
    "aarch64-sel4-qemu-virt": {
        "platform": "qemu-arm-virt",
        "prefix": "build/sel4-prefix",
        "pins": "observed_prefix",
        "cargo_target": target_spec_sdk_path("aarch64-sel4-minimal"),
        "cargo_target_is_spec": True,
        "rust_flags": ("-C", "link-arg=--build-id=none"),
        "cargo_flags": (
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ),
    },
    "aarch64-rpi5": {
        "platform": "bcm2712-rpi5",
        "prefix": "build/sel4-rpi5-prefix",
        "pins": "observed_prefix_bcm2712_rpi5",
        "cargo_target": "aarch64-unknown-none",
        "cargo_target_is_spec": False,
        "rust_flags": (
            "-C",
            "relocation-model=static",
            "-C",
            "code-model=small",
            "-C",
            "link-arg=--build-id=none",
            "-C",
            "link-arg=-z",
            "-C",
            "link-arg=max-page-size=4096",
        ),
        "cargo_flags": (),
    },
    # Both RV64 profiles build against one specification and differ in platform
    # identity, prefix, and pins -- exactly the distinction CP8 records for
    # AArch64: `qemu-riscv-virt` is a QEMU reference and `cv1800b-duo` is a named
    # board whose C906, firmware handoff, PLIC, timer, and 63.25 MiB window are
    # not interchangeable with it. A component qualified for one is refused by
    # the other.
    #
    # Flags mirror the AArch64 seL4 profile because both are JSON targets, and
    # `scripts/build/build-generation.py` builds every JSON target with the same
    # `-Z` set and the same single determinism-relevant link argument. No linker
    # script: `slime-build-support` returns `None` for the seL4 targets, so a
    # component there links at its own addresses as an ordinary seL4 task.
    "riscv64-sel4-qemu-virt": {
        "platform": "qemu-riscv-virt",
        "prefix": "build/sel4-riscv64-prefix",
        "pins": "observed_prefix_qemu_riscv_virt",
        "cargo_target": target_spec_sdk_path("riscv64imac-sel4-minimal"),
        "cargo_target_is_spec": True,
        "rust_flags": ("-C", "link-arg=--build-id=none"),
        "cargo_flags": (
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ),
    },
    "riscv64-sel4-milkv-duo": {
        "platform": "cv1800b-duo",
        "prefix": "build/sel4-cv1800b-duo-prefix",
        "pins": "observed_prefix_cv1800b_duo",
        "cargo_target": target_spec_sdk_path("riscv64imac-sel4-minimal"),
        "cargo_target_is_spec": True,
        "rust_flags": ("-C", "link-arg=--build-id=none"),
        "cargo_flags": (
            "-Z",
            "json-target-spec",
            "-Z",
            "build-std=core,alloc,compiler_builtins",
            "-Z",
            "build-std-features=compiler-builtins-mem",
        ),
    },
}
DEFAULT_PROFILES = ("aarch64-sel4-qemu-virt", "aarch64-rpi5")

# The logical prefix the seL4 build already maps its own source paths to
# (`-ffile-prefix-map=<deps/sel4>=/slime/sel4` in `build-sel4.py`). Two
# generated libsel4 headers record their `.bf` input in a leading comment the
# prefix map does not reach, because nothing compiles them here: seL4's own
# Python generators emit them. Canonicalizing those two to the same logical
# prefix makes an installed prefix checkout-independent without inventing a
# second convention.
LOGICAL_SEL4_SOURCE = "/slime/sel4"
CANONICALIZED_PREFIX_FILES = (
    "libsel4/include/sel4/shared_types_gen.h",
    "libsel4/include/sel4/sel4_arch/types_gen.h",
)

_GENERATED_BY = re.compile(rb"@generated by (contracts/[a-z0-9./-]+)/gen_rust\.zt")
_SEMVER = re.compile(r"^(\d+)\.(\d+)\.(\d+)$")
_COMMIT = re.compile(r"^[0-9a-f]{40}$")
_SHA256 = re.compile(r"^[0-9a-f]{64}$")
_RANK = {"patch": 0, "minor": 1, "major": 2}


class ComponentSdkError(ValueError):
    pass


def _fail(message: str) -> None:
    raise ComponentSdkError(message)


@dataclass(frozen=True)
class ExportedSdk:
    """One exported tree and the record that describes it."""

    root: Path
    record: dict
    normalized: bytes
    identity: bytes

    @property
    def version(self) -> str:
        return self.record["version"]

    @property
    def tree_identity(self) -> str:
        return self.record["treeIdentity"]


# --------------------------------------------------------------------------
# Source-tree facts
# --------------------------------------------------------------------------


def git(arguments: list[str], *, cwd: Path) -> str:
    process = subprocess.run(
        ["git", *arguments],
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        _fail(f"git {' '.join(arguments)} failed: {process.stdout.strip()}")
    return process.stdout.strip()


def pins(source: Path = ROOT) -> dict:
    return tomllib.loads((source / "sel4" / "pins.toml").read_text(encoding="utf-8"))


def source_commit(source: Path = ROOT) -> str:
    commit = git(["rev-parse", "HEAD"], cwd=source)
    if _COMMIT.fullmatch(commit) is None:
        _fail(f"source commit is not a full SHA-1 identity: {commit!r}")
    return commit


def source_repository(source: Path = ROOT) -> str:
    return git(["config", "--get", "remote.origin.url"], cwd=source)


def host_path_needles(source: Path = ROOT, destination: Path | None = None) -> tuple[bytes, ...]:
    """Absolute host paths an exported byte may never contain.

    Derived from paths this export actually used rather than pattern-matched: a
    literal `/home/<name>` in a vendored upstream Dockerfile is that project's
    documentation, not a leak from this build, and a regex over generic path
    prefixes cannot tell those apart.

    The *shared* temp root is deliberately absent. `tempfile.gettempdir()` is
    `/tmp` on Linux, which four tracked `deps/rust-sel4` files legitimately
    contain (`.gitignore`, `rustfmt.toml`, and two Nix expressions), so treating
    it as a needle refused every export on the CI runner while passing on macOS,
    whose per-user `/var/folders/...` root never collides. `destination` is the
    export's own staging directory, which is specific enough to be a real leak
    and is what the exporter controls.
    """
    needles = [
        str(source).encode("utf-8"),
        str(Path.home()).encode("utf-8"),
        b"/nix/store/",
    ]
    if destination is not None:
        needles.append(str(destination).encode("utf-8"))
    return tuple(needle for needle in needles if needle not in (b"", b"/"))


# --------------------------------------------------------------------------
# Canonical digests
# --------------------------------------------------------------------------


def _absorb(digest, value: bytes) -> None:
    """Length-prefix every variable-length field.

    Concatenating variable-length fields lets two different field splits produce
    one byte string, so the length prefix is what makes the encoding injective
    rather than merely deterministic.
    """
    digest.update(len(value).to_bytes(8, "little"))
    digest.update(value)


def tree_files(root: Path, *, exclude: tuple[str, ...] = ()) -> list[Path]:
    """Every regular file below `root`, sorted by POSIX relative path.

    An `exclude` entry names either an exact relative path or a directory whose
    whole subtree is skipped, so `.git` excludes repository metadata rather than
    only a file literally named `.git`.

    A symlink is a refusal rather than a followed edge: an exported tree that
    carries one describes bytes outside itself.
    """
    if not root.is_dir():
        _fail(f"not a directory: {root}")
    found: list[Path] = []
    for path in sorted(root.rglob("*"), key=lambda entry: entry.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        if any(relative == entry or relative.startswith(f"{entry}/") for entry in exclude):
            continue
        if path.is_symlink():
            _fail(f"tree contains a symlink: {relative}")
        if path.is_dir():
            continue
        if not path.is_file():
            _fail(f"tree contains a non-regular file: {relative}")
        found.append(path)
    return found


def tree_digest(root: Path, *, exclude: tuple[str, ...] = ()) -> str:
    """Canonical SHA-256 over a directory's content, paths, and executability.

    The mode is reduced to one executable bit deliberately: that is the only
    permission `git` itself preserves, so a digest sensitive to more than that
    could not survive a clone of the published SDK.
    """
    digest = hashlib.sha256()
    digest.update(TREE_DOMAIN)
    files = tree_files(root, exclude=exclude)
    digest.update(len(files).to_bytes(8, "little"))
    for path in files:
        _absorb(digest, path.relative_to(root).as_posix().encode("utf-8"))
        digest.update(b"\1" if os.access(path, os.X_OK) else b"\0")
        _absorb(digest, hashlib.sha256(path.read_bytes()).digest())
    return digest.hexdigest()


def set_digest(label: str, rows: list[tuple[str, ...]]) -> str:
    """Canonical digest over a sorted set of tuples."""
    digest = hashlib.sha256()
    digest.update(SET_DOMAIN)
    _absorb(digest, label.encode("utf-8"))
    ordered = sorted(rows)
    digest.update(len(ordered).to_bytes(8, "little"))
    for row in ordered:
        digest.update(len(row).to_bytes(8, "little"))
        for field in row:
            _absorb(digest, field.encode("utf-8"))
    return digest.hexdigest()


def axis_digest(label: str, *values: str) -> str:
    digest = hashlib.sha256()
    digest.update(AXIS_DOMAIN)
    _absorb(digest, label.encode("utf-8"))
    for value in values:
        _absorb(digest, value.encode("utf-8"))
    return digest.hexdigest()


def canonical_tar(root: Path, destination: Path) -> None:
    """Write `root` as an uncompressed tar with fixed member metadata.

    Every field a host could vary -- owner, group, mode beyond the executable
    bit, mtime, device numbers, PAX headers -- is fixed here, so the archive
    bytes are a function of the tree's content and nothing else.
    """
    destination.parent.mkdir(parents=True, exist_ok=True)
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for path in tree_files(root):
            content = path.read_bytes()
            info = tarfile.TarInfo(path.relative_to(root).as_posix())
            info.size = len(content)
            info.mtime = 0
            info.mode = 0o755 if os.access(path, os.X_OK) else 0o644
            info.uid = 0
            info.gid = 0
            info.uname = ""
            info.gname = ""
            info.type = tarfile.REGTYPE
            archive.addfile(info, io.BytesIO(content))
    destination.write_bytes(buffer.getvalue())


def extract_canonical_tar(archive: Path, destination: Path) -> None:
    """Extract an exporter-written archive, refusing anything it cannot have written."""
    destination.mkdir(parents=True, exist_ok=True)
    resolved = destination.resolve()
    try:
        opened = tarfile.open(archive, mode="r:")
    except tarfile.TarError as error:
        _fail(f"{archive.name}: not a readable uncompressed tar archive: {error}")
    with opened:
        try:
            members = opened.getmembers()
        except tarfile.TarError as error:
            _fail(f"{archive.name}: truncated or malformed archive: {error}")
        if not members:
            _fail(f"{archive.name}: archive is empty")
        for info in members:
            if not info.isfile():
                _fail(f"{archive.name}: member {info.name!r} is not a regular file")
            target = (destination / info.name).resolve()
            if not target.is_relative_to(resolved):
                _fail(f"{archive.name}: member {info.name!r} escapes the destination")
            try:
                handle = opened.extractfile(info)
                content = b"" if handle is None else handle.read()
            except tarfile.TarError as error:
                _fail(f"{archive.name}: member {info.name!r} is unreadable: {error}")
            if len(content) != info.size:
                _fail(f"{archive.name}: member {info.name!r} is truncated")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
            target.chmod(0o755 if info.mode & 0o111 else 0o644)


# --------------------------------------------------------------------------
# Public contract set
# --------------------------------------------------------------------------


def exported_contract_paths(crate_roots: list[Path], source: Path = ROOT) -> list[str]:
    """Every contract the exported crates' generated bindings were rendered from.

    Derived by reading the `@generated by` headers in the exported bytes rather
    than from a list here. A hand-maintained contract set is a second authority
    on which formats an SDK consumer's component speaks, and it would silently
    stop covering a contract the moment a binding moved crates.
    """
    found: set[str] = set()
    for crate in crate_roots:
        for path in tree_files(crate):
            if path.suffix != ".rs":
                continue
            match = _GENERATED_BY.search(path.read_bytes()[:512])
            if match is not None:
                found.add(match.group(1).decode("ascii"))
    # `contracts/interface-schema/v1` renders through a Python generator rather
    # than a `gen_rust.zt`, so its header names the script and the derivation
    # above cannot see it. Omitting it would leave a public format out of the
    # set that decides compatibility.
    found.add("contracts/interface-schema/v1")
    # The release record's own contract. A consumer decodes it before it decodes
    # anything else, so a change to it is a change to the SDK's public surface.
    found.add("contracts/component-sdk-release/v1")
    for relative in sorted(found):
        if not (source / relative).is_dir():
            _fail(f"exported binding names a contract that does not exist: {relative}")
    return sorted(found)


def contract_identities(
    paths: list[str], source: Path = ROOT, contract: ModuleType = default_contract
) -> list[dict]:
    if len(paths) > contract.MAX_CONTRACTS:
        _fail(f"public contract set exceeds {contract.MAX_CONTRACTS} entries")
    return [{"name": name, "identity": tree_digest(source / name)} for name in paths]


# --------------------------------------------------------------------------
# Generated SDK files
# --------------------------------------------------------------------------


def _workspace_lints(source: Path) -> str:
    """The `[workspace.lints]` table the exported crates inherit.

    Read from the source repository's root manifest rather than restated: an
    exported crate keeps `[lints] workspace = true` byte-for-byte, so the SDK
    workspace must supply exactly the table that clause resolves against here.
    """
    manifest = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
    rows = ["[workspace.lints.clippy]"]
    for name, level in manifest["workspace"]["lints"]["clippy"].items():
        rows.append(f'{name} = "{level}"')
    return "\n".join(rows) + "\n"


def _release_profile(source: Path) -> str:
    """The release profile and the per-package stanzas the exported crates need.

    `codegen-units = 1` is load-bearing rather than cosmetic for a component
    image, and the source manifest names each package because Cargo offers no
    wildcard that applies to workspace members. The SDK carries the stanzas for
    exactly the crates it exports.
    """
    manifest = tomllib.loads((source / "Cargo.toml").read_text(encoding="utf-8"))
    release = manifest["profile"]["release"]
    packages = release.get("package", {})
    rows = [
        "[profile.release]",
        f'panic = "{release["panic"]}"',
        'opt-level = "s"',
        "codegen-units = 1",
        "debug = false",
    ]
    for _, package in EXPORT_CRATES:
        stanza = packages.get(package)
        if stanza is None:
            continue
        rows += ["", f"[profile.release.package.{package}]"]
        for key, value in stanza.items():
            rendered = f'"{value}"' if isinstance(value, str) else str(value).lower()
            rows.append(f"{key} = {rendered}")
    return "\n".join(rows) + "\n"


def sdk_workspace_manifest(source: Path = ROOT) -> str:
    members = "".join(f'    "{path}",\n' for path, _ in EXPORT_CRATES)
    return (
        "# @generated by scripts/lib/component_sdk.py; do not edit.\n"
        "#\n"
        "# The generated SDK workspace. It exists so every exported crate manifest\n"
        "# can be copied byte-for-byte: `[lints] workspace = true` and the release\n"
        "# profile resolve against the tables below instead of being deleted out of\n"
        "# a copied file.\n"
        "[workspace]\n"
        'resolver = "3"\n'
        f"members = [\n{members}]\n"
        "\n" + _workspace_lints(source) + "\n" + _release_profile(source)
    )


def sdk_readme(record: dict) -> str:
    profiles = "\n".join(
        f"- `{profile['profile']}` -- platform `{profile['platform']}`, "
        f"prefix `{profile['prefix']['archive']}`"
        for profile in record["profiles"]
    )
    crates = "\n".join(f"- `{crate['name']}` {crate['version']}" for crate in record["crates"])
    first = record["profiles"][0]["profile"]
    return f"""# Slime component SDK {record["version"]}

Generated from `slime_os` commit `{record["sourceCommit"]}`. This tree is a
one-way release mirror: fixes land in the source repository and are exported,
never patched here. A commit in this repository reproduces exactly from the
source commit it records, and `just component_sdk_release_check` refuses any
byte difference.

`component-sdk-release.zti` is the authoritative description of this tree: the
originating commit, every exported crate and public contract identity, the
pinned toolchain and sources, and the platform build inputs per target profile.
`component-sdk-release.json` is its normalized form and
`component-sdk-release.identity` is the SHA-256 over that form under the
contract's identity domain. The README deliberately does not restate the tree
identity, because `treeIdentity` is a digest over this tree including this file.

## Crates

{crates}

Pin this repository by full commit and depend on the crates by `git` + `rev`.
A branch or a movable tag is not a pin. `template/` is a ready workspace that
does exactly that.

## Target profiles

{profiles}

Each profile ships its own seL4 prefix archive and target specification. They
are not interchangeable: a component built for one profile is refused as
wrong-target before its bytes reach a loader.

## Building a component

`tools/sdk-build.py` is the non-interactive entry point. It verifies this
tree's release record and the selected prefix archive, extracts the prefix,
exports `SEL4_PREFIX`, `SLIME_TARGET_PROFILE`, the target specification, and
the required Cargo flags, then runs `cargo build`:

```sh
python3 tools/sdk-build.py --profile {first} \\
  --manifest-path /path/to/your/component/Cargo.toml \\
  --package your-component
```

It requires the pinned Rust toolchain `{record["toolchain"]}` and a `libclang`
for `sel4-sys`'s bindgen (`LIBCLANG_PATH`). It reads nothing below a `slime_os`
checkout.

`tools/sdk-update.py` moves an existing consumer checkout onto this release: it
rewrites every SDK `rev` pin, refreshes the lockfile, copies this record beside
the consumer's manifest, rebuilds, and reports the component's new bare-ELF
SHA-256 for its component spec.

## Pinned sources

- seL4 `{record["sel4"]["release"]}` commit `{record["sel4"]["commit"]}`
  from `{record["sel4"]["repository"]}`
- rust-sel4 `{record["rustSel4"]["release"]}` commit
  `{record["rustSel4"]["commit"]}` from `{record["rustSel4"]["repository"]}`

## Compatibility

Support is exact-pair and evidence-backed. A pairing absent from the published
compatibility matrix is unsupported, not implicitly compatible: nothing here
infers interoperability from equal crate versions or a SemVer range.
"""


SDK_BUILD_ENTRY = '''#!/usr/bin/env python3
"""Non-interactive Slime component SDK build entry point (CP8).

Verifies this SDK tree's release record and the selected profile's prefix
archive, extracts the prefix into a cache directory, then runs Cargo with the
exact target specification, target profile, and flags the record declares.

Nothing here reads a `slime_os` checkout: the record, the archive, the target
specification, and the toolchain pin are all inside this tree.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
TREE_DOMAIN = b"slime-component-sdk-tree-v1\\0"
IDENTITY_DOMAIN = b"slime-component-sdk-release-v1:"
RECORD_FILES = (
    "component-sdk-release.zti",
    "component-sdk-release.json",
    "component-sdk-release.identity",
)


def fail(message: str) -> None:
    raise SystemExit(f"slime sdk build: {message}")


def tree_files(root: Path, exclude: tuple[str, ...] = ()) -> list[Path]:
    found = []
    for path in sorted(root.rglob("*"), key=lambda entry: entry.relative_to(root).as_posix()):
        relative = path.relative_to(root).as_posix()
        if any(relative == entry or relative.startswith(entry + "/") for entry in exclude):
            continue
        if path.is_symlink():
            fail("tree contains a symlink: " + relative)
        if path.is_dir():
            continue
        found.append(path)
    return found


def tree_digest(root: Path, exclude: tuple[str, ...] = ()) -> str:
    digest = hashlib.sha256()
    digest.update(TREE_DOMAIN)
    files = tree_files(root, exclude)
    digest.update(len(files).to_bytes(8, "little"))
    for path in files:
        relative = path.relative_to(root).as_posix().encode("utf-8")
        digest.update(len(relative).to_bytes(8, "little"))
        digest.update(relative)
        digest.update(b"\\1" if os.access(path, os.X_OK) else b"\\0")
        inner = hashlib.sha256(path.read_bytes()).digest()
        digest.update(len(inner).to_bytes(8, "little"))
        digest.update(inner)
    return digest.hexdigest()


def load_record() -> dict:
    normalized_path = SDK / "component-sdk-release.json"
    identity_path = SDK / "component-sdk-release.identity"
    if not normalized_path.is_file() or not identity_path.is_file():
        fail("this tree carries no release record")
    normalized = normalized_path.read_bytes()
    expected = hashlib.sha256(IDENTITY_DOMAIN + normalized).hexdigest()
    if identity_path.read_text(encoding="utf-8").strip() != expected:
        fail("release record identity does not match its normalized bytes")
    return json.loads(normalized.decode("utf-8"))


def verify_tree(record: dict) -> None:
    observed = tree_digest(SDK, RECORD_FILES + (".git",))
    if observed != record["treeIdentity"]:
        fail(
            "exported tree identity mismatch: the record describes "
            + record["treeIdentity"]
            + " and this tree is "
            + observed
        )


def select_profile(record: dict, name: str) -> dict:
    for profile in record["profiles"]:
        if profile["profile"] == name:
            return profile
    available = ", ".join(entry["profile"] for entry in record["profiles"])
    fail("this release declares no profile " + repr(name) + "; it declares " + available)


def verify_prefix(profile: dict, cache: Path) -> Path:
    archive = SDK / profile["prefix"]["archive"]
    if not archive.is_file():
        fail("missing prefix archive " + profile["prefix"]["archive"])
    if hashlib.sha256(archive.read_bytes()).hexdigest() != profile["prefix"]["archiveHash"]:
        fail("prefix archive " + archive.name + " does not match its recorded hash")
    prefix = cache / profile["profile"] / profile["prefix"]["treeHash"]
    if not prefix.is_dir():
        staging = prefix.with_name(prefix.name + ".partial")
        shutil.rmtree(staging, ignore_errors=True)
        staging.mkdir(parents=True)
        # Any failure below removes the staging tree. Without this a refused
        # extraction left a partial prefix behind that only a later run for the
        # *same* tree hash would ever clear, so a consumer's cache accumulated
        # orphans after every damaged archive.
        try:
            with tarfile.open(archive, mode="r:") as opened:
                for info in opened.getmembers():
                    if not info.isfile():
                        fail(
                            "prefix archive member "
                            + repr(info.name)
                            + " is not a regular file"
                        )
                    target = (staging / info.name).resolve()
                    if not target.is_relative_to(staging.resolve()):
                        fail("prefix archive member " + repr(info.name) + " escapes the prefix")
                    handle = opened.extractfile(info)
                    content = b"" if handle is None else handle.read()
                    if len(content) != info.size:
                        fail("prefix archive member " + repr(info.name) + " is truncated")
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(content)
                    target.chmod(0o755 if info.mode & 0o111 else 0o644)
        except tarfile.TarError as error:
            shutil.rmtree(staging, ignore_errors=True)
            fail("prefix archive " + archive.name + " is not a readable tar: " + str(error))
        except BaseException:
            shutil.rmtree(staging, ignore_errors=True)
            raise
        staging.replace(prefix)
    if tree_digest(prefix) != profile["prefix"]["treeHash"]:
        fail("extracted prefix does not match its recorded tree hash")
    return prefix


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", required=True)
    parser.add_argument("--manifest-path")
    parser.add_argument("--package", action="append", default=[])
    parser.add_argument("--target-dir")
    parser.add_argument("--locked", action="store_true", help="pass --locked to cargo")
    parser.add_argument(
        "--cache",
        default=str(Path.home() / ".cache" / "slime-component-sdk"),
        help="where verified prefixes are extracted",
    )
    parser.add_argument("--verify-only", action="store_true")
    parser.add_argument("--print-environment", action="store_true")
    arguments = parser.parse_args()

    record = load_record()
    verify_tree(record)
    profile = select_profile(record, arguments.profile)
    prefix = verify_prefix(profile, Path(arguments.cache).expanduser())

    # The target is either a JSON specification inside this tree or a plain
    # triple. Only the first has bytes to bind, and only the first needs the
    # unstable flags the record carries.
    if profile["cargoTargetIsSpec"]:
        target = SDK / profile["cargoTarget"]
        if not target.is_file():
            fail("missing target specification " + profile["cargoTarget"])
        if hashlib.sha256(target.read_bytes()).hexdigest() != profile["targetSpecHash"]:
            fail("target specification does not match its recorded hash")
        target_argument = str(target)
    else:
        if profile["targetSpecHash"]:
            fail("a triple target must not declare a specification hash")
        target_argument = profile["cargoTarget"]

    environment = os.environ.copy()
    environment["RUSTUP_TOOLCHAIN"] = record["toolchain"]
    environment["SEL4_PREFIX"] = str(prefix)
    environment["SLIME_TARGET_PROFILE"] = profile["profile"]
    # `slime-build-support` passes a `-T` linker script for the bare-metal
    # triples, and an out-of-tree crate cannot find one relative to its own
    # manifest. The exported copies are inside this tree.
    environment["SLIME_COMPONENT_LINKER_DIR"] = str(SDK / "linker")
    if arguments.target_dir:
        environment["CARGO_TARGET_DIR"] = str(Path(arguments.target_dir).resolve())
    # The recorded flags replace any ambient `RUSTFLAGS` rather than merging with
    # them. A component's link layout is admitted by exact comparison, so an
    # inherited flag that changed it would produce an ELF the generation builder
    # refuses -- and an override the operator cannot see is worse than one they
    # must state.
    environment["RUSTFLAGS"] = " ".join(profile["rustFlags"])

    if arguments.print_environment:
        for key in (
            "RUSTUP_TOOLCHAIN",
            "SEL4_PREFIX",
            "SLIME_TARGET_PROFILE",
            "SLIME_COMPONENT_LINKER_DIR",
            "RUSTFLAGS",
        ):
            print(key + "=" + environment[key])
        print("SLIME_TARGET=" + target_argument)
    if arguments.verify_only:
        print(
            "slime sdk build: verified "
            + record["version"]
            + " ("
            + record["treeIdentity"][:16]
            + ") for "
            + profile["profile"]
        )
        return

    command = ["cargo", "build", "--release", "--target", target_argument]
    command += list(profile["cargoFlags"])
    if arguments.locked:
        command.append("--locked")
    if arguments.manifest_path:
        command += ["--manifest-path", str(Path(arguments.manifest_path).resolve())]
    for package in arguments.package:
        command += ["-p", package]
    sys.exit(subprocess.run(command, env=environment, check=False).returncode)


if __name__ == "__main__":
    main()
'''


SDK_UPDATE_ENTRY = '''#!/usr/bin/env python3
"""Move a consumer checkout onto this SDK release (CP10).

One command changes every coupled pin together -- the SDK git revision in the
consumer's manifest, its lockfile, the verified platform asset, and the recorded
release identity -- then rebuilds the component and reports its new bare-ELF
SHA-256 for the operator-owned component spec.

Coupled deliberately: a consumer that advanced its `rev` without refreshing the
lockfile, the prefix, or the recorded identity would have a checkout whose
declared release and built bytes disagree. Every step happens in a staging copy
and is promoted only once the rebuild succeeds, so a failure at dependency
resolution, prefix verification, or compilation leaves the previous pin and its
built artifact exactly as they were.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

SDK = Path(__file__).resolve().parent.parent
BUILD = SDK / "tools" / "sdk-build.py"
RECORD_FILES = ("component-sdk-release.json", "component-sdk-release.identity")


def fail(message: str) -> None:
    raise SystemExit(f"slime sdk update: {message}")


def run(command: list[str], cwd: Path, description: str) -> str:
    process = subprocess.run(
        command,
        cwd=cwd,
        check=False,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )
    if process.returncode != 0:
        fail(f"{description} failed:\\n{process.stdout}")
    return process.stdout


def repin(text: str, packages: set[str], url: str, commit: str) -> tuple[str, set[str]]:
    """Repoint only the SDK dependencies, and report which ones were found.

    Scoped by dependency name rather than applied to the whole manifest. An
    unscoped `git = "..."` / `rev = "..."` rewrite silently repointed *every*
    git dependency a consumer had at the SDK repository and commit, and then
    committed the result over their manifest.
    """
    updated: set[str] = set()
    lines = text.splitlines(keepends=True)
    for index, line in enumerate(lines):
        name = line.split("=", 1)[0].strip().strip('"')
        if name not in packages or "git" not in line:
            continue
        rewritten = re.sub(r'(git\\s*=\\s*")[^"]+(")', lambda m: m.group(1) + url + m.group(2), line)
        rewritten = re.sub(
            r'(rev\\s*=\\s*")[0-9a-f]{40}(")',
            lambda m: m.group(1) + commit + m.group(2),
            rewritten,
        )
        if re.search(r'rev\\s*=\\s*"[0-9a-f]{40}"', rewritten) is None:
            fail("SDK dependency " + name + " is not pinned by a full commit")
        lines[index] = rewritten
        updated.add(name)
    return "".join(lines), updated


def promote(consumer: Path, staging: Path) -> None:
    """Replace the consumer with the staging tree by rename, never by deletion.

    The consumer's `target` is carried over rather than left behind: a build
    directory is not part of the pin and `copytree` deliberately skips it. `.git`
    is already in the staging copy, so it is moved only if absent there.

    Rename-then-unlink rather than delete-then-copy: a failure midway through a
    delete-then-copy leaves the consumer with its old files gone and the new ones
    half-written, which is the one state no retry recovers from.
    """
    for name in (".git", "target"):
        source = consumer / name
        if source.exists() and not (staging / name).exists():
            source.replace(staging / name)
    retired = consumer.with_name(consumer.name + ".sdk-previous")
    shutil.rmtree(retired, ignore_errors=True)
    consumer.replace(retired)
    try:
        staging.replace(consumer)
    except BaseException:
        retired.replace(consumer)
        raise
    shutil.rmtree(retired, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("consumer", type=Path, help="consumer workspace root")
    parser.add_argument("--sdk-url", required=True, help="SDK git URL to pin")
    parser.add_argument("--sdk-commit", required=True, help="SDK commit to pin")
    parser.add_argument("--profile", required=True)
    parser.add_argument("--package", required=True, help="consumer package to rebuild")
    parser.add_argument("--binary", required=True, help="built binary name, without .elf")
    parser.add_argument("--cache", help="prefix cache directory")
    parser.add_argument("--target-dir", help="cargo target directory")
    arguments = parser.parse_args()

    if re.fullmatch(r"[0-9a-f]{40}", arguments.sdk_commit) is None:
        fail("an SDK pin must be a full 40-character commit, not a branch or tag")
    consumer = arguments.consumer.resolve()
    manifest = consumer / "Cargo.toml"
    if not manifest.is_file():
        fail(f"no consumer manifest at {manifest}")

    record = json.loads((SDK / "component-sdk-release.json").read_text(encoding="utf-8"))
    sdk_packages = {entry["name"] for entry in record["crates"]}

    staging = consumer.with_name(consumer.name + ".sdk-update")
    shutil.rmtree(staging, ignore_errors=True)
    shutil.copytree(consumer, staging, ignore=shutil.ignore_patterns("target"))
    try:
        text = (staging / "Cargo.toml").read_text(encoding="utf-8")
        rewritten, updated = repin(text, sdk_packages, arguments.sdk_url, arguments.sdk_commit)
        if not updated:
            fail("consumer manifest declares no full-commit SDK pin to update")
        missing = sdk_packages - updated
        if missing:
            fail("consumer manifest pins no SDK dependency named " + ", ".join(sorted(missing)))
        (staging / "Cargo.toml").write_text(rewritten, encoding="utf-8")
        for name in RECORD_FILES:
            shutil.copyfile(SDK / name, staging / name)

        target_dir = arguments.target_dir or str(staging / "target")
        cache = ["--cache", arguments.cache] if arguments.cache else []
        run(
            [sys.executable, str(BUILD), "--profile", arguments.profile, "--verify-only"] + cache,
            staging,
            "release verification",
        )
        run(
            ["cargo", "update", "--manifest-path", str(staging / "Cargo.toml")],
            staging,
            "dependency resolution",
        )
        # `--locked` is what makes the refreshed lockfile the one that was built.
        # Without it `cargo update` resolves, the rebuild re-resolves, and the
        # lockfile promoted back is the only artifact in the coupled set that
        # nothing confirmed.
        run(
            [
                sys.executable,
                str(BUILD),
                "--profile",
                arguments.profile,
                "--manifest-path",
                str(staging / "Cargo.toml"),
                "--package",
                arguments.package,
                "--target-dir",
                target_dir,
                "--locked",
            ]
            + cache,
            staging,
            "component rebuild",
        )

        profile = next(
            entry for entry in record["profiles"] if entry["profile"] == arguments.profile
        )
        # Cargo names its output directory by the JSON specification's file stem
        # or by the triple itself, and a `[[bin]]` for a triple target has no
        # `.elf` suffix. Both come from the record rather than being assumed.
        stem = Path(profile["cargoTarget"]).stem
        release = Path(target_dir) / stem / "release"
        candidates = [release / f"{arguments.binary}.elf", release / arguments.binary]
        elf = next((path for path in candidates if path.is_file()), None)
        if elf is None:
            fail(f"rebuild produced no {arguments.binary} artifact in {release}")
        digest = hashlib.sha256(elf.read_bytes()).hexdigest()
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise

    # Promotion is rename-then-unlink, never delete-then-copy. The docstring's
    # atomicity promise has to survive this step too: a failure midway through a
    # delete-then-copy leaves the consumer with its old files gone and the new
    # ones half-written, which is the one state no retry can recover from.
    promote(consumer, staging)

    print(f"slime sdk update: pinned {arguments.sdk_commit} ({record['version']})")
    print(f"slime sdk update: {arguments.binary} contentHash {digest}")


if __name__ == "__main__":
    main()
'''


def template_workspace_manifest(record: dict, *, url: str, commit: str) -> str:
    dependency = f'{{ git = "{url}", rev = "{commit}" }}'
    return f"""# The canonical external component workspace (CP10).
#
# Every SDK dependency is pinned by full commit. A branch, a tag, a registry
# version, or a path into a `slime_os` checkout is not a pin, and
# `just component_sdk_upgrade_check` refuses each of them.
#
# Generated for Slime component SDK {record["version"]}.
[workspace]
resolver = "3"
members = ["component"]

[workspace.dependencies]
boot-contracts = {dependency}
slime-proto = {dependency}
slime-components = {dependency}
slime-rt = {dependency}
slime-build-support = {dependency}

[profile.release]
panic = "abort"
opt-level = "s"
codegen-units = 1
debug = false
"""


TEMPLATE_COMPONENT_MANIFEST = """[package]
name = "slime-external-component"
version = "0.1.0"
edition = "2024"
publish = false
rust-version = "1.96"
build = "build.rs"

[[bin]]
name = "external-component"
path = "src/main.rs"
test = false

[dependencies]
boot-contracts.workspace = true
slime-components.workspace = true
slime-proto.workspace = true
slime-rt.workspace = true

[build-dependencies]
slime-build-support.workspace = true
"""

TEMPLATE_BUILD_SCRIPT = """fn main() {
    // Compiles `SLIME_TARGET_PROFILE` into the image, which is what makes a
    // component target-qualified. `tools/sdk-build.py` exports it.
    slime_build_support::configure();
}
"""

TEMPLATE_COMPONENT_SOURCE = """#![no_std]
#![no_main]

//! The smallest complete external Slime component.
//!
//! It resolves what it needs from the authenticated root at run time rather
//! than compiling a manifest-derived table in, which is why this crate depends
//! on no generated per-composition file.

slime_rt::entry!(main);

/// `startup_arg` is the authenticated startup argument the root placed in this
/// thread's first parameter: the generation's boot action for the bootstrap
/// instance, zero for every other component.
fn main(startup_arg: u32) {
    let _ = startup_arg;
    slime_rt::debug_write(b"[external-component] ready\\n");
    // The boot action is a root-answered query, not a compiled constant, which
    // is what lets one component source enter more than one composition.
    match slime_rt::boot_action() {
        Ok(_) => slime_rt::debug_write(b"[external-component] boot action resolved\\n"),
        Err(_) => slime_rt::debug_write(b"[external-component] boot action refused\\n"),
    };
    slime_rt::debug_write(b"[external-component] done\\n");
}
"""


# --------------------------------------------------------------------------
# Prefix assets (CP8)
# --------------------------------------------------------------------------


def canonicalize_prefix(prefix: Path, source: Path) -> None:
    """Remove the source checkout's absolute path from an installed seL4 prefix.

    Two libsel4 headers are emitted by seL4's own Python generators and record
    their `.bf` input by absolute path in a leading comment. The compiler's
    `-ffile-prefix-map` does not reach them because nothing compiles them here.
    They are rewritten to the same `/slime/sel4` logical prefix the kernel build
    maps its debug paths to; every other host path is a refusal.
    """
    needle = str(source / "deps" / "sel4").encode("utf-8")
    replacement = LOGICAL_SEL4_SOURCE.encode("utf-8")
    for relative in CANONICALIZED_PREFIX_FILES:
        path = prefix / relative
        if not path.is_file():
            _fail(f"installed prefix is missing {relative}")
        content = path.read_bytes()
        if needle in content:
            path.write_bytes(content.replace(needle, replacement))


def assert_no_host_paths(root: Path, source: Path = ROOT) -> None:
    needles = host_path_needles(source)
    for path in tree_files(root):
        content = path.read_bytes()
        for needle in needles:
            if needle in content:
                relative = path.relative_to(root).as_posix()
                _fail(
                    f"{relative}: exported byte names the build-host path "
                    f"{needle.decode('utf-8', 'replace')!r}"
                )


def prefix_rebuild_recipe(profile: str) -> str:
    platform = PROFILE_PLATFORMS[profile]["platform"]
    return (
        f"python3 scripts/build/build-sel4.py --platform {platform} && "
        f"python3 scripts/check/check-sel4-pins.py --prefix --platform {platform}"
    )


def export_prefix_asset(
    sdk: Path, profile: str, table: dict, *, source: Path, prefix_source: Path
) -> dict:
    """Package one installed seL4 prefix as a content-addressed SDK asset.

    `prefix_source` is the checkout whose `build/` holds the installed prefixes.
    It is separate from `source` because an installed prefix is a build output
    and therefore absent from a fresh worktree of a recorded commit: CP7's
    reverse-drift check exports the recorded *source* from a worktree while
    taking the prefix from the checkout that built it. What binds the two is
    `sel4/pins.toml`, which the recorded source commit carries and which states
    the five artifact hashes the prefix must have -- verified below, so a
    substituted prefix is a refusal rather than a silent difference.
    """
    binding = PROFILE_PLATFORMS[profile]
    prefix = prefix_source / binding["prefix"]
    if not (prefix / "bin" / "kernel.elf").is_file():
        _fail(
            f"{profile}: no installed seL4 prefix at {binding['prefix']}; "
            f"run python3 scripts/build/build-sel4.py --platform {binding['platform']}"
        )
    observed = table[binding["pins"]]
    for relative, key in (
        ("bin/kernel.elf", "kernel_sha256"),
        ("libsel4/include/kernel/gen_config.json", "kernel_config_sha256"),
        ("libsel4/include/sel4/gen_config.json", "libsel4_config_sha256"),
        ("support/kernel.dtb", "dtb_sha256"),
        ("support/platform_gen.yaml", "platform_info_sha256"),
    ):
        digest = hashlib.sha256((prefix / relative).read_bytes()).hexdigest()
        if digest != observed[key]:
            _fail(
                f"{profile}: installed prefix {relative} is {digest}, but the source "
                f"commit's pins declare {observed[key]}"
            )
    staging = sdk / "prefixes" / f".{profile}-staging"
    if staging.exists():
        shutil.rmtree(staging)
    shutil.copytree(prefix, staging, ignore=COPY_IGNORE)
    canonicalize_prefix(staging, prefix_source)
    assert_no_host_paths(staging, prefix_source)
    archive_relative = f"prefixes/{profile}.tar"
    archive = sdk / archive_relative
    canonical_tar(staging, archive)
    asset = {
        "archive": archive_relative,
        "archiveHash": hashlib.sha256(archive.read_bytes()).hexdigest(),
        "treeHash": tree_digest(staging),
        "kernelHash": observed["kernel_sha256"],
        "kernelConfigHash": observed["kernel_config_sha256"],
        "libsel4ConfigHash": observed["libsel4_config_sha256"],
        "dtbHash": observed["dtb_sha256"],
        "platformInfoHash": observed["platform_info_sha256"],
        "rebuildRecipe": prefix_rebuild_recipe(profile),
    }
    shutil.rmtree(staging)
    return asset


# --------------------------------------------------------------------------
# The export
# --------------------------------------------------------------------------


def zti(value: object, indent: int = 0) -> str:
    """Render a decoded record as the Zutai instance syntax it decodes from."""
    padding = " " * indent
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        return json.dumps(value, ensure_ascii=True)
    if isinstance(value, list):
        if not value:
            return "[]"
        rows = "".join(f"{padding}  {zti(item, indent + 2)};\n" for item in value)
        return "[\n" + rows + padding + "]"
    if isinstance(value, dict):
        rows = "".join(
            f"{padding}  {key} = {zti(item, indent + 2)};\n" for key, item in value.items()
        )
        return "{\n" + rows + padding + "}"
    raise TypeError(type(value))


def normalize(record: dict) -> bytes:
    """The record's canonical byte form.

    Sorted-key, whitespace-free, ASCII-escaped UTF-8 JSON plus one trailing
    newline -- `contracts/interface-schema/v1`'s convention, reused verbatim
    rather than reimplemented, so an interface identity, a component identity,
    and an SDK release identity are computed one way.
    """
    return (
        json.dumps(record, sort_keys=True, separators=(",", ":"), ensure_ascii=True) + "\n"
    ).encode("utf-8")


def canonical(value: dict) -> dict:
    """The record as its canonical form orders it.

    `zti()` renders a mapping in insertion order, and the canonical form is
    sorted, so rendering the in-memory record directly produced a `.zti` a reader
    could not reproduce from the `.json` beside it. Both files are rendered from
    this one ordering instead, which is what lets a consumer bind the typed file
    to the identity rather than trusting it.
    """
    return json.loads(normalize(value).decode("utf-8"))


def _write_record(sdk: Path, record: dict, contract: ModuleType) -> tuple[bytes, bytes]:
    normalized = normalize(record)
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail("normalized release record exceeds bound")
    identity = hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).digest()
    (sdk / contract.NORMALIZED_FILE_NAME).write_bytes(normalized)
    (sdk / contract.IDENTITY_FILE_NAME).write_text(identity.hex() + "\n", encoding="utf-8")
    (sdk / contract.RECORD_FILE_NAME).write_text(
        zti(canonical(record)) + "\n", encoding="utf-8"
    )
    return normalized, identity


def export(
    destination: Path,
    *,
    version: str,
    sdk_repository: str,
    profiles: tuple[str, ...] = DEFAULT_PROFILES,
    source: Path = ROOT,
    prefix_source: Path | None = None,
    commit: str | None = None,
    repository: str | None = None,
    contract: ModuleType = default_contract,
) -> ExportedSdk:
    """Emit one deterministic SDK tree and its release record into `destination`.

    `destination` must not exist: an export into a populated directory would
    produce a tree whose identity depends on what was there first.
    """
    if _SEMVER.fullmatch(version) is None:
        _fail(f"SDK version must be MAJOR.MINOR.PATCH, got {version!r}")
    if len(version.encode("utf-8")) > contract.MAX_VERSION_BYTES:
        _fail("SDK version exceeds bound")
    if not profiles:
        _fail("an SDK release must declare at least one target profile")
    if len(profiles) > contract.MAX_PROFILES:
        _fail(f"SDK profile count exceeds {contract.MAX_PROFILES}")
    if len(set(profiles)) != len(profiles):
        _fail("an SDK release declares one target profile twice")
    unknown = [profile for profile in profiles if profile not in PROFILE_PLATFORMS]
    if unknown:
        _fail(f"unknown SDK target profile(s): {unknown}")
    if destination.exists():
        _fail(f"export destination already exists: {destination}")

    source = source.resolve()
    prefixes = (prefix_source or source).resolve()
    table = pins(source)
    resolved_commit = commit or source_commit(source)
    if _COMMIT.fullmatch(resolved_commit) is None:
        _fail(f"source commit is not a full SHA-1 identity: {resolved_commit!r}")

    destination.mkdir(parents=True)
    crate_roots: list[Path] = []
    for relative, _ in EXPORT_CRATES:
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source / relative, target, ignore=COPY_IGNORE)
        crate_roots.append(target)
    for relative in VENDORED:
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copytree(source / relative, target, ignore=COPY_IGNORE)
    specs = destination / TARGET_SPEC_SDK_DIR
    specs.mkdir(parents=True, exist_ok=True)
    for stem in TARGET_SPECS:
        shutil.copyfile(
            source / TARGET_SPEC_SOURCE_DIR / f"{stem}.json",
            destination / target_spec_sdk_path(stem),
        )
    linker = destination / LINKER_SCRIPT_SDK
    linker.mkdir()
    for relative in LINKER_SCRIPTS:
        shutil.copyfile(source / relative, linker / Path(relative).name)

    (destination / "Cargo.toml").write_text(sdk_workspace_manifest(source), encoding="utf-8")
    tools = destination / "tools"
    tools.mkdir()
    for name, body in (
        ("sdk-build.py", SDK_BUILD_ENTRY),
        ("sdk-update.py", SDK_UPDATE_ENTRY),
    ):
        entry = tools / name
        entry.write_text(body, encoding="utf-8")
        entry.chmod(0o755)

    profile_records = []
    for profile in profiles:
        binding = PROFILE_PLATFORMS[profile]
        is_spec = bool(binding["cargo_target_is_spec"])
        profile_records.append(
            {
                "profile": profile,
                "platform": binding["platform"],
                "cargoTarget": binding["cargo_target"],
                "cargoTargetIsSpec": is_spec,
                # Digest the specification this profile actually names, not a
                # single exported file: two architectures ship two
                # specifications, and binding both to one hash would let a
                # changed RV64 target leave every recorded digest untouched.
                # A triple has no specification bytes to bind, so the field is
                # empty rather than carrying a digest of an unrelated file.
                "targetSpecHash": (
                    hashlib.sha256(
                        (destination / str(binding["cargo_target"])).read_bytes()
                    ).hexdigest()
                    if is_spec
                    else ""
                ),
                "rustFlags": list(binding["rust_flags"]),
                "cargoFlags": list(binding["cargo_flags"]),
                "prefix": export_prefix_asset(
                    destination, profile, table, source=source, prefix_source=prefixes
                ),
            }
        )

    crates = []
    for relative, package in EXPORT_CRATES:
        manifest = tomllib.loads((destination / relative / "Cargo.toml").read_text("utf-8"))
        if manifest["package"]["name"] != package:
            _fail(f"{relative}: exported crate is not {package}")
        crates.append(
            {
                "name": package,
                "version": manifest["package"]["version"],
                "identity": tree_digest(destination / relative),
            }
        )
    if len(crates) > contract.MAX_CRATES:
        _fail(f"exported crate count exceeds {contract.MAX_CRATES}")

    contracts = contract_identities(
        exported_contract_paths(crate_roots, source), source, contract
    )
    files = sorted(
        [relative for relative, _ in EXPORT_CRATES]
        + list(VENDORED)
        + [target_spec_sdk_path(stem) for stem in TARGET_SPECS]
        + [LINKER_SCRIPT_SDK, *GENERATED_FILES]
        + [profile["prefix"]["archive"] for profile in profile_records]
        + list(contract.RECORD_FILE_NAMES)
    )
    if len(files) > contract.MAX_FILES:
        _fail(f"exported file set exceeds {contract.MAX_FILES} entries")

    toolchain = table["rust_sel4"]["toolchain"]
    sel4_pin = {
        "repository": table["sel4"]["repository"],
        "release": table["sel4"]["release"],
        "commit": table["sel4"]["commit"],
    }
    rust_sel4_pin = {
        "repository": table["rust_sel4"]["repository"],
        "release": table["rust_sel4"]["release"],
        "commit": table["rust_sel4"]["commit"],
    }
    by_name = {entry["name"]: entry["identity"] for entry in contracts}
    compatibility = {
        "syscallAbi": by_name["contracts/syscall-abi/v1"],
        "componentImage": by_name["contracts/component/v2"],
        "contractSet": set_digest(
            "contractSet", [(entry["name"], entry["identity"]) for entry in contracts]
        ),
        "publicApi": set_digest(
            "publicApi",
            [(entry["name"], entry["version"], entry["identity"]) for entry in crates],
        ),
        "toolchain": axis_digest("toolchain", toolchain),
        "rustSel4": axis_digest(
            "rustSel4",
            rust_sel4_pin["repository"],
            rust_sel4_pin["release"],
            rust_sel4_pin["commit"],
        ),
        "targetSpecSet": set_digest(
            "targetSpecSet",
            [
                (
                    entry["profile"],
                    entry["cargoTarget"],
                    entry["targetSpecHash"],
                    " ".join(entry["rustFlags"]),
                    " ".join(entry["cargoFlags"]),
                )
                for entry in profile_records
            ],
        ),
        "prefixSet": set_digest(
            "prefixSet",
            [
                (
                    entry["profile"],
                    entry["prefix"]["archiveHash"],
                    entry["prefix"]["treeHash"],
                    entry["prefix"]["kernelHash"],
                    entry["prefix"]["kernelConfigHash"],
                    entry["prefix"]["libsel4ConfigHash"],
                    entry["prefix"]["dtbHash"],
                    entry["prefix"]["platformInfoHash"],
                )
                for entry in profile_records
            ],
        ),
    }

    record = {
        "formatVersion": contract.FORMAT_VERSION,
        "version": version,
        "sourceRepository": repository or source_repository(source),
        "sourceCommit": resolved_commit,
        "sdkRepository": sdk_repository,
        "treeIdentity": "",
        "toolchain": toolchain,
        "sel4": sel4_pin,
        "rustSel4": rust_sel4_pin,
        "files": files,
        "crates": crates,
        "contracts": contracts,
        "profiles": profile_records,
        "compatibility": compatibility,
    }

    # The template pins the SDK repository by full commit, and the commit this
    # export becomes does not exist yet. The consumer-facing pin is therefore
    # `sourceCommit`'s SDK counterpart, written by the publisher: the template
    # ships with the release's own `sdkRepository` and a placeholder revision
    # `tools/sdk-update.py` replaces, which is exactly the update path CP10
    # exercises rather than a second mechanism.
    template = destination / "template"
    (template / "component" / "src").mkdir(parents=True)
    (template / "Cargo.toml").write_text(
        template_workspace_manifest(record, url=sdk_repository, commit="0" * 40),
        encoding="utf-8",
    )
    (template / "component" / "Cargo.toml").write_text(
        TEMPLATE_COMPONENT_MANIFEST, encoding="utf-8"
    )
    (template / "component" / "build.rs").write_text(TEMPLATE_BUILD_SCRIPT, encoding="utf-8")
    (template / "component" / "src" / "main.rs").write_text(
        TEMPLATE_COMPONENT_SOURCE, encoding="utf-8"
    )

    # The README quotes the record's crate, profile, and pin fields, so it is
    # written before the tree is digested. It deliberately quotes no digest of
    # this tree: a README that printed `treeIdentity` would put the digest
    # inside its own input.
    (destination / "README.md").write_text(sdk_readme(record), encoding="utf-8")
    assert_no_host_paths(destination, source)
    record["treeIdentity"] = tree_digest(
        destination, exclude=tuple(contract.RECORD_FILE_NAMES)
    )

    normalized, identity = _write_record(destination, record, contract)
    return ExportedSdk(
        root=destination, record=record, normalized=normalized, identity=identity
    )


# --------------------------------------------------------------------------
# Verifying an exported or cloned tree
# --------------------------------------------------------------------------


def load_record(sdk: Path, contract: ModuleType = default_contract) -> dict:
    """Read and self-check a release record from an exported or cloned tree.

    All three record files are bound to one identity. The `.zti` is the
    schema-typed artifact a consumer decodes through the contract, the `.json` is
    its canonical normalization, and the `.identity` is SHA-256 over that
    normalization under the contract's domain. Checking only the latter two would
    leave the typed file outside the chain -- a hand-written `.zti` naming another
    commit would pass, while the README this exporter generates tells consumers
    that file is the authoritative description.
    """
    normalized_path = sdk / contract.NORMALIZED_FILE_NAME
    identity_path = sdk / contract.IDENTITY_FILE_NAME
    record_path = sdk / contract.RECORD_FILE_NAME
    for path in (normalized_path, identity_path, record_path):
        if not path.is_file():
            _fail(f"SDK tree is missing {path.name}")
    normalized = normalized_path.read_bytes()
    if len(normalized) > contract.MAX_NORMALIZED_BYTES:
        _fail("normalized release record exceeds bound")
    record = json.loads(normalized.decode("utf-8"))
    if normalize(record) != normalized:
        _fail("release record is not in its canonical normalized form")
    expected = hashlib.sha256(contract.IDENTITY_DOMAIN + normalized).hexdigest()
    if identity_path.read_text(encoding="utf-8").strip() != expected:
        _fail("release record identity does not match its normalized bytes")
    if record_path.read_text(encoding="utf-8") != zti(canonical(record)) + "\n":
        _fail(f"{contract.RECORD_FILE_NAME} does not render the identified record")
    if record["formatVersion"] != contract.FORMAT_VERSION:
        _fail(f"unsupported release record format version {record['formatVersion']}")
    return record


def verify_tree(sdk: Path, record: dict, contract: ModuleType = default_contract) -> str:
    """Recompute an exported tree's identity and compare it to the record."""
    observed = tree_digest(sdk, exclude=tuple(contract.RECORD_FILE_NAMES) + (".git",))
    if observed != record["treeIdentity"]:
        _fail(
            "exported tree identity mismatch: record declares "
            f"{record['treeIdentity']} and the tree is {observed}"
        )
    return observed


def verify_digests(sdk: Path, record: dict, source: Path = ROOT) -> None:
    """Every digest the record states, recomputed against the emitted bytes."""
    for crate in record["crates"]:
        relative = next(path for path, name in EXPORT_CRATES if name == crate["name"])
        if tree_digest(sdk / relative) != crate["identity"]:
            _fail(f"{crate['name']}: recorded crate identity does not match its bytes")
    for entry in record["contracts"]:
        if tree_digest(source / entry["name"]) != entry["identity"]:
            _fail(f"{entry['name']}: recorded contract identity does not match its bytes")
    for profile in record["profiles"]:
        archive = sdk / profile["prefix"]["archive"]
        if not archive.is_file():
            _fail(f"{profile['profile']}: missing prefix archive")
        if hashlib.sha256(archive.read_bytes()).hexdigest() != profile["prefix"]["archiveHash"]:
            _fail(f"{profile['profile']}: prefix archive hash does not match its bytes")
        if not profile["cargoTargetIsSpec"]:
            # A triple has no bytes in the tree, so there is nothing to compare.
            # It is still checked: the record must not claim a hash for it.
            if profile["targetSpecHash"]:
                _fail(f"{profile['profile']}: a triple target declares a specification hash")
            continue
        target = sdk / profile["cargoTarget"]
        if hashlib.sha256(target.read_bytes()).hexdigest() != profile["targetSpecHash"]:
            _fail(f"{profile['profile']}: target specification hash does not match its bytes")


def verify_prefix_extraction(sdk: Path, record: dict, cache: Path) -> dict[str, Path]:
    """Extract every profile's prefix archive and check the recorded tree hash."""
    extracted: dict[str, Path] = {}
    for profile in record["profiles"]:
        destination = cache / profile["profile"]
        if destination.exists():
            shutil.rmtree(destination)
        extract_canonical_tar(sdk / profile["prefix"]["archive"], destination)
        observed = tree_digest(destination)
        if observed != profile["prefix"]["treeHash"]:
            _fail(
                f"{profile['profile']}: extracted prefix is {observed}, record declares "
                f"{profile['prefix']['treeHash']}"
            )
        extracted[profile["profile"]] = destination
    return extracted


def allowlisted(relative: str) -> bool:
    """Whether a repository-relative path is part of the exported source set.

    The exporter's own allowlist, asked as a question. A gate proving that an
    unrelated product-only file does not move the identity needs the same
    answer the exporter used, not a second list of directories.
    """
    roots = [path for path, _ in EXPORT_CRATES] + list(VENDORED) + ["Cargo.toml", "sel4/pins.toml"]
    return any(relative == root or relative.startswith(f"{root}/") for root in roots)


# --------------------------------------------------------------------------
# CP9: version policy and compatibility matrix
# --------------------------------------------------------------------------


def _semver(version: str) -> tuple[int, int, int]:
    match = _SEMVER.fullmatch(version)
    if match is None:
        _fail(f"SDK version must be MAJOR.MINOR.PATCH, got {version!r}")
    return (int(match.group(1)), int(match.group(2)), int(match.group(3)))


def changed_axes(
    previous: dict, current: dict, contract: ModuleType = default_contract
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    """Which compatibility axes moved, split into breaking and feature changes.

    Scalar axes are compared as digests: each names one thing that is either the
    same or not, so any difference is breaking.

    Structural axes -- the exported crates and the target profiles -- are keyed
    sets, compared entry by entry. A new key is a compatible feature; a changed
    or removed key is breaking. A digest over either set could only say
    "different", which would make adding a target profile a major release and so
    make the classification useless exactly where CP8 grows the platform set.
    """
    before = previous["compatibility"]
    after = current["compatibility"]
    breaking = [axis for axis in contract.BREAKING_AXES if before.get(axis) != after.get(axis)]
    feature: list[str] = []
    axes = tuple(zip(contract.STRUCTURAL_AXES, contract.STRUCTURAL_KEYS, strict=True))
    for axis, key in axes:
        old = {entry[key]: entry for entry in previous[axis]}
        new = {entry[key]: entry for entry in current[axis]}
        removed = sorted(set(old) - set(new))
        added = sorted(set(new) - set(old))
        mutated = sorted(name for name in set(old) & set(new) if old[name] != new[name])
        if removed:
            breaking.append(f"{axis}:removed:{','.join(removed)}")
        if mutated:
            breaking.append(f"{axis}:changed:{','.join(mutated)}")
        if added:
            feature.append(f"{axis}:added:{','.join(added)}")
    return tuple(breaking), tuple(feature)


def classify(
    previous: dict | None, current: dict, contract: ModuleType = default_contract
) -> str:
    """The change class a release's version must state.

    Crate versions are deliberately not consulted. Equal crate versions across a
    changed syscall ABI is exactly the claim CP9 refuses, so a classifier that
    read them would be able to agree with it.
    """
    if previous is None:
        return contract.CLASSIFICATION_INITIAL
    breaking, feature = changed_axes(previous, current, contract)
    if breaking:
        return contract.CLASSIFICATION_BREAKING
    if feature:
        return contract.CLASSIFICATION_COMPATIBLE_FEATURE
    return contract.CLASSIFICATION_PATCH


def bump(previous: str, current: str) -> str:
    """Which SemVer component a version change moved.

    Total over its inputs: the highest differing component names the change, and
    a version that does not advance is refused here rather than reported as a
    patch. Before this the answer depended on an ordering check in a *different*
    function, so `2.0.0 -> 1.5.0` came back `patch` for any second caller.
    """
    old = _semver(previous)
    new = _semver(current)
    if new == old:
        _fail(f"version {current} is unchanged from {previous}")
    if new < old:
        _fail(f"version {current} does not advance past {previous}")
    if new[0] != old[0]:
        return "major"
    if new[1] != old[1]:
        return "minor"
    return "patch"


def admit_version_change(
    previous: dict | None, current: dict, contract: ModuleType = default_contract
) -> str:
    """Refuse a release whose version understates its classification.

    One-directional on purpose: a release may overstate its change -- a major
    bump for a patch is conservative and loses nothing -- but a version that
    understates a changed identity is the failure CP9 exists to prevent.
    """
    classification = classify(previous, current, contract)
    if previous is None:
        return classification
    if _semver(current["version"]) <= _semver(previous["version"]):
        _fail(f"release {current['version']} does not advance past {previous['version']}")
    required = {
        contract.CLASSIFICATION_BREAKING: "major",
        contract.CLASSIFICATION_COMPATIBLE_FEATURE: "minor",
        contract.CLASSIFICATION_PATCH: "patch",
    }[classification]
    claimed = bump(previous["version"], current["version"])
    if _RANK[claimed] < _RANK[required]:
        breaking, feature = changed_axes(previous, current, contract)
        moved = ", ".join(breaking + feature)
        _fail(
            f"release {current['version']} claims a {claimed} change but "
            f"{classification} is required: {moved} changed"
        )
    return classification


_MATRIX_ROW_FIELDS = {
    "sdkVersion",
    "sdkCommit",
    "sdkIdentity",
    "productCommit",
    "profile",
    "classification",
    "status",
    "evidence",
}


def admit_matrix(table: dict, contract: ModuleType = default_contract) -> dict:
    """Every rule the contract states about a matrix, on one path.

    Called when a matrix is built *and* when one is read back, because the read
    path is the one a consumer or a later gate runs. The Zutai schema types every
    field as `Text`, so a matrix carrying an invented status or a branch name
    where a commit belongs decodes as `#valid`: the closed vocabularies and the
    immutable-commit rule are semantic admission, and a checker that applied them
    only on the way out would let a re-identified matrix answer `supported` for a
    pairing nobody exercised.
    """
    if set(table) != {"formatVersion", "rows"}:
        _fail(f"compatibility matrix declares unexpected fields: {sorted(table)}")
    if table["formatVersion"] != contract.FORMAT_VERSION:
        _fail(f"unsupported compatibility matrix version {table['formatVersion']}")
    rows = table["rows"]
    if not isinstance(rows, list):
        _fail("compatibility matrix rows are not a list")
    if len(rows) > contract.MAX_ROWS:
        _fail(f"compatibility matrix exceeds {contract.MAX_ROWS} rows")
    for row in rows:
        if not isinstance(row, dict) or set(row) != _MATRIX_ROW_FIELDS:
            _fail(f"compatibility row declares unexpected fields: {sorted(row)}")
        if row["classification"] not in contract.CLASSIFICATIONS:
            _fail(f"unknown classification {row['classification']!r}")
        if row["status"] not in contract.STATUSES:
            _fail(f"unknown support status {row['status']!r}")
        if row["profile"] not in PROFILE_PLATFORMS:
            _fail(f"matrix row names an unknown target profile: {row['profile']!r}")
        for field in ("sdkCommit", "productCommit"):
            if _COMMIT.fullmatch(row[field]) is None:
                _fail(f"matrix row names a non-immutable {field}: {row[field]!r}")
        if _SHA256.fullmatch(row["sdkIdentity"]) is None:
            _fail(f"matrix row names a malformed SDK identity: {row['sdkIdentity']!r}")
        if _SEMVER.fullmatch(row["sdkVersion"]) is None:
            _fail(f"matrix row names a malformed SDK version: {row['sdkVersion']!r}")
        evidence = row["evidence"]
        if not isinstance(evidence, list) or not evidence:
            _fail("a compatibility row must name the evidence that backs it")
        if len(evidence) > contract.MAX_EVIDENCE:
            _fail(f"compatibility evidence exceeds {contract.MAX_EVIDENCE} entries")
        for entry in evidence:
            if not isinstance(entry, str) or len(entry.encode("utf-8")) > contract.MAX_TEXT_BYTES:
                _fail(f"compatibility evidence entry is malformed or exceeds bound: {entry!r}")
    # Uniqueness is over the same key `supported` matches on: two rows naming one
    # exported-tree identity against one product commit and profile are one
    # pairing, whatever mirror commits they were published under.
    seen = {(row["sdkIdentity"], row["productCommit"], row["profile"]) for row in rows}
    if len(seen) != len(rows):
        _fail("compatibility matrix declares one pairing twice")
    return table


def matrix_row(
    record: dict,
    *,
    sdk_commit: str,
    product_commit: str,
    profile: str,
    classification: str,
    evidence: tuple[str, ...],
    contract: ModuleType = default_contract,
) -> dict:
    """One tested pairing. A row exists only where its gates were observed."""
    return {
        "sdkVersion": record["version"],
        "sdkCommit": sdk_commit,
        "sdkIdentity": record["treeIdentity"],
        "productCommit": product_commit,
        "profile": profile,
        "classification": classification,
        "status": contract.STATUS_SUPPORTED,
        "evidence": list(evidence),
    }


def matrix(rows: list[dict], contract: ModuleType = default_contract) -> dict:
    return admit_matrix(
        {"formatVersion": contract.FORMAT_VERSION, "rows": rows}, contract
    )


def supported(
    table: dict,
    *,
    sdk_identity: str,
    product_commit: str,
    profile: str,
) -> bool:
    """Whether a pairing is supported.

    Keyed on the *exported-tree identity*, not on the SDK repository's commit.
    The identity is what CP6 makes reproducible: anyone holding the recorded
    `slime_os` source commit can re-derive it, whereas a mirror commit depends on
    which repository the export was published to and cannot be resolved from a
    matrix alone. `sdkCommit` stays in the row as provenance -- it names where the
    tested artifact was published -- but it is not what a query matches on.

    Absence is unsupported. No version-range inference: a pairing nobody
    exercised is exactly the pairing this answer must not manufacture.
    """
    return any(
        row["sdkIdentity"] == sdk_identity
        and row["productCommit"] == product_commit
        and row["profile"] == profile
        for row in table["rows"]
    )


def write_matrix(path: Path, table: dict, contract: ModuleType = default_contract) -> str:
    """Write the matrix and its normalized form, returning the matrix identity."""
    normalized = normalize(table)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(zti(canonical(table)) + "\n", encoding="utf-8")
    path.with_suffix(".json").write_bytes(normalized)
    identity = hashlib.sha256(contract.MATRIX_IDENTITY_DOMAIN + normalized).hexdigest()
    path.with_suffix(".identity").write_text(identity + "\n", encoding="utf-8")
    return identity


def read_matrix(
    path: Path = MATRIX_PATH, contract: ModuleType = default_contract
) -> tuple[dict, str]:
    normalized_path = path.with_suffix(".json")
    identity_path = path.with_suffix(".identity")
    for candidate in (path, normalized_path, identity_path):
        if not candidate.is_file():
            _fail(f"compatibility matrix is missing {candidate.name}")
    normalized = normalized_path.read_bytes()
    table = json.loads(normalized.decode("utf-8"))
    if normalize(table) != normalized:
        _fail("compatibility matrix is not in its canonical normalized form")
    identity = hashlib.sha256(contract.MATRIX_IDENTITY_DOMAIN + normalized).hexdigest()
    if identity_path.read_text(encoding="utf-8").strip() != identity:
        _fail("compatibility matrix identity does not match its normalized bytes")
    if path.read_text(encoding="utf-8") != zti(canonical(table)) + "\n":
        _fail(f"{path.name} does not render the identified matrix")
    return admit_matrix(table, contract), identity
