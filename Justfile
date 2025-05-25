
[private]
help:
    @just --choose

CWD := `pwd`
KERNEL_DEBUG_PATH := CWD + "/kernel/target/x86_64-unknown-none/debug/slime_os-kernel"
KERNEL_RELEASE_PATH := CWD + "/kernel/target/x86_64-unknown-none/release/slime_os-kernel"
KERNEL_LN_PATH := "kernel/target/slime_os-kernel"

# === Core Build Targets ===

# Build kernel in release mode with optimizations
build_kernel_release:
    @echo "🔨 Building kernel (release mode)..."
    cd kernel && cargo build --release
    ln -sf {{KERNEL_RELEASE_PATH}} {{KERNEL_LN_PATH}}
    @echo "✅ Kernel build complete"

# Build kernel in debug mode for debugging
build_kernel_debug:
    @echo "🐛 Building kernel (debug mode)..."
    cd kernel && cargo build
    ln -sf {{KERNEL_DEBUG_PATH}} {{KERNEL_LN_PATH}}
    @echo "✅ Debug kernel build complete"

# Build kernel with test features enabled
build_kernel_test:
    @echo "🧪 Building kernel with test..."
    cd kernel && cargo build --release --features kernel_test
    ln -sf {{KERNEL_RELEASE_PATH}} {{KERNEL_LN_PATH}}
    @echo "✅ Test kernel build complete"


# === Run Targets ===

# Run kernel in release mode
run: build_kernel_release
    @echo "🚀 Starting SlimeOS (release)..."
    cd entry_point &&cargo run --release

# Run kernel tests
run_test: build_kernel_test
    @echo "🧪 Starting SlimeOS (test mode)..."
    cd entry_point && cargo run --release

# Run kernel in debug mode
run_debug: build_kernel_debug
    @echo "🐛 Starting SlimeOS (debug)..."
    cd entry_point && cargo run --release

# Run with QEMU monitor enabled for debugging
run_monitor: build_kernel_debug
    @echo "🖥️ Starting SlimeOS with QEMU monitor..."
    cd entry_point && cargo run --release -- -monitor stdio

# === Debug Targets ===

# Start LLDB debugging session
debug_client: build_kernel_debug
    @echo "🔍 Starting LLDB debugging session..."
    ./debug.sh

# Start QEMU with debug server
debug_server: build_kernel_debug
    @echo "🌐 Starting QEMU debug server on port 1234..."
    @echo "Connect with 'just debug_client' in another terminal"
    cd entry_point && cargo run -- -s -S

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

objdump:
    nm {{KERNEL_LN_PATH}} | rustfilt


# === Testing ===

# Run all tests
test:
    @echo "🧪 Running tests..."
    cd kernel && cargo test --features kernel_test
    @echo "✅ Tests complete"

# Run tests with output
test_verbose:
    @echo "🧪 Running tests (verbose)..."
    cd kernel && cargo test --features kernel_test -- --nocapture

# === Benchmarking & Performance ===

# Build with maximum optimizations for performance testing
build_perf:
    @echo "🏎️ Building with performance optimizations..."
    cd kernel && RUSTFLAGS="-C target-cpu=native -C opt-level=3 -C lto=fat" cargo build --release
    @echo "✅ Performance build complete"

# Run with performance monitoring
run_perf: build_perf
    @echo "📈 Running with performance monitoring..."
    cd entry_point && KERNEL_PATH={{KERNEL_RELEASE_PATH}} cargo run --release
