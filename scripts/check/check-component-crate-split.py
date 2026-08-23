#!/usr/bin/env python3
"""CP3: prove the crate-per-component boundary is real, not just rearranged.

Before CP3 every component was a `[[bin]]` of one crate whose `build.rs`
privately parsed a generation manifest, so no component could be built outside
that crate. This gate asserts the properties that make the split load-bearing
rather than cosmetic, each of which was a real failure mode measured while
landing it:

1. Every component is its own workspace package with exactly one binary, and the
   package/directory/binary names agree. A component invisible to
   `component_spec.workspace_binaries()` still builds, so its spec record would
   resolve `undeclared` while it ships -- the drift class B70 records.
2. No component crate carries a private manifest parser. `build.rs` may only
   call `slime-component-build`, which is the documented, shared entry point an
   out-of-tree crate depends on.
3. The allocator is scoped. Exactly the components that declare
   `boot-contracts/gpt` + `slime-rt/heap` are the ones the builder groups as
   store components, and no other crate declares either. Cargo unifies features
   across every package in one invocation, so a mismatch here silently links a
   `#[global_allocator]` into a component that never allocates.
4. Every component package has a `[profile.release.package]` stanza. Cargo's
   `"*"` wildcard is accepted but does not apply to workspace members, and a
   glob is rejected outright, so a new crate would silently build at
   `opt-level=3` -- a size regression in an image `slime-root` maps whole.
5. `components/bins` holds no shared source. A file two component crates both
   need lives in `components/lib`, reached as a library or by `#[path]`; one left
   behind in the old location would be dead or duplicated.
"""

from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts" / "lib"))

from component_spec import workspace_binaries  # noqa: E402
from harness import load_script  # noqa: E402

CRATE_ROOT = ROOT / "components" / "bins"
SHARED_LIB = ROOT / "components" / "lib"
BUILD_SUPPORT = ROOT / "components" / "build-support"
WORKSPACE = ROOT / "Cargo.toml"

BUILDER = load_script("component_crate_split_builder", "build/build-generation.py")

failures: list[str] = []


def fail(message: str) -> None:
    failures.append(message)


def manifest(path: Path) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8"))


crates = sorted(p for p in CRATE_ROOT.iterdir() if p.is_dir())

# 1. One crate per component, with agreeing names and exactly one binary.
#    `workspace_binaries()` raises on a mismatch, so calling it is itself the
#    assertion; what remains is that it sees every directory and that each
#    package name follows the convention an out-of-tree crate would copy.
binaries = dict(workspace_binaries())
if len(binaries) != len(crates):
    fail(
        f"{len(crates)} component crate directories but "
        f"{len(binaries)} discovered binaries; every directory must be a crate"
    )
for crate in crates:
    name = crate.name
    data = manifest(crate / "Cargo.toml")
    package = data["package"]["name"]
    if package != f"slime-component-{name}":
        fail(f"{name}: package is {package!r}, expected 'slime-component-{name}'")
    bins = data.get("bin", [])
    if len(bins) != 1 or bins[0]["name"] != name:
        fail(f"{name}: must declare exactly one [[bin]] named {name!r}")
    if data["package"].get("build") != "build.rs":
        fail(f"{name}: must declare build = \"build.rs\"")
    if "slime-build-support" not in data.get("build-dependencies", {}):
        fail(f"{name}: build script must depend on slime-build-support")

# 2. No component crate parses a manifest or compiles in per-plane data. The
#    whole point of extracting the parser is that this file is the same three
#    lines everywhere, so any other content is a private derivation growing
#    back. B70 deleted the last manifest-derived generators and the fabric
#    profile copier, so naming any of them here would re-admit helpers that no
#    longer exist.
ALLOWED_BUILD_CALLS = {"configure"}
for crate in crates:
    script = crate / "build.rs"
    if not script.exists():
        fail(f"{crate.name}: has no build.rs")
        continue
    text = script.read_text(encoding="utf-8")
    calls = set(re.findall(r"slime_build_support::(\w+)\(", text))
    stripped = re.sub(r"slime_build_support::\w+\(\);", "", text)
    stripped = re.sub(r"fn main\(\)\s*\{|\}|\s", "", stripped)
    if stripped:
        fail(
            f"{crate.name}: build.rs does more than call slime-build-support; "
            "a private manifest derivation belongs in components/build-support"
        )
    if not calls <= ALLOWED_BUILD_CALLS:
        fail(f"{crate.name}: build.rs calls unknown helpers {sorted(calls - ALLOWED_BUILD_CALLS)}")
    if "configure" not in calls:
        fail(f"{crate.name}: build.rs must call slime_build_support::configure()")

# 3. The allocator is scoped to the components that declare it, and the builder's
#    grouping matches. Read from the crates rather than restated here, so the two
#    cannot drift: the builder's set is the operand under test.
#
#    C10.3 makes this two independent groups, because there are now two
#    allocators and they are mutually exclusive: `slime-rt/heap` is the store
#    plane's fixed `.bss` bump allocator, `slime-rt/private-heap` is the free
#    list over the generation-declared private region, and `slime-rt/lib.rs`
#    refuses both in one link. So a crate declaring both is a compile error
#    waiting to happen, and the builder needs a third invocation rather than a
#    wider second one.
declared_store = set()
declared_private_heap = set()
for crate in crates:
    data = manifest(crate / "Cargo.toml")
    deps = data.get("dependencies", {})
    runtime_features = (deps.get("slime-rt") or {}).get("features", [])
    gpt = "gpt" in (deps.get("boot-contracts") or {}).get("features", [])
    heap = "heap" in runtime_features
    private_heap = "private-heap" in runtime_features
    if gpt != heap:
        fail(
            f"{crate.name}: declares only one of boot-contracts/gpt and slime-rt/heap; "
            "the object store and the allocator that backs it go together"
        )
    if heap and private_heap:
        fail(
            f"{crate.name}: declares both slime-rt/heap and slime-rt/private-heap; "
            "#[global_allocator] is one symbol per link, so a component picks one"
        )
    if gpt and heap:
        declared_store.add(crate.name)
    if private_heap:
        declared_private_heap.add(crate.name)
