[private]
help:
    @just --choose

# === Run Targets ===

# Run kernel (dev profile) with serial on stdout.
run:
    cd kernel && cargo run

# Run kernel in release mode.
run_release:
    cd kernel && cargo run --release

# Run with a visible QEMU window (no -display none).
run_gui:
    cd kernel && cargo run

# Run kernel tests under QEMU; optimized code keeps boot-time integrity hashing bounded.
test:
    cd kernel && cargo test --release -- -display none


# M6.5: native generation inspection, staging, selection, and rollback.
generation_cmd_check: contracts_check generation_check
    python3 scripts/check-generation-commands.py


# M6.6: console chooser, user-gesture mint, narrow-only single-object grant,
# cancellation, bypass denial, and provenance event.
powerbox_check: contracts_check generation_check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test powerbox
    python3 scripts/check-powerbox.py

# M6.1: capability factories, narrow derive-copy spawn grants, bounded task
# accounting, supervision result shape, and generation-v2 determinism.
spawn_prereq_check: contracts_check generation_check
    cd kernel && cargo test --test spawn_authority -- -display none
# M6.2: generated spawn protocol, deterministic command profile, bounded
# userspace spawn service, profile rejection, and exact grant composition.
spawn_service_check: contracts_check generation_check
    python3 scripts/generate-spawn-bindings.py --check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test spawn
    ./scripts/build-storage-fixture.py /tmp/slime-os-spawn-service.img
    cd kernel && cargo run --release -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-spawn-service.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M6.4: native Dango command parser, explicit launch contexts, keyboard REPL,
# profile-mediated spawn, and structured termination reporting.
dango_check: contracts_check generation_check
    python3 scripts/check-dango.py

# M6.3: generated filesystem protocol, explicit transferable Directory
# authority, bounded immutable snapshots, and atomic namespace root commits.
directory_check: contracts_check generation_check
    cd components && cargo test --target x86_64-unknown-linux-gnu -p slime-proto --test fs
    ./scripts/check-directory.py /tmp/slime-os-directory.img

# M6.7: explicit block-capability generation transfer, bounded closure,
# durable pending selection, health promotion, and retained rollback root.
transfer_check: contracts_check generation_check
    python3 scripts/check-transfer.py

# M5.1: exercise the storage-capability foundation (PCI/DMA/cap/block-proto)
# under QEMU. Proves an unprivileged component cannot acquire device rights.
storage_cap_check:
    cd kernel && cargo test --test storage_capability -- -display none

# M5.2: attach a disposable read-only virtio block fixture and require the
# storage-probe component to read and verify sector zero through its capability.
storage_read_check:
    rm -f /tmp/slime-os-storage-read.img
    ./scripts/build-storage-fixture.py /tmp/slime-os-storage-read.img
    cd kernel && cargo run --release -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-storage-read.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M5.7: attach a disposable read-only NVMe namespace and require the existing
# capability-gated storage probe to verify it through the common block service.
storage_nvme_read_check:
    rm -f /tmp/slime-os-nvme-read.img
    ./scripts/build-storage-fixture.py /tmp/slime-os-nvme-read.img
    cd kernel && cargo run --release -- \
        -display none \
        -drive if=none,id=slime-nvme,format=raw,readonly=on,file=/tmp/slime-os-nvme-read.img \
        -device nvme,serial=slime-nvme,drive=slime-nvme

# M5.3: persist a bounded write, flush it, and verify it after a fresh boot.
storage_write_check:
    ./scripts/check-storage.py write /tmp/slime-os-storage-write.img

# M5.3: inject deterministic block failures and replay the recorded request.
storage_fault_check:
    ./scripts/check-storage.py fault /tmp/slime-os-storage-fault.img

# M5.4: GPT + integrity-checked object store: partition recovery, content-
# addressed retrieval, append/seal durability, and malformed-metadata
# rejection against disposable fixture images.
storage_store_check:
    ./scripts/check-storage.py store /tmp/slime-os-storage-store.img

# M5.6: consume pending attempts durably and return to known-good after failure.
rollback_check:
    cd kernel && cargo build
    ./scripts/check-rollback.py /tmp/slime-os-rollback.img

# Run with QEMU monitor on stdin.
monitor:
    cd kernel && cargo run -- -monitor stdio -serial null

# === Debug Targets ===

# Start QEMU paused with a gdb stub on port 1234.
debug_server:
    cd kernel && cargo run -- -s -S -serial stdio
    @echo "🌐 QEMU debug server on port 1234 (waiting for gdb/lldb)"

