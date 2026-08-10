[private]
help:
    @just --choose

# === seL4 Product Targets ===
#
# The product runs on upstream seL4 16.0.0 (`deps/sel4`) with `slime-root` as
# the dynamic mechanism owner. All behavioral targets below boot that product
# or exercise its host-testable contracts.

# Verify every exact pin the seL4 product depends on: submodule commits and
# origins, the seL4 release, the rust-sel4 and workspace Rust toolchains, the
# root target spec bytes, and the qemu-arm-virt kernel configuration (hypervisor
# ON, MCS OFF, one node) against `sel4/pins.toml`. Fetches nothing: submodules
# must already be initialized and the pinned toolchain already installed.
sel4_pin_check:
    python3 scripts/check/check-sel4-pins.py

# Configure, build, and install seL4 for qemu-arm-virt, build the root child and
# root task against the pinned rust-sel4 target specs, build the loader, add the
# kernel+root payload, and write `build/slime-sel4.identity.json` recording the
# source, config, ELF, and image digests the boot gate re-checks.
sel4_qemu_image_check: sel4_pin_check
    python3 scripts/build/build-sel4.py --skip-pin-check

# Boot the packaged image on the pinned machine (`virt,virtualization=on`,
# cortex-a53, 1 CPU, 2048 MiB) and require the ordered generation, task, IPC,
# fault, and ready markers on serial. Rebuilds first so the booted bytes are the
# ones the identity manifest describes.
sel4_root_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-root-boot.py

# P5.2: boot the component-graph image, whose root task embeds the
# `aarch64-sel4-qemu-virt` generation, and require the ordered markers proving
# that its five native ELF components launch with their declared grants, that
# the root answers the operation surface they invoke, and that an unsupported
# operation returns a bounded Slime error with the caller still running.
#
# A separate image from `sel4_root_boot_check`'s: the two differ only in which
# generation the root task embeds, and each gate boots the artifact it asserts
# about so neither invalidates the other's evidence by being built last.
sel4_component_graph_check: sel4_pin_check
    python3 scripts/check/check-sel4-component-graph.py

# P5.3.1: build the channel-plane image, boot it, and require the ordered
# markers proving that channels are materialized from the generation's declared
# send/recv grants, that a component parked in `recv` is woken by its peer's
# send with a payload too large for the fast registers, that the queue-full and
# capability-transfer refusals are bounded Slime errors, and that every channel
# and held reply is reclaimed at teardown.
#
# A third image, beside `sel4_root_boot_check`'s and
# `sel4_component_graph_check`'s. All three differ only in which generation the
# root task embeds, and each gate boots the artifact it asserts about.
sel4_channel_check: sel4_pin_check
    python3 scripts/check/check-sel4-channel-plane.py

# P5.3.2: build the loan-plane image, boot it, and require the ordered markers
# proving that a sealed subrange is loaned to a receiver named by capability,
# mapped read-only and returned exactly once by the unmodified `sample-receiver`,
# that each of the four quota classes refuses at ceiling+1 against limits decoded
# from the generation rather than a hardcoded constant, that an unrelated holder
# is undisturbed, and that every loan, mapping, region, and in-flight capability
# is reclaimed at teardown.
#
# A fourth image, beside the three above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_loan_check: sel4_pin_check
    python3 scripts/check/check-sel4-loan-plane.py

# P5.3.3: build the spawn-plane image, boot it, and require the ordered markers
# proving that a component constructs a child from a grant-resolved executable,
# that the child receives its declared capabilities at the slots its own
# numbering names, that an ungranted or widened grant is refused with nothing
# constructed, and that the parent observes the child's termination through a
# supervision handle rather than an ambient task id.
#
# A fifth image, beside the four above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_spawn_check: sel4_pin_check
    python3 scripts/check/check-sel4-spawn-plane.py

