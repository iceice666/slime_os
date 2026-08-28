# Prove the seL4 plane gates fail when their evidence is absent. This drives
# each gate's own marker table with a deleted marker, a transposition, and an
# appended failure marker. Needs no build and no QEMU: it asserts that the
# assertions have teeth, not that any image boots.
[group('quality')]
sel4_gate_control_check:
    python3 scripts/check/check-sel4-gate-controls.py

# === Development Tools ===

[group('quality')]
fmt:
    cargo fmt --all
    cargo fmt --manifest-path slime-root/child/Cargo.toml

[group('quality')]
fmt_check:
    cargo fmt --all --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml --check

# The runtime, the protocol library, the shared component library, the
# build-support crate, and all 52 component crates.
#
# The package list is derived from `cargo metadata` rather than written out,
# because neither shortcut works: there is no `components/Cargo.toml` — the
# component crates are members of the *root* workspace through the
# `components/bins/*` glob — so `cargo fmt --all` from here resolves the root
# manifest and would also format `slime-root`, duplicating `fmt_sel4_root` and
# making this recipe's name a lie. And unlike `cargo build`/`clippy`, `cargo fmt`
# accepts neither a `-p` glob ("package `slime-component-*` is not a member of
# the workspace") nor `--exclude` ("unexpected argument"). Deriving the list
# keeps a new component crate from having to edit this recipe.
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

# Validate devlog structure: entry layout, front matter, kind-required sections,
# roadmap/Justfile identifier resolution, index agreement, and link health.
[group('quality')]
devlog_check:
    python3 scripts/check/check-devlog.py

[group('quality')]
x86_portability_check:
    python3 scripts/check/check-architecture-portability.py

[group('quality')]
framework_safety_check:
    python3 scripts/check/check-framework-authority.py

# Physical Framework image production is intentionally unavailable. P4's board
# image now builds (`just sel4_rpi5_image_check`, `just rpi5_media_check`); what
# P4 still lacks is the observed removable-media boot, which `just
# rpi5_boot_check` requires and never emulates.
# Historical devlog identifiers remain explicit aliases to their product gates.
[group('quality')]
test: sel4_root_boot_check sel4_component_graph_check sel4_gate_control_check

[group('quality')]
lint: lint_all

[group('quality')]
lint_boot_contracts:
    cd boot-contracts && cargo clippy --all-features -- -D warnings

# Host-target clippy for every cutover component crate that does not require a
# built seL4 prefix. The product-target pass remains `lint_sel4_root`.
[group('quality')]
lint_components_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo clippy -p slime-proto --target "$host" -- -D warnings

# Clippy for the seL4 product crates: the root task, its child, and the
# seL4-enabled component runtime. Unlike the format gates these compile, so
# they need the installed seL4 prefix (libsel4 headers and config), the
# rust-sel4 toolchain pin, the custom target specs, and the child ELF the root
# task embeds at compile time. `sel4_qemu_image_check` produces both; this gate
# refuses to run against a missing one rather than silently linting a
# different configuration.
[group('quality')]
lint_sel4_root clippy_flags='-D warnings':
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "lint_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # Platform-qualified since `0dd7d0c` gave the builder a second platform:
    # `build-sel4.py` writes each platform's child under
    # `build/sel4-cargo/<platform>/child/`. The unqualified path this named
    # before only resolved on a checkout still holding a pre-`0dd7d0c` build,
    # so the gate passed locally and refused on any clean tree.
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
    # Three invocations, not one, for the same reason the *builder* uses three:
    # Cargo unifies features across every package named in one invocation, and
    # `slime-rt`'s two allocators are mutually exclusive — `heap` and
    # `private-heap` each register a `#[global_allocator]`, and one link holds
    # one. A single `-p 'slime-component-*'` pass therefore enables both and dies
    # on "cannot define a new global allocator" rather than reporting lints.
    #
    # The groups are derived from `cargo metadata`, not listed here: a component
    # declaring an allocator moves group by editing its own manifest, and
    # `just component_crate_split_check` pins that the builder's grouping agrees
    # with what the crates declare.
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
        # `slime-rt` is named in *every* group, deliberately. Its two allocator
        # modules are behind the features this grouping exists to separate, so
        # naming it only in the plain group would cfg both of them out of every
        # lint pass — including the new unsafe pointer arithmetic in
        # `private_heap.rs`, which is the code in this tree that most needs
        # `just lint_pedantic`'s `undocumented_unsafe_blocks`. A path dependency
        # cargo merely checks is not linted. `slime-proto`/`slime-components`
        # declare no allocator feature, so they stay with the plain group alone.
        shared="-p slime-rt"
        if [ "$group" = plain ]; then
            shared="$shared -p slime-proto -p slime-components"
        fi
        SLIME_TARGET_PROFILE=aarch64-sel4-qemu-virt \
            cargo clippy $shared $packages \
            --target "$targets/aarch64-sel4-minimal.json" \
            --target-dir "../build/sel4-cargo/lint-components-$group" "${build_std[@]}" -- {{ clippy_flags }}
    done <<< "$groups"

