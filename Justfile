[private]
default:
    @just --list --unsorted

alias bk:= build_kernel
alias r := run
alias b := build

build_kernel:
    cd kernel && cargo build 

build:
    cd entry_point && cargo build

run:
    cd entry_point && cargo run