[private]
help:
    @just --choose

# === Run Targets ===

# Run kernel (dev profile) with serial on stdout.
run:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel

# Run kernel in release mode.
run_release:
    cd kernel && SLIME_INTERACTIVE=1 cargo run --release -p slime_os-kernel

# Run with a visible QEMU window (no -display none).
run_gui:
    cd kernel && SLIME_INTERACTIVE=1 cargo run -p slime_os-kernel

# Run kernel tests under QEMU; optimized code keeps boot-time integrity hashing bounded.
test:
    cd kernel && cargo test --release -p slime_os-kernel -- -display none


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
    cd kernel && cargo test --release -p slime_os-kernel --test fabric_manifest -- -display none

# C8.3: attenuated endpoint provisioning and control plane. A live userspace
# fabric service derives exact non-widening, non-transferable route endpoints
# from the authenticated generation graph; possession of route names or generic
# channel authority mints, widens, or delegates nothing.
fabric_authority_check: contracts_check generation_check
    python3 scripts/check/check-fabric-authority.py
    cd kernel && cargo test --release -p slime_os-kernel --test fabric_authority -- -display none

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
    cd kernel && cargo run --release -p slime_os-kernel -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-storage-read.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M5.7: attach a disposable read-only NVMe namespace and require the existing
# capability-gated storage probe to verify it through the common block service.
storage_nvme_read_check:
    rm -f /tmp/slime-os-nvme-read.img
    ./scripts/build/build-storage-fixture.py /tmp/slime-os-nvme-read.img
    cd kernel && cargo run --release -p slime_os-kernel -- \
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

# Validate the pinned generation manifest schema and fixtures.
contracts_check: bootstate_model_check
    python3 scripts/check/check-contracts.py
    python3 scripts/generate/generate-spawn-bindings.py --check

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

# components/ is no_std bare-metal with no test harness (like the kernel, it
# is QEMU-verified rather than cargo-test-verified), so --all-targets is
# deliberately omitted: it would try to build a std test harness that does
# not exist for this target.
lint_components:
    cd components && cargo clippy -p slime-rt -p slime-proto -p slime-components -- -D warnings

lint_fix_components:
    cd components && cargo clippy -p slime-rt -p slime-proto -p slime-components --fix --allow-dirty
