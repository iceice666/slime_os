[private]
help:
    @just --choose

# === seL4 Product Targets ===
#
# The product runs on upstream seL4 16.0.0 (`deps/sel4`) with `slime-root` as
# the dynamic mechanism owner. These three targets are the product path: pins,
# image, boot. The `legacy_*` targets below still drive the retained custom
# kernel as a semantic oracle and are removed once parity is signed off.

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

# P5.3.4: build the sample-plane image, boot it, and require the ordered
# transcript `just sample_plane_live_check` records on x86 — produced by the
# unmodified `sample-lender` and `sample-receiver`, which carry no seL4 branch.
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
# and KEEP_LAST eviction under a stalled subscriber — producing the transcript
# `fabric_stream_check` records on x86. The transfer contract's subset test
# (B17) is observed here too.
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

# B22: build the channel-crossing image, boot it, and require that a graph
# minting more channels over its lifetime than `MAX_CHANNELS` holds at once
# still sends and receives on every live channel — including a pair held across
# the crossing and an end parked in `Transit` across it.
#
# A ninth image, on the same rule as the eight above: each gate boots the
# artifact it asserts about.
sel4_crossing_check: sel4_pin_check
    python3 scripts/check/check-sel4-crossing-plane.py

# Run the seL4 product image interactively on the pinned QEMU machine.
run: sel4_qemu_image_check
    qemu-system-aarch64 -machine virt,virtualization=on -cpu cortex-a53 -smp 1 \
        -m size=2048M -nographic -serial mon:stdio -kernel build/slime-sel4.elf

run_release: run

# === Legacy Oracle Targets ===
#
# The custom kernel is retained only as the semantic oracle for the cutover.
# Nothing in the product path depends on it; these targets disappear with it.

# Run the legacy custom kernel (dev profile) with serial on stdout.
legacy_run:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel

# Run the legacy custom kernel in release mode.
legacy_run_release:
    cd kernel && SLIME_INTERACTIVE=1 cargo run --release -p slime_os-kernel

# Run the legacy kernel with a visible QEMU window (no -display none).
run_gui:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel

# Run legacy kernel tests under QEMU; optimized code keeps boot-time integrity
# hashing bounded. The kernel test binaries assert on the booted generation's
# fabric graph and health policy, so they select the boot profile that declares
# the verification scaffolding they describe (B11); `product_boot_check` covers
# the scaffolding-free legacy product boot.
test:
    cd kernel && SLIME_FABRIC_PROFILE=test cargo test --release -p slime_os-kernel -- -display none

# B11: the product boot profile declares only components the product needs — no
# probes, no scenario doubles. Boots it and requires the same healthy vertical
# slice the scaffolding profiles reach, so "the product generation still boots"
# is an observed result rather than an inference from the layout diff.
product_boot_check: contracts_check generation_check
    python3 scripts/check/check-product-boot.py


# M6.5: native generation inspection, staging, selection, and rollback.
generation_cmd_check: contracts_check generation_check
    python3 scripts/check/check-generation-commands.py


# M6.6: console chooser, user-gesture mint, narrow-only single-object grant,
# cancellation, bypass denial, and provenance event.
powerbox_check: contracts_check generation_check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test powerbox
    python3 scripts/check/check-powerbox.py

# M6.1: capability factories, narrow derive-copy spawn grants, bounded task
# accounting, supervision result shape, and generation-v2 determinism.
spawn_prereq_check: contracts_check generation_check
    cd kernel && cargo test -p slime_os-kernel --test spawn_authority -- -display none

# C7.2: shared-buffer factory authority, kernel-identified allocation under
# fixed global byte/object bounds, structured isolated exhaustion, and
# narrow-only buffer rights distinct from DMA authority.
shared_buffer_factory_check:
    cd kernel && cargo test -p slime_os-kernel --test shared_buffer_authority -- -display none
# C7.3: generation-declared per-holder shared-buffer quotas charged to the
# creating supervision subtree, structured per-holder exhaustion isolated from
# other holders, and full reclamation on subtree teardown.
shared_buffer_accounting_check:
    cd kernel && cargo test -p slime_os-kernel --test shared_buffer_accounting -- -display none