# Every surviving workspace crate plus the seL4 product crates.
[group('quality')]
lint_all: lint_boot_contracts lint_components_host lint_sel4_root

# Historical component lint identifiers now resolve to the product lint.
[group('quality')]
lint_components: lint_sel4_root

[group('quality')]
lint_fix_components: lint_sel4_root

# Dependency advisories (RUSTSEC), duplicate/wildcard bans, license
# allowlist, and source pinning. Config in deny.toml.
[group('quality')]
deny:
    cargo-deny check

# Unused-dependency scan; scoped to surviving workspace crates.
[group('quality')]
machete:
    cargo-machete boot-contracts components slime-root

# UB check for the host-testable crates, on the actual host triple. Components
# has a bare-metal default target, so both invocations override it explicitly.
[group('quality')]
miri:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cd boot-contracts
    cargo miri test --all-features --target "$host"
    cd ../components
    cargo miri test --target "$host" -p slime-proto

# Host-side unit tests for the crates that need neither QEMU nor a built seL4
# prefix. Use the actual host triple: hardcoding Linux makes the gate fail on
# Darwin, while omitting `--target` makes components pick its bare-metal default.
#
# `slime-build-support` is a host crate, so its tests need no `--target`
# override. They are here because CP3 is what made them runnable at all: while
# the manifest parser lived in `components/bins/build.rs`, `cargo test` never
# built it as a test target, so its two `#[test]`s were compiled and run by
# nothing — and had rotted into asserting real slot numbers against a fixture
# that parsed as a single block, so they could only have failed. A parser with
# no test is how a wrong slot number reaches a component image.
[group('quality')]
test_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo test --manifest-path boot-contracts/Cargo.toml --all-features
    (cd components && cargo test --target "$host" -p slime-proto)
    cargo test -p slime-build-support