# P5.3.4: build the sample-plane image, boot it, and require the frozen P5
# cutover transcript from the unmodified `sample-lender` and `sample-receiver`.
# Also requires that a spawned child is budgeted from the generation, that the
# declared spawn budget refuses a child past its ceiling, and that every loan,
# mapping, region, and window is reclaimed at teardown.
#
# A sixth image, beside the five above, on the same rule: each gate boots the
# artifact it asserts about.
sel4_sample_check: sel4_pin_check
    python3 scripts/check/check-sel4-sample-plane.py

# P5.5.2: build the stream-plane image, boot it, and require that the full C8.4
# stream plane runs on seL4 with every participant unmodified — two publishers,
# two subscribers, two routes, the >MAX_INLINE_BYTES descriptor and loan path,
# KEEP_LAST eviction under a stalled subscriber, and the frozen P5 cutover
# transcript. The transfer contract's subset test (B17) is observed here too.
#
# A seventh image, beside the six above, on the same rule: each gate boots the
# artifact it asserts about. It replaces P5.5.1's typed-fabric image, whose
# every assertion this one subsumes over a larger graph.
sel4_stream_check: sel4_pin_check
    python3 scripts/check/check-sel4-stream-plane.py

# B16: build the supervision-plane image, boot it, and require that a graph
# creating more tasks over its lifetime than `MAX_RECORDS` can hold at once
# still answers `supervision_status` correctly for every live handle —
# including one held across the crossing and one parked in `Transit` across it.
#
# An eighth image, on the same rule as the seven above: each gate boots the
# artifact it asserts about.
sel4_supervision_check: sel4_pin_check
    python3 scripts/check/check-sel4-supervision-plane.py

# B38: repeatedly spawn and reclaim more task lifetimes than the old monotonic
# root CSlot and ordinary-untyped watermarks could sustain.
sel4_reclamation_check: sel4_pin_check
    python3 scripts/check/check-sel4-reclamation-plane.py

# B22: build the channel-crossing image, boot it, and require that a graph
# minting more channels over its lifetime than `MAX_CHANNELS` holds at once
# still sends and receives on every live channel — including a pair held across
# the crossing and an end parked in `Transit` across it.
#
# A ninth image, on the same rule as the eight above: each gate boots the
# artifact it asserts about.
sel4_crossing_check: sel4_pin_check
    python3 scripts/check/check-sel4-crossing-plane.py

# B10 on seL4 (P5.4.10): init's resolved capability layout, per plane, against
# the fixtures frozen at product cutover.
#
# Boots every plane and reads one `[layout]` block each — a layout diff should
# be readable as a layout diff rather than inferred from a component failing,
# which is why this is separate from the planes' own gates.
#
# Regenerate with `just sel4_boot_layout_bless`; the resulting diff is the
# evidence that a layout change was intended.
sel4_boot_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py

sel4_boot_layout_bless: sel4_pin_check
    python3 scripts/check/check-sel4-boot-layout.py --bless

# B40: every child's CSpace against the generation's admitted plan.
#
# Boots the unmutated boot plane, then rebuilds the root once per injected
# mutation and requires the audit to refuse each. Here a boot that *succeeds*
# under a mutation is the failure: it means the audit cannot see that
# deviation. Covers a declared capability missing, an extra one in an
# undeclared slot, one at the wrong slot, one aliased into two slots, and one
# carrying broader rights than admitted.
#
# Six root builds and six boots, so it is slower than the plane gates and is
# not part of `just test`.
sel4_capability_layout_check: sel4_pin_check
    python3 scripts/check/check-sel4-capability-layout.py

# B25 and P5.4.6: build the bounded-call image, boot it, and require the full
# C8.6 native-call plane plus the seL4 composition that makes it possible:
# init mints authenticated control pairs and transfers each participant's
# supervision handle to the broker after spawn. Every spawned component must
# then reach one clean terminal status.
sel4_call_check: sel4_pin_check
    python3 scripts/check/check-sel4-call-plane.py