# C7.4: bounded shared-buffer mappings charged to manifest quota, exact-range
# PTE installation, irreversible read-only sealing, and lifecycle reclamation.
shared_buffer_mapping_check:
    cd kernel && cargo test -p slime_os-kernel --test shared_buffer_mapping -- -display none
# C7.5: bounded sealed-region loans, single-return identities, retained
# accounting, exact receiver mappings, and peer-fault reclamation.
shared_buffer_loan_check:
    cd kernel && cargo test -p slime_os-kernel --test shared_buffer_loan -- -display none
# C8.1: deterministic bounded native interface schemas, full identities,
# generation-local type tags, generated Stream/Call/Operation bindings, and
# pre-artifact collision rejection.
interface_schema_check: contracts_check generation_check
    python3 scripts/check/check-interface-schema.py
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test interface_schema

# C8.2: deterministic authenticated fabric-graph generation resource; exact
# route/grant authority tuples, bounded QoS, interposition chains without
# bypass, and per-entry plus aggregate admission before component launch.
fabric_manifest_check: contracts_check generation_check
    python3 scripts/check/check-fabric-manifest.py
    cd kernel && SLIME_FABRIC_PROFILE=test cargo test --release -p slime_os-kernel --test fabric_manifest -- -display none

# C8.3: attenuated endpoint provisioning and control plane. A live userspace
# fabric service derives exact non-widening, non-transferable route endpoints
# from the authenticated generation graph; possession of route names or generic
# channel authority mints, widens, or delegates nothing.
fabric_authority_check: contracts_check generation_check
    python3 scripts/check/check-fabric-authority.py
    cd kernel && SLIME_FABRIC_PROFILE=test cargo test --release -p slime_os-kernel --test fabric_authority -- -display none

# C8.4: bounded many-to-many streams. Two publishers and two subscribers
# exchange inline and >MAX_MSG samples over generation-declared routes;
# KEEP_LAST evicts the exact oldest sequence, a stalled BEST_EFFORT subscriber
# reports bounded loss without retry growth, and one large sample incurs one
# fabric copy plus one quota-charged receiver-bound loan per subscriber.
fabric_stream_check: contracts_check generation_check
    python3 scripts/check/check-fabric-stream.py
    cd kernel && SLIME_FABRIC_PROFILE=test cargo test --release -p slime_os-kernel --test fabric_stream -- -display none

# C8.5: bounded reliable/best-effort QoS, retained history, compatibility
# events, fixed retry exhaustion, and explicit simulated-time transitions.
fabric_qos_check: contracts_check generation_check
    python3 scripts/check/check-fabric-qos.py

fabric_qos_gen:
    python3 scripts/generate/generate-fabric-qos-bindings.py
    python3 scripts/generate/generate-fabric-time-bindings.py

# C8.6: generation/session-qualified bounded native calls with exact client and
# server authority, one terminal result, duplicate/stale suppression, timeout,
# cancellation, rejection, malformed reply, retry exhaustion, and peer death.
fabric_call_check: contracts_check generation_check
    python3 scripts/generate/generate-fabric-call-bindings.py --check
    python3 scripts/check/check-fabric-call.py

fabric_call_gen:
    python3 scripts/generate/generate-fabric-call-bindings.py

# C8.7: generation/session-qualified bounded native operations composed from a
# start-goal call, an operation-keyed feedback stream, a result call, and a
# cancellation request. Exact per-role authority, no cross-correlation between
# concurrent operations, duplicate goals and results suppressed, feedback after
# terminal state dropped, deterministic cancellation/expiry/timeout from the
# explicit time capability, and peer death leaving unrelated routes live.
fabric_operation_check: contracts_check generation_check
    python3 scripts/generate/generate-fabric-operation-bindings.py --check
    python3 scripts/check/check-fabric-operation.py

fabric_operation_gen:
    python3 scripts/generate/generate-fabric-operation-bindings.py

# C8.8: caller-filtered bounded graph introspection and explicit acyclic
# interposition with no direct bypass, narrowed proxy authority, deterministic
# trace records, and proxy failure isolated from unrelated routes.
fabric_visibility_check: contracts_check generation_check
    python3 scripts/generate/generate-fabric-visibility-bindings.py --check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test fabric_visibility
    python3 scripts/check/check-fabric-visibility.py

