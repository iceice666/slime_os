#!/usr/bin/env bash
set -euo pipefail

# Run inside the pinned Nix dev shell. The product builder uses --offline,
# including build-std's independent rust-src workspace on both toolchains.
toolchain="$(python3 -c \
  "import pathlib,tomllib; print(tomllib.loads(pathlib.Path('sel4/pins.toml').read_text())['rust_sel4']['toolchain'])")"
for manifest in \
  Cargo.toml \
  slime-root/child/Cargo.toml \
  deps/rust-sel4/Cargo.toml \
  deps/rust-sel4-bcm2712-rpi5/Cargo.toml \
  deps/rust-sel4-cv1800b-duo/Cargo.toml
do
  cargo fetch --locked --manifest-path "$manifest"
  RUSTUP_TOOLCHAIN="$toolchain" cargo fetch --locked --manifest-path "$manifest"
done

for tc in "$RUSTUP_TOOLCHAIN" "$toolchain"; do
  sysroot="$(RUSTUP_TOOLCHAIN="$tc" rustc --print sysroot)"
  library="$sysroot/lib/rustlib/src/rust/library/Cargo.toml"
  if [ ! -f "$library" ]; then
    echo "no rust-src library workspace at $library" >&2
    exit 1
  fi
  RUSTUP_TOOLCHAIN="$tc" cargo fetch --locked --manifest-path "$library"
done
