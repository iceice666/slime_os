[private]
default:
    @just --list --unsorted

alias bk:=build_kernel

build_kernel:
    cd kernel && cargo build 

run:
    cargo +nightly run