fabric_visibility_gen:
    python3 scripts/generate/generate-fabric-visibility-bindings.py

# C8.9: one typed generation source resolves the authenticated graph, the
# userspace build profile, every downstream limit, and the deterministic
# normalized-schema corpus; mutually unsatisfiable declarations fail closed.
data_fabric_profile_check: contracts_check generation_check
    python3 scripts/check/check-data-fabric-profile.py

# C8.10: one generation boots every C8 role at once through collision-free,
# bounded capability layouts — stream, call, and operation planes plus the
# unauthorized probe, declared interposition proxy, and filtered-introspection
# client as distinct identities — and every bounded route worker blocks on all
# of its declared sources without polling or exceeding kernel limits.
data_fabric_boot_check: contracts_check generation_check
    python3 scripts/check/check-data-fabric-boot.py

# C7.6: versioned sample descriptor over the C7.5 loan lifecycle; byte-identical
# binding round-trip, malformed-descriptor rejection before mapping, and a
# payload larger than MAX_MSG carried over descriptor plus shared buffer.
sample_descriptor_check: contracts_check
    cd kernel && cargo test -p slime_os-kernel --test sample_descriptor -- -display none
# C7.7: sample-plane integration and isolation. Two isolated holders exchange
# and return a payload larger than MAX_MSG through a quota-charged shared buffer
# over a real channel; malformed descriptors, every quota class, and peer death
# stay bounded, reclaim all resources, and disturb neither an unrelated channel
# nor the retained v2 known-good decode path.
sample_plane_check: contracts_check
    cd kernel && cargo test -p slime_os-kernel --test sample_plane -- -display none
# B5: the same sample plane driven by two real components over the actual
# SYS_SHARED_BUFFER_* syscalls, with capabilities granted by the generation.
# Complements sample_plane_check, which composes the lifecycle in-harness:
# this arm exercises the rights gates, the loan receiver binding, and
# reclamation through real task termination.
sample_plane_live_check: contracts_check generation_check
    python3 scripts/check/check-sample-plane.py
# B10: init's resolved capability layout, per boot profile, against frozen
# fixtures. The layout is a contract between the kernel that builds the table,
# the component images that address slots by number, and the gates that assert
# on what those components do; nothing else fails when the three disagree.
# Regenerate with `just boot_layout_bless` — the resulting diff is the evidence
# that a layout change was intended.
boot_layout_check: contracts_check generation_check
    python3 scripts/check/check-boot-layout.py
boot_layout_bless: contracts_check generation_check
    python3 scripts/check/check-boot-layout.py --bless
# M6.2: generated spawn protocol, deterministic command profile, bounded
# userspace spawn service, profile rejection, and exact grant composition.
spawn_service_check: contracts_check generation_check
    python3 scripts/generate/generate-spawn-bindings.py --check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test spawn
    ./scripts/build/build-storage-fixture.py /tmp/slime-os-spawn-service.img
    cd kernel && cargo run --release -p slime_os-kernel -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-spawn-service.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M6.4: native Dango command parser, explicit launch contexts, keyboard REPL,
# profile-mediated spawn, and structured termination reporting.
dango_check: contracts_check generation_check
    python3 scripts/check/check-dango.py

# M6.3: generated filesystem protocol, explicit transferable Directory
# authority, bounded immutable snapshots, and atomic namespace root commits.
directory_check: contracts_check generation_check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test fs
    ./scripts/check/check-directory.py /tmp/slime-os-directory.img

# M6.7: explicit block-capability generation transfer, bounded closure,
# durable pending selection, health promotion, and retained rollback root.
transfer_check: contracts_check generation_check
    python3 scripts/check/check-transfer.py

# M5.1: exercise the storage-capability foundation (PCI/DMA/cap/block-proto)
# under QEMU. Proves an unprivileged component cannot acquire device rights.
storage_cap_check:
    cd kernel && cargo test -p slime_os-kernel --test storage_capability -- -display none