# P5.4.7 and C8.7: build the native-operation image, boot it, and require the
# full bounded-operation surface with the shared broker and participants —
# correlation, authority denials, restart determinism, cancellation races,
# explicit-time expiry, peer-death settlement, parent-vouched supervision, and
# one clean exit per spawned task.
sel4_operation_check: sel4_pin_check
    python3 scripts/check/check-sel4-operation-plane.py

# P5.4.8 and C8.8: build the visibility image, boot it, and require filtered
# introspection plus declared interposition with the shared broker and
# participants — three callers with different bounded views, an ungranted
# caller that infers nothing, a telemetry path reaching its subscriber only
# through the declared proxy, and proxy death leaving the unrelated route live.
sel4_visibility_check: sel4_pin_check
    python3 scripts/check/check-sel4-visibility-plane.py

# P5.4.9 and C8.10: build the full-graph image, boot it, and require every C8
# role to launch at once in one collision-free layout — the stream, call, and
# operation planes in disjoint slots, the fabric split into three bounded route
# workers, the unauthorized probe refused as its own task, and the whole graph
# coming to rest rather than finishing.
sel4_boot_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-plane.py

# P5.4.2a: boot the component-graph image with a virtio-blk device attached and
# require the root to reach it — retype a granule out of BootInfo device untyped
# memory, map it non-cacheably into its own VSpace, and identify the disk by
# register read. `sel4_root_boot_check` asserts the same probe finding nothing
# when no drive is attached; the pair is what makes the mechanism observed.
sel4_device_check: sel4_pin_check
    python3 scripts/check/check-sel4-device-plane.py

# P5.4.2c and M5.2/M5.3: boot the storage image with a virtio-blk device and
# require a *component* to move sectors through a capability its generation
# granted — a read returning the fixture's signature, a write and flush verified
# by read-back and confirmed durable in the image afterwards, and three refusal
# arms. The root-launched copy of the same component parks, so every arm is the
# spawned instance's.
sel4_storage_check: sel4_pin_check
    python3 scripts/check/check-sel4-storage-plane.py

# P5.4.2c and M5.4: boot the store image and require a *component* to validate a
# GPT, open a content-addressed object store, retrieve an object by hash with
# its payload re-verified, append a durable commit that preserves the previous
# root, deduplicate identical content, scrub every payload, and fall back to the
# older superblock when the newest is damaged. The oracle keeps all of this in
# the kernel; here the root mediates sectors and nothing else.
sel4_store_check: sel4_pin_check
    python3 scripts/check/check-sel4-store-plane.py

# P5.4.2c and M5.6: boot the rollback image and require a *component* to walk
# the BootState transition model on two durable slots — stage a pending
# generation, consume both attempts (the oracle's 2 -> 1 -> 0), roll back to
# known-good when they are exhausted, find rollback idempotent, refuse
# promotion with a wrong running identity or a stale release, and promote the
# running generation. Every commit is older-slot-first, so the previously
# selected root survives each transition.
sel4_rollback_check: sel4_pin_check
    python3 scripts/check/check-sel4-rollback-plane.py

# P5.4.2c and M5.9: boot the recovery image with TWO disks — the target, and a
# guard disk no capability names — and require a *component* to refuse two
# corrupt BootState slots, decode a signed recovery index, verify its whole
# state closure against the content-addressed store, and reconstruct a bootable
# root into both slots idempotently. The guard image is hashed before and after:
# M5.9 requires reconstruction to modify no device it was not explicitly
# granted, and only the image proves that.
sel4_recovery_plane_check: sel4_pin_check
    python3 scripts/check/check-sel4-recovery-plane.py

# P5.4.3 and M6.5: boot the generation image and require an *unprivileged*
# client to drive list, inspect, stage, select, and rollback through a
# management service that holds the plane's only block capability. Every
# refusal must leave BootState untouched — checked against the disk image, not
# just the status — and the client's own direct BlockTransact must be refused,
# because no slot it holds names a device.
sel4_generation_check: sel4_pin_check
    python3 scripts/check/check-sel4-generation-plane.py

