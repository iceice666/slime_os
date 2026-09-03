[group('quality')]
sel4_gate_control_check:
    python3 scripts/check/check-sel4-gate-controls.py


[group('quality')]
fmt:
    cargo fmt --all
    cargo fmt --manifest-path slime-root/child/Cargo.toml

[group('quality')]
fmt_check:
    cargo fmt --all --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml --check

# Derive root-workspace component packages because `cargo fmt` supports neither
# package globs nor `--exclude`; metadata also picks up newly added components.
[group('quality')]
[private]
_component_packages:
    @cd components && cargo metadata --format-version 1 --no-deps \
        | python3 -c 'import json,sys; print(" ".join("-p " + p["name"] for p in json.load(sys.stdin)["packages"] if p["name"].startswith("slime-component") or p["name"] in {"slime-rt","slime-proto","slime-components","slime-build-support"}))'

[group('quality')]
fmt_components:
    cd components && cargo fmt $(just _component_packages)

[group('quality')]
fmt_check_components:
    cd components && cargo fmt $(just _component_packages) --check

[group('quality')]
fmt_boot_contracts:
    cd boot-contracts && cargo fmt

[group('quality')]
fmt_check_boot_contracts:
    cd boot-contracts && cargo fmt -- --check

[group('quality')]
fmt_sel4_root:
    cargo fmt -p slime-root
    cargo fmt --manifest-path slime-root/child/Cargo.toml

[group('quality')]
fmt_check_sel4_root:
    cargo fmt -p slime-root -- --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml -- --check

[group('quality')]
fmt_check_all: fmt_check

[group('quality')]
devlog_check:
    python3 scripts/check/check-devlog.py

[group('quality')]
x86_portability_check:
    python3 scripts/check/check-architecture-portability.py

[group('quality')]
framework_safety_check:
    python3 scripts/check/check-framework-authority.py

[group('quality')]
test: sel4_root_boot_check sel4_component_graph_check sel4_gate_control_check

[group('quality')]
lint: lint_all

[group('quality')]
lint_boot_contracts:
    cd boot-contracts && cargo clippy --all-features -- -D warnings

# Host-target lint excludes crates that require the built seL4 prefix.
[group('quality')]
lint_components_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo clippy -p slime-proto --target "$host" -- -D warnings

# Product-target lint requires the installed seL4 prefix, pinned toolchain,
# custom targets, and embedded child ELF; missing prerequisites fail closed.
[group('quality')]
lint_sel4_root clippy_flags='-D warnings':
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "lint_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # Child artifacts are platform-qualified; never accept a stale legacy path.
    child_elf="$PWD/build/sel4-cargo/qemu-arm-virt/child/aarch64-sel4-minimal/release/slime-root-child.elf"
    if [ ! -f "$child_elf" ]; then
        echo "lint_sel4_root: no root child ELF at $child_elf; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    targets="$PWD/deps/rust-sel4/support/targets"
    export SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" CHILD_ELF="$child_elf"
    build_std=(-Z json-target-spec -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem)
    cargo clippy -p slime-root --target "$targets/aarch64-sel4-roottask-minimal.json" \
        --target-dir build/sel4-cargo/lint-root "${build_std[@]}" -- {{ clippy_flags }}
    cargo clippy --manifest-path slime-root/child/Cargo.toml -p slime-root-child \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir build/sel4-cargo/lint-child "${build_std[@]}" -- {{ clippy_flags }}
    cd components
    # Keep allocator feature groups separate: `heap` and `private-heap` each
    # define a global allocator. Derive groups from package manifests.
    groups="$(cargo metadata --format-version 1 --no-deps | python3 -c '
    import json, sys
    packages = json.load(sys.stdin)["packages"]
    groups = {"plain": [], "heap": [], "private-heap": []}
    for package in packages:
        if not package["name"].startswith("slime-component-"):
            continue
        features = set()
        for dependency in package["dependencies"]:
            if dependency["name"] == "slime-rt":
                features.update(dependency.get("features") or [])
        key = "heap" if "heap" in features else "private-heap" if "private-heap" in features else "plain"
        groups[key].append(package["name"])
    for key, names in groups.items():
        if names:
            print(key + " " + " ".join("-p " + name for name in sorted(names)))
    ')"
    if ! grep -q '[^[:space:]]' <<< "$groups"; then
        echo "lint_sel4_root: cargo metadata named no slime-component-* package" >&2
        exit 1
    fi
    while read -r group packages; do
        [ -n "$group" ] || continue
        # Lint `slime-rt` in every feature group so allocator-gated modules are
        # checked; shared crates without allocator features stay in `plain`.
        shared="-p slime-rt"
        if [ "$group" = plain ]; then
            shared="$shared -p slime-proto -p slime-components"
        fi
        SLIME_TARGET_PROFILE=aarch64-sel4-qemu-virt \
            cargo clippy $shared $packages \
            --target "$targets/aarch64-sel4-minimal.json" \
            --target-dir "../build/sel4-cargo/lint-components-$group" "${build_std[@]}" -- {{ clippy_flags }}
    done <<< "$groups"

