[private]
default:
    @just --list --unsorted

alias bk:= build_kernel

build_kernel:
    cd kernel && cargo build 


qemu:
    cargo run --bin qemu

debug:
    lldb -s debug.lldb