# B35: build the immutable selector once, then boot fresh QEMU processes
# against one retained raw disk to prove durable attempt consumption,
# exhaustion rollback, health-only promotion, and sector-scoped mutation.
sel4_boot_selection_check: sel4_pin_check
    python3 scripts/check/check-sel4-boot-selection.py

# P5.4.3 and M6.3: boot the directory image and require a component holding one
# unscoped directory capability to derive narrower views that can neither escape
# their scope nor widen their rights, to be refused a stale commit and a scoped
# one, and to see its commits through every view of the shared namespace. The
# root owns this because a namespace root is unforgeable shared state with an
# atomic transition; what a directory contains stays in userspace.
sel4_directory_check: sel4_pin_check
    python3 scripts/check/check-sel4-directory-plane.py

# P5.4.3: `InputRead` mediation on its own, separate from M6.4's Dango session.
# A granted capability decodes the generation's scripted keys in order, an
# exhausted script ends its reader rather than blocking it — the arm that would
# have caught `WAIT_KIND_INPUT` resolving to a never-ready wait target — and a
# slot holding no input capability is refused.
sel4_input_check: sel4_pin_check
    python3 scripts/check/check-sel4-input-plane.py

# P5.4.3 and M6.6: boot the powerbox image and require a chooser holding
# directory authority the requester lacks to grant exactly one narrowed object
# capability on a selection gesture, with a provenance record — and to deny a
# request exceeding its own authority, refuse derivation past the granted
# scope, and mint nothing at all on cancellation. Both components are the
# oracle's, unmodified.
sel4_powerbox_check: sel4_pin_check
    python3 scripts/check/check-sel4-powerbox-plane.py

# P5.4.3 and M6.4: boot the dango image and require a scripted console session
# to resolve two commands through the generation's profile and launch both
# through the spawn service — the second carrying a derived working directory
# and a stdin endpoint — while an undeclared command is denied at resolution
# and a malformed line is a parse error. Every component is the oracle's.
sel4_dango_check: sel4_pin_check
    python3 scripts/check/check-sel4-dango-plane.py

# P5.4.3 and M6.7: boot the transfer image with TWO devices — a read-only source
# carrying the manifest and a writable receiver — and require a generation to
# cross: digest, object closure, and travel policy all verified before any
# write, staged pending without disturbing the known-good root, and promoted
# only on health confirmation. The source is compared byte for byte afterwards.
sel4_transfer_check: sel4_pin_check
    python3 scripts/check/check-sel4-transfer-plane.py

# P5.4.3 and M6.3's service half: boot the filesystem image and require the
# shared `directory-probe`, unmodified, to resolve names, survive an interrupted
# root transition, commit a new one, and derive a narrowed subdirectory through
# a seL4 filesystem service backed by a userspace object store. The client hands
# its own directory view across with every request.
sel4_filesystem_check: sel4_pin_check
    python3 scripts/check/check-sel4-filesystem-plane.py

# P5.4.5 on seL4: C8.5's declared QoS policy, on the `sel4-qos` plane.
# Separate from `sel4_stream_check` because `sel4-stream.zti` grants no time
# capability, so its simulated-time clause is structurally unreachable — the
# arms this asserts cannot fire there at all.
sel4_qos_check: sel4_pin_check
    python3 scripts/check/check-sel4-qos-plane.py


# Prove the seL4 plane gates fail when their evidence is absent. This drives
# each gate's own marker table with a deleted marker, a transposition, and an
# appended failure marker. Needs no build and no QEMU: it asserts that the
# assertions have teeth, not that any image boots.
sel4_gate_control_check:
    python3 scripts/check/check-sel4-gate-controls.py

# Run the seL4 product image interactively on the pinned QEMU machine.
run: sel4_qemu_image_check
    qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 \
        -m size=2048M -nographic -serial mon:stdio -kernel build/slime-sel4.elf