[group('quality')]
lint_all: lint_boot_contracts lint_components_host lint_sel4_root

[group('quality')]
lint_components: lint_sel4_root

[group('quality')]
lint_fix_components: lint_sel4_root

[group('quality')]
deny:
    cargo-deny check

[group('quality')]
machete:
    cargo-machete boot-contracts components slime-root

# Override the components bare-metal default with the actual host target.
[group('quality')]
miri:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cd boot-contracts
    cargo miri test --all-features --target "$host"
    cd ../components
    cargo miri test --target "$host" -p slime-proto

# Keep Kani proofs separate: `cargo-kani` uses its bundled toolchain.
[group('quality')]
kani_io_proofs:
    #!/usr/bin/env bash
    set -euo pipefail
    # Use `cargo-kani`: the Kani shell intentionally has no ambient `cargo`.
    if ! command -v cargo-kani >/dev/null 2>&1; then
        echo "kani_io_proofs: cargo-kani not found." >&2
        echo "  run: nix develop .#kani --command just kani_io_proofs" >&2
        exit 1
    fi
    # Enforce the harness count so a proof file cannot silently stop compiling.
    expected=18
    log="$(mktemp)"
    trap 'rm -f "$log"' EXIT
    cargo-kani --manifest-path verification/io-proofs/Cargo.toml 2>&1 | tee "$log"
    if ! grep -q "^VERIFICATION:- SUCCESSFUL" "$log"; then
        echo "kani_io_proofs: verification did not succeed" >&2
        exit 1
    fi
    got="$(sed -n 's/^Complete - \([0-9]*\) successfully verified harnesses.*/\1/p' "$log" | tail -1)"
    if [ "$got" != "$expected" ]; then
        echo "kani_io_proofs: expected $expected harnesses, ran ${got:-0}" >&2
        exit 1
    fi
    echo "kani_io_proofs: $got harnesses verified"

# Device-boundary proofs use separate manifests because Kani accepts one
# `[lib] path` per proof crate.
[group('quality')]
kani_virtio_proofs:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-kani >/dev/null 2>&1; then
        echo "kani_virtio_proofs: cargo-kani not found." >&2
        echo "  run: nix develop .#kani --command just kani_virtio_proofs" >&2
        exit 1
    fi
    # Enforce the harness count so the gate cannot silently verify nothing.
    expected=13
    log="$(mktemp)"
    trap 'rm -f "$log"' EXIT
    cargo-kani --manifest-path verification/virtio-proofs/Cargo.toml 2>&1 | tee "$log"
    if ! grep -q "^VERIFICATION:- SUCCESSFUL" "$log"; then
        echo "kani_virtio_proofs: verification did not succeed" >&2
        exit 1
    fi
    got="$(sed -n 's/^Complete - \([0-9]*\) successfully verified harnesses.*/\1/p' "$log" | tail -1)"
    if [ "$got" != "$expected" ]; then
        echo "kani_virtio_proofs: expected $expected harnesses, ran ${got:-0}" >&2
        exit 1
    fi
    echo "kani_virtio_proofs: $got harnesses verified"