# Start LLDB and attach to the QEMU debug server.
debug_client:
    @echo "🔍 Starting LLDB debugging session..."
    ./debug.sh

# === Clean Targets ===

clean:
    cd kernel && cargo clean

clean_debug:
    cd kernel && cargo clean --profile dev

clean_release:
    cd kernel && cargo clean --release

# === Development Tools ===

fmt:
    cd kernel && cargo fmt

fmt_check:
    cd kernel && cargo fmt -- --check

fmt_components:
    cd components && cargo fmt

fmt_check_components:
    cd components && cargo fmt -- --check

# Regenerate Rust block protocol bindings from the Zutai schema.
block_gen:
    python3 scripts/generate-block-bindings.py

# Regenerate Rust component image bindings from the Zutai schema.
component_gen:
    python3 scripts/generate-component-bindings.py

# Regenerate Rust + component store protocol bindings from the Zutai schema.
store_gen:
    python3 scripts/generate-store-bindings.py

# Regenerate userspace spawn-service protocol bindings.
spawn_gen:
    python3 scripts/generate-spawn-bindings.py

# Regenerate kernel + component generation-management protocol bindings.
generation_management_gen:
    python3 scripts/generate-generation-management-bindings.py

# Regenerate userspace powerbox protocol bindings.
powerbox_gen:
    python3 scripts/generate-powerbox-bindings.py

# Regenerate host constants for generation v2, kernel image, and BootState.
boot_gen:
    python3 scripts/generate-boot-bindings.py

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
    cd kernel && cargo build
    ./scripts/check-bootstate-trace.py /tmp/slime-os-bootstate-trace.img

# M5.8: verify bounded threshold release authorization, replay protection,
# dual-authorized root rotation, failed-pending rollback, and promotion.
release_trust_check:
    cd kernel && cargo build
    ./scripts/check-release-trust.py

# M5.9: boot signed removable recovery, scrub a disposable repair target,
# reconstruct both BootState slots, and prove an ungranted disk is unchanged.
recovery_check:
    cd kernel && cargo build --release
    ./scripts/check-recovery.py

# Validate the pinned generation manifest schema and fixtures.
contracts_check: bootstate_model_check
    python3 scripts/check-contracts.py
    python3 scripts/generate-spawn-bindings.py --check

# Build and validate deterministic generation and redundant boot metadata.
generation_check:
    cd kernel && cargo build
    rm -rf /tmp/slime-os-generation-check-a /tmp/slime-os-generation-check-b
    ./scripts/build-generation.py kernel/target/x86_64-unknown-none/debug/slime_os-kernel /tmp/slime-os-generation-check-a
    ./scripts/build-generation.py kernel/target/x86_64-unknown-none/debug/slime_os-kernel /tmp/slime-os-generation-check-b
    cmp /tmp/slime-os-generation-check-a/generation-1.bin /tmp/slime-os-generation-check-b/generation-1.bin
    cmp /tmp/slime-os-generation-check-a/generation-2.bin /tmp/slime-os-generation-check-b/generation-2.bin
    cmp /tmp/slime-os-generation-check-a/boot-store.bin /tmp/slime-os-generation-check-b/boot-store.bin
    ./scripts/check-generation.py /tmp/slime-os-generation-check-a/boot-store.bin

# Prove Framework images grant no storage-write authority and contain no
# storage-write path even though disposable QEMU generations may opt in.
framework_safety_check:
    python3 scripts/check-no-storage-authority.py

# Build a removable-media UEFI image for Framework safe bring-up.
framework_usb_image output="/tmp/slime-os-framework.img": framework_safety_check
    cd kernel && cargo build --release
    kernel/scripts/build-iso.sh kernel/target/x86_64-unknown-none/release/slime_os-kernel {{output}} 128

# Destructively write a Slime OS image to a removable disk only.
framework_usb_write device output="/tmp/slime-os-framework.img":
    just framework_usb_image {{output}}
    sudo env "PATH=$PATH" scripts/write-removable-image.py {{output}} {{device}}

lint:
    cd kernel && cargo clippy --all-features -- -D warnings

lint_fix:
    cd kernel && cargo clippy --fix --all-features --allow-dirty

# components/ is no_std bare-metal with no test harness (like the kernel, it
# is QEMU-verified rather than cargo-test-verified), so --all-targets is
# deliberately omitted: it would try to build a std test harness that does
# not exist for this target.
lint_components:
    cd components && cargo clippy -- -D warnings

lint_fix_components:
    cd components && cargo clippy --fix --allow-dirty