# M5.2: attach a disposable read-only virtio block fixture and require the
# storage-probe component to read and verify sector zero through its capability.
storage_read_check:
    rm -f /tmp/slime-os-storage-read.img
    ./scripts/build/build-storage-fixture.py /tmp/slime-os-storage-read.img
    cd kernel && SLIME_FABRIC_PROFILE=test cargo run --release -p slime_os-kernel -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-storage-read.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M5.7: attach a disposable read-only NVMe namespace and require the existing
# capability-gated storage probe to verify it through the common block service.
storage_nvme_read_check:
    rm -f /tmp/slime-os-nvme-read.img
    ./scripts/build/build-storage-fixture.py /tmp/slime-os-nvme-read.img
    cd kernel && SLIME_FABRIC_PROFILE=test cargo run --release -p slime_os-kernel -- \
        -display none \
        -drive if=none,id=slime-nvme,format=raw,readonly=on,file=/tmp/slime-os-nvme-read.img \
        -device nvme,serial=slime-nvme,drive=slime-nvme

# M7.1: bounded, versioned hardware inventory; deterministic two-boot QEMU
# normalization; read-only NVMe identity; and safe Framework image build.
framework_inventory_check: framework_safety_check
    python3 scripts/check/check-framework-inventory.py

# Capture pre-boot internal-storage hashes before booting the physical image.
framework_inventory_prepare device output="/tmp/slime-os-framework-inventory.img":
    python3 scripts/check/check-framework-inventory.py --image {{output}} --prepare --storage-device {{device}}

# Append the physical serial report and verify the pre-boot hash is unchanged.
framework_inventory_record serial evidence="evidence/framework-inventory.jsonl" pending="/tmp/slime-framework-inventory-pending.json":
    python3 scripts/check/check-framework-inventory.py --record --pending {{pending}} --serial-log {{serial}} --evidence {{evidence}}

# M5.3: persist a bounded write, flush it, and verify it after a fresh boot.
storage_write_check:
    ./scripts/check/check-storage.py write /tmp/slime-os-storage-write.img

# M5.3: inject deterministic block failures and replay the recorded request.
storage_fault_check:
    ./scripts/check/check-storage.py fault /tmp/slime-os-storage-fault.img

# M5.4: GPT + integrity-checked object store: partition recovery, content-
# addressed retrieval, append/seal durability, and malformed-metadata
# rejection against disposable fixture images.
storage_store_check:
    ./scripts/check/check-storage.py store /tmp/slime-os-storage-store.img

# M5.6: consume pending attempts durably and return to known-good after failure.
rollback_check:
    cd kernel && cargo build --release -p slime_os-kernel
    ./scripts/check/check-rollback.py /tmp/slime-os-rollback.img

# Run with QEMU monitor on stdin.
monitor:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel -- -monitor stdio -serial null

# === Debug Targets ===

# Start QEMU paused with a gdb stub on port 1234.
debug_server:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel -- -s -S -serial stdio
    @echo "🌐 QEMU debug server on port 1234 (waiting for gdb/lldb)"

# Start LLDB and attach to the QEMU debug server.
debug_client:
    @echo "🔍 Starting LLDB debugging session..."
    ./tools/debug/lldb-attach.sh

# === Clean Targets ===

clean:
    cargo clean

clean_debug:
    cargo clean --profile dev

clean_release:
    cargo clean --release

# === Development Tools ===

fmt:
    cd kernel && cargo fmt -p slime_os-kernel

fmt_check:
    cd kernel && cargo fmt -p slime_os-kernel -- --check

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

# The seL4 root task and its child task. Formatting needs no seL4 prefix or
# cross toolchain, so these run with the default toolchain like every other
# crate; only compilation needs the rust-sel4 pin.
fmt_sel4_root:
    cargo fmt -p slime-root
    cargo fmt --manifest-path slime-root/child/Cargo.toml

fmt_check_sel4_root:
    cargo fmt -p slime-root -- --check
    cargo fmt --manifest-path slime-root/child/Cargo.toml -- --check

# Every crate's format gate, mirroring lint_all.
fmt_check_all: fmt_check fmt_check_components fmt_check_stage0 fmt_check_boot_contracts fmt_check_sel4_root

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

# M5.6c: validate durable BootState transition traces from the rollback
# power-cut scenario against the checked M5.6a/M5.6b state machines.
bootstate_trace_check:
    cd kernel && cargo build --release -p slime_os-kernel
    ./scripts/check/check-bootstate-trace.py /tmp/slime-os-bootstate-trace.img