run_release: run

# === Development Tools ===

fmt:
    cargo fmt --all
    cargo fmt --manifest-path slime-root/child/Cargo.toml

fmt_check:
    cargo fmt --all --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml --check

fmt_components:
    cd components && cargo fmt -p slime-rt -p slime-proto -p slime-components

fmt_check_components:
    cd components && cargo fmt -p slime-rt -p slime-proto -p slime-components -- --check

fmt_stage0:
    cd stage0 && cargo fmt

fmt_check_stage0:
    cd stage0 && cargo fmt -- --check

fmt_boot_contracts:
    cd boot-contracts && cargo fmt

fmt_check_boot_contracts:
    cd boot-contracts && cargo fmt -- --check

fmt_sel4_root:
    cargo fmt -p slime-root
    cargo fmt --manifest-path slime-root/child/Cargo.toml

fmt_check_sel4_root:
    cargo fmt -p slime-root -- --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml -- --check

fmt_check_all: fmt_check

# Regenerate Rust block protocol bindings from the Zutai schema.
block_gen:
    python3 scripts/generate/generate-block-bindings.py

# Regenerate Rust component image bindings from the Zutai schema.
component_gen:
    python3 scripts/generate/generate-component-bindings.py

# Regenerate Rust + component store protocol bindings from the Zutai schema.
store_gen:
    python3 scripts/generate/generate-store-bindings.py

# Regenerate userspace spawn-service protocol bindings.
spawn_gen:
    python3 scripts/generate/generate-spawn-bindings.py

# Regenerate the sample-descriptor protocol bindings (C7.6).
sample_descriptor_gen:
    python3 scripts/generate/generate-sample-descriptor-bindings.py

# Regenerate native interface-schema compiler constants and Rust bindings (C8.1).
interface_schema_gen:
    python3 scripts/generate/generate-interface-schema-bindings.py

# Regenerate the capability-transfer protocol bindings (C8.3).
capability_transfer_gen:
    python3 scripts/generate/generate-capability-transfer-bindings.py

# Regenerate the fabric stream framing bindings (C8.4).
fabric_stream_gen:
    python3 scripts/generate/generate-fabric-stream-bindings.py

# Regenerate the fabric-graph resource bindings (C8.2); part of the boot set.
fabric_graph_gen: boot_gen

# Regenerate kernel + component generation-management protocol bindings.
generation_management_gen:
    python3 scripts/generate/generate-generation-management-bindings.py

# Regenerate userspace powerbox protocol bindings.
powerbox_gen:
    python3 scripts/generate/generate-powerbox-bindings.py

# Regenerate host constants for generation v2, kernel image, and BootState.
boot_gen:
    python3 scripts/generate/generate-boot-bindings.py

generation_gen: boot_gen

kernel_image_gen: boot_gen

bootstate_gen: boot_gen

# Exhaustively check the bounded BootState transition and interruption model.
bootstate_model_check:
    cargo build --release --manifest-path deps/zutai/Cargo.toml -q -p zutai-cli
    ZUTAI_STDLIB_ROOT=deps/zutai/stdlib deps/zutai/target/release/zutai-cli model-check contracts/bootstate/model/bootstate.zt

# The custom-kernel persistence gates were superseded by the userspace seL4
# planes. These aliases keep historical devlog gate identifiers resolvable.
bootstate_trace_check: bootstate_model_check sel4_rollback_check
    python3 scripts/check/check-bootstate-trace.py

release_trust_check: contracts_check sel4_rollback_check
    python3 scripts/check/check-release-trust.py

recovery_check: sel4_recovery_plane_check

# Validate devlog structure: entry layout, front matter, kind-required sections,
# roadmap/Justfile identifier resolution, index agreement, and link health.
devlog_check:
    python3 scripts/check/check-devlog.py

