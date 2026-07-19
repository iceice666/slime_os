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

# Run kernel tests under QEMU; exit code reflects test pass/fail.
test:
    cd kernel && cargo test -- -display none


# M5.1: exercise the storage-capability foundation (PCI/DMA/cap/block-proto)
# under QEMU. Proves an unprivileged component cannot acquire device rights.
storage_cap_check:
    cd kernel && cargo test --test storage_capability -- -display none

# M5.2: attach a disposable read-only virtio block fixture and require the
# storage-probe component to read and verify sector zero through its capability.
storage_read_check:
    rm -f /tmp/slime-os-storage-read.img
    ./scripts/build-storage-fixture.py /tmp/slime-os-storage-read.img
    cd kernel && cargo run -- \
        -display none \
        -drive if=none,id=slime-storage,format=raw,readonly=on,file=/tmp/slime-os-storage-read.img \
        -device virtio-blk-pci,drive=slime-storage,disable-legacy=on,queue-size=8

# M5.3: persist a bounded write, flush it, and verify it after a fresh boot.
storage_write_check:
    ./scripts/check-storage.py write /tmp/slime-os-storage-write.img

# M5.3: inject deterministic block failures and replay the recorded request.
storage_fault_check:
    ./scripts/check-storage.py fault /tmp/slime-os-storage-fault.img

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

# Regenerate Rust block protocol bindings from the Zutai schema.
block_gen:
    python3 scripts/generate-block-bindings.py

# Regenerate Rust component image bindings from the Zutai schema.
component_gen:
    python3 scripts/generate-component-bindings.py

# Validate the pinned generation manifest schema and fixtures.
contracts_check:
    python3 scripts/check-contracts.py

# Build and validate the deterministic boot generation binary.
generation_check:
    cd kernel && cargo build
    rm -rf /tmp/slime-os-generation-check
    ./scripts/build-generation.py kernel/target/x86_64-unknown-none/debug/slime_os-kernel /tmp/slime-os-generation-check
    ./scripts/check-generation.py /tmp/slime-os-generation-check/generation.bin

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