# M5.8: verify bounded threshold release authorization, replay protection,
# dual-authorized root rotation, failed-pending rollback, and promotion.
release_trust_check:
    cd kernel && cargo build --release -p slime_os-kernel
    ./scripts/check/check-release-trust.py

# M5.9: boot signed removable recovery, scrub a disposable repair target,
# reconstruct both BootState slots, and prove an ungranted disk is unchanged.
recovery_check:
    cd kernel && cargo build --release -p slime_os-kernel
    ./scripts/check/check-recovery.py

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

# P0: one exact target profile per executable artifact, with closed admission,
# generated-binding agreement, and the retained x86 rollback-window identity.
architecture_contract_check: contracts_check
    python3 scripts/check/check-architecture-contract.py

# RP1: bind the RP0 DDS runtime and ROS node executable closure to the exact
# aarch64-rpi5 profile. Uses real generation/release encoders and admission
# checks to reject x86 and same-ISA QEMU artifacts, prove deterministic RPi5
# outputs, preserve neutral resource identity, and isolate profile build caches.
rpi5_artifact_check: architecture_contract_check rpi5_ros2_demo_contract_check
    python3 scripts/check/check-rpi5-artifacts.py

# P2.1: the first AArch64 boot. Builds a verified aarch64-qemu-virt generation
# and boots it under the pinned QEMU virt machine and AArch64 UEFI firmware,
# asserting ordered serial evidence that the kernel reached EL1 with the MMU and
# caches enabled, came up over the direct map, and saw the generation stage-0
# verified. Requires AAVMF firmware (exported by `nix develop`) and the
# aarch64-unknown-uefi/none Rust targets. Closes P2.1 only: no component runs
# and no syscall is served, so it is not evidence for the P2 parent.
aarch64_boot_check:
    python3 scripts/check/check-aarch64-boot.py

# P1: no x86 mechanism outside the architecture/platform boundary. Scans the
# neutral trees for x86 instructions, registers, ELF/linker constants, and
# undeclared profile dispatch, then *builds* the neutral kernel library and
# component runtime for aarch64-unknown-none. The build is the binding half:
# inline assembly is only validated during codegen, so `cargo check` would
# accept x86 assembly on an AArch64 target and make this gate vacuous. Requires
# `rustup target add aarch64-unknown-none`; it proves the boundary holds, not
# that AArch64 boots (that is P2).
x86_portability_check:
    python3 scripts/check/check-x86-portability.py

# Build and validate deterministic generation and redundant boot metadata.
generation_check:
    cd kernel && cargo build --release -p slime_os-kernel
    rm -rf /tmp/slime-os-generation-check-a /tmp/slime-os-generation-check-b
    ./scripts/build/build-generation.py target/x86_64-unknown-none/release/slime_os-kernel /tmp/slime-os-generation-check-a
    ./scripts/build/build-generation.py target/x86_64-unknown-none/release/slime_os-kernel /tmp/slime-os-generation-check-b
    cmp /tmp/slime-os-generation-check-a/generation-1.bin /tmp/slime-os-generation-check-b/generation-1.bin
    cmp /tmp/slime-os-generation-check-a/generation-2.bin /tmp/slime-os-generation-check-b/generation-2.bin
    cmp /tmp/slime-os-generation-check-a/boot-store.bin /tmp/slime-os-generation-check-b/boot-store.bin
    ./scripts/check/check-generation.py /tmp/slime-os-generation-check-a/boot-store.bin

# Prove Framework images grant no storage-write authority and contain no
# storage-write path even though disposable QEMU generations may opt in.
framework_safety_check:
    python3 scripts/check/check-no-storage-authority.py

# Build a removable-media UEFI image for Framework safe bring-up.
framework_usb_image output="/tmp/slime-os-framework.img": framework_safety_check
    cd kernel && cargo build --release -p slime_os-kernel
    kernel/scripts/build-iso.sh target/x86_64-unknown-none/release/slime_os-kernel {{output}} 128

