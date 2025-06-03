[private]
help:
    @just --choose


build_runner:
    cd entry_point && cargo build --release

# === Run Targets ===

# Run kernel in release mode
run: build_runner
    @echo "🚀 Starting SlimeOS (release)..."
    cd kernel && cargo run --release -- -a="-serial stdio"

# Run kernel tests
test: build_runner
    @echo "🧪 Starting SlimeOS (test mode)..."
    cd kernel && cargo test -- -a="-serial stdio -display none"


# Run with QEMU monitor enabled for debugging
monitor: build_runner
    @echo "🖥️ Starting SlimeOS with QEMU monitor..."
    cd kernel && cargo run --  -a="-monitor stdio"

# Run with performance monitoring
run_perf: build_runner
    @echo "📈 Running with performance monitoring..."
    cd kernel && RUSTFLAGS="-C target-cpu=native -C opt-level=3 -C lto=fat" cargo run --release -- -a="-serial stdio"

# === Debug Targets ===

# Start LLDB debugging session
debug_client:
    @echo "🔍 Starting LLDB debugging session..."
    ./debug.sh

# Start QEMU with debug server
debug_server:
    @echo "🌐 Starting QEMU debug server on port 1234..."
    @echo "Connect with 'just debug_client' in another terminal"
    cd kernel && cargo run -- -d

# === Clean Targets ===

# Clean all build artifacts
clean:
    @echo "🧹 Cleaning all build artifacts..."
    cd kernel && cargo clean
    cd entry_point && cargo clean
    @echo "✅ Clean complete"

# Clean only debug builds
clean_debug:
    @echo "🧹 Cleaning debug artifacts..."
    cd kernel && cargo clean --profile dev
    cd entry_point && cargo clean --profile dev

# Clean only release builds
clean_release:
    @echo "🧹 Cleaning release artifacts..."
    cd kernel && cargo clean --release
    cd entry_point && cargo clean --release

# === Development Tools ===

# Format all code
fmt:
    @echo "📝 Formatting code..."
    cd kernel && cargo fmt
    cd entry_point && cargo fmt
    @echo "✅ Formatting complete"

# Check code formatting
fmt_check:
    @echo "📋 Checking code formatting..."
    cd kernel && cargo fmt -- --check
    cd entry_point && cargo fmt -- --check
    @echo "✅ Format check complete"

# Run clippy linter
lint:
    @echo "🔍 Running clippy linter..."
    cd kernel && cargo clippy --all-features -- -D warnings
    cd entry_point && cargo clippy -- -D warnings
    @echo "✅ Lint check complete"

# Fix automatically fixable clippy issues
lint_fix:
    @echo "🔧 Auto-fixing clippy issues..."
    cd kernel && cargo clippy --fix --all-features --allow-dirty
    cd entry_point && cargo clippy --fix --allow-dirty




