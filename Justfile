[private]
default:
    @just --list --unsorted

alias bk := build_kernel
alias bt := build_kernel_test
alias bd := build_kernel_debug
alias r := run
alias rt := run_test
alias rd := run_debug

build_kernel:
    cd kernel && cargo build --release

build_kernel_test:
    cd kernel && cargo build --features kernel_test

build_kernel_debug:
    cd kernel && cargo build --debug

run: build_kernel
    cd entry_point && cargo run

run_test: build_kernel_test
    cd entry_point && cargo run --release

run_debug: build_kernel_debug
    cd entry_point && cargo run

lldb:
    ./debug.sh