# RP0: exact Raspberry Pi 5 board/firmware/media and ROS 2 Jazzy DDSI-RTPS
# Profile 0 acceptance fixture; closed identifiers, finite resources, explicit
# authority, trace ordering, and distinct operator failure markers.
rpi5_ros2_demo_contract_check:
    python3 scripts/check/check-rpi5-ros2-demo-contract.py

# Validate the pinned generation manifest schema and fixtures.
contracts_check: bootstate_model_check
    python3 scripts/check/check-contracts.py
    python3 scripts/generate/generate-spawn-bindings.py --check
    python3 scripts/check/check-boot-layout-resource.py

# P0 target-profile and executable-artifact contract matrix.
architecture_contract_check: contracts_check
    python3 scripts/check/check-architecture-contract.py

rpi5_artifact_check: architecture_contract_check rpi5_ros2_demo_contract_check
    python3 scripts/check/check-rpi5-artifacts.py

# Historical architecture gate backed by a real neutral-source boundary scan.
aarch64_boot_check: sel4_root_boot_check

x86_portability_check:
    python3 scripts/check/check-architecture-portability.py

generation_check: contracts_check sel4_component_graph_check
    python3 scripts/check/check-generation-determinism.py

framework_safety_check:
    python3 scripts/check/check-framework-authority.py

# Physical Framework image production is intentionally unavailable. P4 remains
# blocked until a seL4 hardware image and observed removable-media boot exist.
# Historical devlog identifiers remain explicit aliases to their product gates.
test: sel4_root_boot_check sel4_component_graph_check sel4_gate_control_check

product_boot_check: sel4_component_graph_check

generation_cmd_check: sel4_generation_check

powerbox_check: sel4_powerbox_check

shared_buffer_factory_check: sel4_loan_check

shared_buffer_accounting_check: sel4_loan_check

shared_buffer_mapping_check: sel4_loan_check

shared_buffer_loan_check: sel4_loan_check

interface_schema_check: contracts_check
    python3 scripts/check/check-interface-schema.py

fabric_manifest_check: contracts_check sel4_stream_check
    python3 scripts/check/check-fabric-manifest.py

fabric_authority_check: sel4_stream_check

fabric_stream_check: sel4_stream_check

fabric_qos_check: sel4_qos_check

fabric_call_check: sel4_call_check

fabric_operation_check: sel4_operation_check

fabric_visibility_check: sel4_visibility_check

data_fabric_profile_check: contracts_check
    python3 scripts/check/check-data-fabric-profile.py

data_fabric_boot_check: sel4_boot_check

sample_descriptor_check: contracts_check sel4_sample_check

sample_plane_check: sel4_sample_check

sample_plane_live_check: sel4_sample_check

boot_layout_check: sel4_boot_layout_check

spawn_service_check: sel4_spawn_check

# Historical M5/M6 verification identifiers. The custom-kernel recipes are
# retired; these names resolve to the seL4 gates that now own the contracts.
spawn_prereq_check: sel4_spawn_check

storage_cap_check: sel4_storage_check sel4_generation_check sel4_transfer_check

# Physical Framework/NVMe qualification did not move to the seL4 QEMU product.
# Keep the documented identifiers, but fail closed rather than turning a
# missing physical transport/evidence gate into a false pass.
storage_nvme_read_check:
    @echo "storage_nvme_read_check: unavailable after custom-kernel retirement; M5.7 requires a seL4 NVMe path and observed Framework evidence" >&2
    @exit 1

framework_inventory_check:
    @echo "framework_inventory_check: unavailable after custom-kernel retirement; H1 requires a seL4 Framework image and evidence/framework-inventory.jsonl" >&2
    @exit 1

dango_check: sel4_dango_check

directory_check: sel4_filesystem_check

transfer_check: sel4_transfer_check

storage_read_check: sel4_storage_check

storage_write_check: sel4_storage_check

storage_fault_check: sel4_storage_check

storage_store_check: sel4_store_check

rollback_check: sel4_rollback_check

lint: lint_all


