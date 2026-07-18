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

# Validate the pinned generation manifest schema and fixtures.
contracts_check:
    python3 scripts/check-contracts.py

lint:
    cd kernel && cargo clippy --all-features -- -D warnings

lint_fix:
    cd kernel && cargo clippy --fix --all-features --allow-dirty