# B23: `slime-root`'s mechanism modules, run on the host.
#
# They were compiled by nothing and run by nothing until the crate grew a lib
# target: `main.rs` is unconditionally `no_std`/`no_main`, and the package built
# only for a seL4 JSON target with no `libtest`. The library is the same code
# the seL4 image links, so a pass here is evidence about the shipped root.
#
# Needs the installed seL4 prefix because `sel4` reads `libsel4`'s generated
# config at build time even on a host target; the gate refuses rather than
# silently skipping, on `lint_sel4_root`'s rule. It runs no seL4 syscall — the
# tests exercise the state machines, and behavior needing a live kernel stays
# the `sel4_*` gates' job.
#
# The count is asserted so a module that stops being covered is visible, which
# is B23's exit condition. Raise it deliberately when tests are added.
[group('quality')]
test_sel4_root:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "test_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    # B70: 129 -> 130. `generation` gained
    # `an_interposition_hop_resolves_to_the_declared_component_name`, covering
    # the root's resolution of a declared interposition hop's identity back to
    # a generation instance name. It replaces an `assert_declared_chain` that
    # lived in each fabric broker and compared a build-time table against a
    # constant compiled beside it.
    #
    # B70: 130 -> 131. `ipc` gained
    # `owned_minted_names_are_their_own_namespace`, covering the
    # `owned-minted:` axis that answers `mintedBindings` from the owner's side
    # rather than the holder's. It pins the dispatch property the two arms
    # depend on -- neither prefix is a prefix of the other -- so an owner's
    # question cannot be answered against a holder's instance index.
    #
    # C10.1: 131 -> 146. The new `private_memory` module contributes twelve
    # tests over the growth admission ordering, the deny-by-default region, the
    # quota clamp, the root-wide ceiling, reclamation idempotence, and the
    # arena frame plan; `child_vspace` goes from eight to nine for the reserved
    # window's guard and alignment; and `object_allocator` gains two for the
    # arena slot table's release invariant, which had no inverse before the
    # growth unwind needed one. Two of `child_vspace`'s existing headroom tests
    # were rewritten rather than added to, because the mapped span a child
    # receives now includes that window.
    #
    # C10.2: 146 -> 149. `generation` gains three, covering the private-memory
    # budget's admission against this root's own ceilings: a satisfiable budget
    # at exactly the per-task reservation is admitted, a quota above that
    # reservation is refused rather than clamped at growth, and B8's aggregate
    # arm refuses holders that each fit but cannot all peak at once.
    #
    # C10.4: 149 -> 152. `private_memory` gains three, covering the one shared
    # resource the two memory planes still have — the child's address space.
    # A mapping anywhere in the reservation overlaps even where no page is
    # backed yet (so the answer cannot depend on how far an allocator has
    # grown), a mapping touching either boundary from outside does not (so the
    # guard granule below the window stays usable), and a denied region
    # overlaps nothing (so a component with no declared quota is not a
    # component that may not map).
    #
    # C9.1: 152 -> 158. The new `clock` module contributes five tests over
    # independently grantable authorities, per-task timer quota isolation,
    # termination cleanup, reuse of live-authority slots across lifetime task
    # ids, and stale expiry suppression; `ipc` adds the sixth for exact clock
    # request shapes.
    #
    # C9.2: 158 -> 160. The new `wait_set` module contributes both: a task is
    # declared once and its row survives a source-free declaration (so
    # `clear_task` answers about a live task rather than about whether it was
    # ever declared), and the per-waiter supervision ceiling is the contract's
    # own source ceiling rather than a second number this module picked.
    # Direction 24: 183 -> 184. `graph` gains the capability-rights partition
    # test for root-only supervision rights and declared-but-ungated rights.
    # IO DMA join: 195 -> 200. `io_resource` gains four tests covering queue
    # and payload reclamation, lease-first and driver-first teardown, and queue
    # adapter rollback; `shared_buffer` gains the authenticated live-loan frame
    # accessor test.
    # IO1 restart: 200 -> 203. `io_resource` gains the three crash/restart
    # reclamation tests: every live MMIO mapping and IRQ binding emits its
    # teardown action in unbind-then-unmap order, repeated reclamation is
    # idempotent without re-issuing effects, and a predecessor-epoch handle
    # fails closed under the fresh epoch a replacement rebinds at.
    # IO2 spawn grants: 203 -> 205. `generation` gains the two crossing-grant
    # tests, and they exist because this rule cost two IO2 passes. Both had
    # concluded the root could not carry a crossing spawn grant to a
    # dynamically spawned child and reverted working migrations on that basis.
    # It can: `only_owner_supplied_authority_crosses_a_spawn` pins which
    # declarations an owner supplies -- owner-to-child yes, endpoint and
    # self-loop no, because those are authority the root places itself -- and
    # `a_declaration_the_root_places_is_never_charged_to_the_owner` pins the
    # direction that makes widening unrepresentable: the count comes from
    # declarations, so there is no index at which an owner can place a grant
    # the generation never declared.
    # Root slot budget: 205 -> 206. `object_allocator` gains
    # `physical_provenance_is_freed_on_release_and_fails_closed_when_full`. The
    # DMA join retained a frame's allocation-time physical base in a
    # `[usize; MAX_ROOT_CSLOTS]` array -- 2 MB of `.bss`, and since the loader
    # creates one root CSlot per `.bss` page, 512 root CSlots -- which dropped
    # free slots to 2185 and made the boot plane's 2313-slot generation
    # inadmissible. The replacement is bounded by the DMA-participating frame
    # population instead, so unlike the array it can fill, and this test pins
    # the three properties that makes load-bearing: a live frame's address is
    # retrievable, a released frame's is not, and a full table refuses the
    # allocation rather than losing a record and letting a device be handed
    # some other frame's address.
    # B83: 206 -> 207. `ipc` gains
    # `the_ordinal_notification_query_requires_a_role`. Removing the root's
    # block path needs the userspace driver declared once per device, and a
    # service declared twice cannot resolve its notifications by grant name:
    # grant names are globally unique with exactly one waiter each, so the two
    # instances are necessarily bound to differently-named grants and a driver
    # spelling one name would serve only the instance that got it. The
    # `notification:@<ordinal>+<role>` query answers positionally instead, and
    # this test pins the part that makes it safe: the role is required, because
    # "my first binding" spans two roles and identifies no slot.
    # B84: 207 -> 208. `io_resource` gains
    # `two_drivers_each_hold_their_own_device`. Two instances of one driver
    # executable, one device each, is the shape both two-disk planes boot; the
    # test pins that every per-device identity is derived from the declared
    # ordinal, because two instances asking for region 1 and source 1 -- what
    # the old fixed identities produced -- collide on the table's uniqueness
    # checks and the second install fails outright.
    expected=208
    # Pinned rather than ambient, on `lint_sel4_root`'s rule: this build
    # consumes the installed seL4 prefix, so it must use the toolchain that
    # prefix was produced against. `rust-toolchain.toml`'s default is a
    # different nightly, and a gate whose result depends on which shell you are
    # in is the property `sel4_pin_check` exists to prevent.
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    mkdir -p build
    capture="build/slime-root-host-tests.txt"
    # Not captured by a command substitution: a build failure must surface as
    # itself. `set -e` ends the recipe here rather than letting an empty capture
    # become a count mismatch reporting "ran  tests" for a compile error. The
    # transcript lands under `build/` rather than a fixed `/tmp` name, which two
    # checkouts or two concurrent runs would share.
    SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" \
        cargo test -p slime-root --target "$host" --lib -- --format=terse \
        | tee "$capture"
    output="$(cat "$capture")"
    actual="$(printf '%s\n' "$output" | sed -n 's/^running \([0-9]*\) test.*/\1/p' | head -1)"
    if [ -z "$actual" ]; then
        echo "test_sel4_root: cargo printed no 'running N tests' line; the harness did not start" >&2
        exit 1
    fi
    if [ "$actual" != "$expected" ]; then
        echo "test_sel4_root: ran $actual tests, expected $expected; a module lost or gained coverage (B23)" >&2
        exit 1
    fi
    # Belt and braces: the count says the harness found them, this says they
    # passed. `cargo test`'s own exit status covers it, but a filtered or
    # ignored run would still print a count.
    if ! printf '%s\n' "$output" | grep -q "^test result: ok\. $expected passed; 0 failed"; then
        echo "test_sel4_root: the run did not report $expected passed and 0 failed" >&2
        exit 1
    fi
    echo "slime-root host tests: $actual/$expected across 19 modules"

# Python lint for the host-side build/check/generate scripts. Config in ruff.toml.
[group('quality')]
ruff:
    ruff check scripts/

[group('quality')]
ruff_fix:
    ruff check scripts/ --fix

# Spell-check sources and docs. Config in _typos.toml.
[group('quality')]
typos:
    typos

# Advisory correctness lint set documented by the workspace policy.
[group('quality')]
lint_pedantic:
    just lint_sel4_root '-D warnings -W clippy::undocumented_unsafe_blocks -W clippy::cast_possible_truncation'