# Both stage-0 targets. The AArch64 loader is behind `cfg(target_arch)`, so
# linting only the x86 target leaves its ~450 lines — including the crate-level
# no-panic, no-unwrap, no-indexing denials this crate depends on — entirely
# unchecked.
lint_stage0:
    cd stage0 && cargo clippy --target x86_64-unknown-uefi -- -D warnings
    cd stage0 && cargo clippy --target aarch64-unknown-uefi -- -D warnings

lint_boot_contracts:
    cd boot-contracts && cargo clippy --all-features -- -D warnings

# Host-target clippy for every cutover component crate that does not require a
# built seL4 prefix. The product-target pass remains `lint_sel4_root`.
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
lint_sel4_root clippy_flags='-D warnings':
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "lint_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    child_elf="$PWD/build/sel4-cargo/child/aarch64-sel4-minimal/release/slime-root-child.elf"
    if [ ! -f "$child_elf" ]; then
        echo "lint_sel4_root: no root child ELF at $child_elf; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    toolchain="$(python3 -c "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
    targets="$PWD/deps/rust-sel4/support/targets"
    export SEL4_PREFIX="$prefix" RUSTUP_TOOLCHAIN="$toolchain" CHILD_ELF="$child_elf"
    build_std=(-Z json-target-spec -Z build-std=core,alloc,compiler_builtins -Z build-std-features=compiler-builtins-mem)
    cargo clippy -p slime-root --target "$targets/aarch64-sel4-roottask-minimal.json" \
        --target-dir build/sel4-cargo/lint-root "${build_std[@]}" -- {{clippy_flags}}
    cargo clippy --manifest-path slime-root/child/Cargo.toml -p slime-root-child \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir build/sel4-cargo/lint-child "${build_std[@]}" -- {{clippy_flags}}
    cd components
    SLIME_TARGET_PROFILE=aarch64-sel4-qemu-virt \
        cargo clippy -p slime-rt -p slime-proto -p slime-components \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir ../build/sel4-cargo/lint-components "${build_std[@]}" -- {{clippy_flags}}

# Every surviving workspace crate plus the seL4 product crates.
lint_all: lint_stage0 lint_boot_contracts lint_components_host lint_sel4_root

# Historical component lint identifiers now resolve to the product lint.
lint_components: lint_sel4_root

lint_fix_components: lint_sel4_root

# Dependency advisories (RUSTSEC), duplicate/wildcard bans, license
# allowlist, and source pinning. Config in deny.toml.
deny:
    cargo-deny check

# Unused-dependency scan; scoped to surviving workspace crates.
machete:
    cargo-machete boot-contracts components slime-root stage0

# UB check for the host-testable crates, on the actual host triple. Components
# has a bare-metal default target, so both invocations override it explicitly.
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
test_host:
    #!/usr/bin/env bash
    set -euo pipefail
    host="$(rustc -vV | sed -n 's/^host: //p')"
    cargo test --manifest-path boot-contracts/Cargo.toml --all-features
    (cd components && cargo test --target "$host" -p slime-proto)

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
test_sel4_root:
    #!/usr/bin/env bash
    set -euo pipefail
    prefix="$PWD/build/sel4-prefix"
    if [ ! -f "$prefix/libsel4/include/kernel/gen_config.json" ]; then
        echo "test_sel4_root: no installed seL4 prefix at $prefix; run 'just sel4_qemu_image_check' first" >&2
        exit 1
    fi
    expected=142
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
    echo "slime-root host tests: $actual/$expected across 13 modules"

# Python lint for the host-side build/check/generate scripts. Config in ruff.toml.
ruff:
    ruff check scripts/

ruff_fix:
    ruff check scripts/ --fix

# Spell-check sources and docs. Config in _typos.toml.
typos:
    typos

# Advisory correctness lint set documented by the workspace policy.
lint_pedantic:
    just lint_sel4_root '-D warnings -W clippy::undocumented_unsafe_blocks -W clippy::cast_possible_truncation'