# Destructively write a Slime OS image to a removable disk only.
framework_usb_write device output="/tmp/slime-os-framework.img":
    just framework_usb_image {{output}}
    sudo env "PATH=$PATH" scripts/build/write-removable-image.py {{output}} {{device}}

lint:
    cd kernel && cargo clippy -p slime_os-kernel --all-features -- -D warnings

lint_fix:
    cd kernel && cargo clippy -p slime_os-kernel --fix --all-features --allow-dirty

# Both stage-0 targets. The AArch64 loader is behind `cfg(target_arch)`, so
# linting only the x86 target leaves its ~450 lines — including the crate-level
# no-panic, no-unwrap, no-indexing denials this crate depends on — entirely
# unchecked.
lint_stage0:
    cd stage0 && cargo clippy --target x86_64-unknown-uefi -- -D warnings
    cd stage0 && cargo clippy --target aarch64-unknown-uefi -- -D warnings

lint_boot_contracts:
    cd boot-contracts && cargo clippy --all-features -- -D warnings

# Clippy for the seL4 product crates: the root task, its child, and the
# seL4-enabled component runtime. Unlike the format gates these compile, so
# they need the installed seL4 prefix (libsel4 headers and config), the
# rust-sel4 toolchain pin, the custom target specs, and the child ELF the root
# task embeds at compile time. `sel4_qemu_image_check` produces both; this gate
# refuses to run against a missing one rather than silently linting a
# different configuration.
lint_sel4_root:
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
        --target-dir build/sel4-cargo/lint-root "${build_std[@]}" -- -D warnings
    cargo clippy --manifest-path slime-root/child/Cargo.toml -p slime-root-child \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir build/sel4-cargo/lint-child "${build_std[@]}" -- -D warnings
    cd components && cargo clippy -p slime-rt --features sel4 \
        --target "$targets/aarch64-sel4-minimal.json" \
        --target-dir ../build/sel4-cargo/lint-runtime "${build_std[@]}" -- -D warnings

# Every crate's clippy gate: kernel, components, stage0, boot-contracts, and
# the seL4 product crates.
lint_all: lint lint_components lint_stage0 lint_boot_contracts lint_sel4_root

# Dependency advisories (RUSTSEC), duplicate/wildcard bans, license
# allowlist, and source pinning. Config in deny.toml.
deny:
    cargo-deny check

# Unused-dependency scan; scoped to workspace crates so submodules under
# deps/ (zutai, dango) do not pollute the result.
machete:
    cargo-machete boot-contracts components kernel stage0

# UB check for the host-testable crates. boot-contracts covers the
# verified-boot decode/crypto path; slime-proto covers wire validation.
# kernel/stage0 are QEMU-only and cannot run under Miri.
miri:
    cd boot-contracts && cargo miri test --all-features --target x86_64-unknown-linux-gnu
    cd components && cargo miri test --target x86_64-unknown-linux-gnu -p slime-proto

# Host-side unit tests for the crates that need neither QEMU nor a built seL4
# prefix.
#
# `slime-root`'s tests are deliberately *not* here: they need the installed
# libsel4 headers to compile at all, which this job's CI runner does not have.
# `just test_sel4_root` is their gate, and it runs on the machine that builds
# the image.
test_host:
    cd boot-contracts && cargo test --all-features
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto

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
    expected=102
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

# Advisory-only lints with known existing hits (missing SAFETY comments,
# lossy casts). Not part of `lint_all`; burn the backlog down module by
# module, then promote a lint into [workspace.lints.clippy] once clean.
lint_pedantic:
    cd kernel && cargo clippy -p slime_os-kernel --all-features -- \
        -W clippy::undocumented_unsafe_blocks \
        -W clippy::cast_possible_truncation \
        -W clippy::cast_sign_loss \
        -W clippy::cast_possible_wrap \
        -W clippy::arithmetic_side_effects

# components/ is no_std bare-metal with no test harness (like the kernel, it
# is QEMU-verified rather than cargo-test-verified), so --all-targets is
# deliberately omitted: it would try to build a std test harness that does
# not exist for this target.
lint_components:
    cd components && cargo clippy -p slime-rt -p slime-proto -p slime-components -- -D warnings

lint_fix_components:
    cd components && cargo clippy -p slime-rt -p slime-proto -p slime-components --fix --allow-dirty