# Override the components bare-metal default with the actual host target;
# host-only build-support tests need no target override.
[group('quality')]
test_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo test --manifest-path boot-contracts/Cargo.toml --all-features
    (cd components && cargo test --target "$host" -p slime-proto)
    # Disable the seL4-only runtime while testing host-safe device arithmetic.
    (cd components && cargo test --target "$host" -p slime-components --no-default-features)
    cargo test -p slime-build-support

# Run shipped root mechanism modules on the host. The generated seL4 config is
# still required at build time, so a missing installed prefix fails closed.
[group('quality')]
test_sel4_root:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "test_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # P3.E adds the sparse device-untyped preflight test: the retained virtual
    # anchor must participate in contiguous-slot planning without mutating the pool.
    expected=214
    # Use the toolchain pinned to the installed seL4 prefix, not the ambient one.
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    mkdir -p build
    capture="build/slime-root-host-tests.txt"
    # The installed prefix is AArch64 and libsel4's syscall headers carry
    # AArch64 inline assembly, so the tests must be compiled *for* AArch64. On
    # an AArch64 host that is the host target; on any other host they are
    # cross-compiled against the flake's cross toolchain and run under
    # qemu-aarch64 with that toolchain's glibc as the sysroot -- the same
    # binaries, the same count, on the same architecture the root ships on.
    case "$host" in
        aarch64-*) target="$host" ;;
        *)
            target="aarch64-unknown-linux-gnu"
            if [ -z "${CROSS_COMPILER_PREFIX:-}" ]; then
                echo "test_sel4_root: CROSS_COMPILER_PREFIX is unset; run inside 'nix develop'" >&2
                exit 1
            fi
            if ! command -v qemu-aarch64 >/dev/null 2>&1; then
                echo "test_sel4_root: qemu-aarch64 is missing; run inside 'nix develop'" >&2
                exit 1
            fi
            libc="$("${CROSS_COMPILER_PREFIX}gcc" -print-file-name=libc.so)"
            sysroot="$(dirname "$(dirname "$libc")")"
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER="${CROSS_COMPILER_PREFIX}gcc"
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L $sysroot"
            export CC_aarch64_unknown_linux_gnu="${CROSS_COMPILER_PREFIX}gcc"
            ;;

    esac
    # Do not use command substitution: `set -e` must preserve build failures,
    # and the per-checkout capture under `build/` avoids concurrent-run clashes.
    SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" \
        cargo test -p slime-root --target "$target" --lib -- --format=terse \
        | tee "$capture"
    output="$(cat "$capture")"
    actual="$(printf '%s\n' "$output" | sed -n 's/^running \([0-9]*\) test.*/\1/p' | head -1)"
    if [ -z "$actual" ]; then
        echo "test_sel4_root: cargo printed no 'running N tests' line; the harness did not start" >&2
        exit 1
    fi
    if [ "$actual" != "$expected" ]; then
        echo "test_sel4_root: ran $actual tests, expected $expected; a module lost or gained coverage" >&2
        exit 1
    fi
    # Require both discovery of the expected count and a fully passing result.
    if ! printf '%s\n' "$output" | grep -q "^test result: ok\. $expected passed; 0 failed"; then
        echo "test_sel4_root: the run did not report $expected passed and 0 failed" >&2
        exit 1
    fi
    echo "slime-root host tests: $actual/$expected across 19 modules"

[group('quality')]
ruff:
    ruff check scripts/

[group('quality')]
ruff_fix:
    ruff check scripts/ --fix

[group('quality')]
typos:
    typos

[group('quality')]
lint_pedantic:
    just lint_sel4_root '-D warnings -W clippy::undocumented_unsafe_blocks -W clippy::cast_possible_truncation'