if declared_store != set(BUILDER.STORE_COMPONENTS):
    fail(
        "the crates declaring an allocator "
        f"({sorted(declared_store)}) are not the builder's store group "
        f"({sorted(BUILDER.STORE_COMPONENTS)}); a component gaining an allocator "
        "must move groups in the same change, or a plain component in its "
        "invocation silently links a #[global_allocator]"
    )
if declared_private_heap != set(BUILDER.PRIVATE_HEAP_COMPONENTS):
    fail(
        "the crates declaring the private-region allocator "
        f"({sorted(declared_private_heap)}) are not the builder's private-heap group "
        f"({sorted(BUILDER.PRIVATE_HEAP_COMPONENTS)}); the two allocators cannot "
        "coexist in one cargo invocation, so this mismatch fails the component "
        "build rather than mis-linking it"
    )
if declared_store & declared_private_heap:
    fail(
        "a crate is in both allocator groups: "
        f"{sorted(declared_store & declared_private_heap)}"
    )
# And the allocator must not reach the shared libraries, which every component
# links: a `heap`/`gpt` feature there would put it back in all 52.
for shared in (SHARED_LIB, BUILD_SUPPORT):
    text = (shared / "Cargo.toml").read_text(encoding="utf-8")
    if "heap" in text or '"gpt"' in text:
        fail(f"{shared.name}: must not enable the allocator features every component would inherit")

# 4. Every component package has its own release-profile stanza *carrying the
#    size-first settings*, because Cargo offers neither a wildcard that applies
#    to workspace members nor a glob package name.
#
#    The settings are compared, not merely counted. Asserting presence alone
#    would pass a stanza that said `opt-level = 3`, which is exactly the
#    regression this arm exists to catch. Parsed with `tomllib` rather than by
#    regex for the same reason: a regex over section headers cannot see a body,
#    and silently misses the equivalent inline-table spelling.
COMPONENT_RELEASE_PROFILE = {"opt-level": "s", "codegen-units": 1, "debug": False}
workspace = manifest(WORKSPACE)
profiles = workspace.get("profile", {}).get("release", {}).get("package", {})
profiled = {name for name in profiles if name.startswith("slime-component-")}
expected_profiled = {f"slime-component-{crate.name}" for crate in crates}
missing_profiles = sorted(expected_profiled - profiled)
if missing_profiles:
    fail(
        f"no [profile.release.package] stanza for {missing_profiles}; "
        "Cargo's \"*\" wildcard does not apply to workspace members and a glob is "
        "rejected, so an unlisted crate builds at opt-level=3"
    )
extra_profiles = sorted(profiled - expected_profiled)
if extra_profiles:
    fail(f"release-profile stanzas name components that do not exist: {extra_profiles}")
for name in sorted(profiled & expected_profiled):
    if profiles[name] != COMPONENT_RELEASE_PROFILE:
        fail(
            f"{name}: release profile is {profiles[name]}, expected "
            f"{COMPONENT_RELEASE_PROFILE}; a component image is mapped whole by "
            "slime-root, so its codegen settings are a size budget rather than a "
            "preference"
        )
# The libraries every component links carry the same settings, for the same
# reason: their code ends up inside those images.
for library in ("slime-components", "slime-proto", "slime-rt"):
    if profiles.get(library) != COMPONENT_RELEASE_PROFILE:
        fail(
            f"{library}: release profile is {profiles.get(library)}, expected "
            f"{COMPONENT_RELEASE_PROFILE}; it is linked into component images"
        )

# 4b. The shared build-support package must stay outside the `slime-component-*`
#     namespace. The builder and the lint recipe select the 52 component crates
#     by that glob, and this crate is a host-only build-script library that does
#     not compile for the component target: named `slime-component-build`, it
#     was swept into both and broke the seL4 clippy pass on a `std`-dependent
#     transitive dependency.
support_package = manifest(BUILD_SUPPORT / "Cargo.toml")["package"]["name"]
if support_package.startswith("slime-component-"):
    fail(
        f"build-support package is {support_package!r}; it must not match the "
        "'slime-component-*' glob the builder and lint recipe select components by"
    )

# 5. No shared source is left in the component tree. A file both crates need is
#    in components/lib; one here would be built into a single crate again.
for stray in sorted(CRATE_ROOT.glob("*.rs")):
    fail(f"{stray.name}: shared source belongs in components/lib, not components/bins")
if (CRATE_ROOT / "src").exists():
    fail("components/bins/src still exists; component sources live in per-crate directories")
if (CRATE_ROOT / "Cargo.toml").exists():
    fail("components/bins/Cargo.toml still exists; there is no shared component crate")

if failures:
    print("component crate split check: FAILED", file=sys.stderr)
    for message in failures:
        print(f"  - {message}", file=sys.stderr)
    raise SystemExit(1)

print(
    f"component crate split check: {len(crates)} component crates, each one workspace package with "
    f"one binary and no private manifest parser; {len(declared_store)} declare the store allocator "
    f"and {len(declared_private_heap)} the private-region allocator, each matching the builder's "
    "own build group; every package carries a release-profile stanza; no shared source remains in "
    "components/bins"
)
