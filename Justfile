[private]
default:
    @just --list --unsorted


run:
    cargo run --bin qemu --release

debug:
    cargo run --bin qemu

lldb:
    ./debug.sh