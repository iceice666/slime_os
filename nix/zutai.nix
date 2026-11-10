# The pinned Zutai toolchain, as the `spec` shell and the default shell use it.
#
# `scripts/spec/` evaluates task/v2 items, area files and observation records
# with `zutai-cli` and its standard library. Contract generation and the schema
# checks build the `deps/zutai` submodule (`scripts/lib/zutai_cli.py`), but the
# judge, the work-item gates and the closeout bot run in jobs that check out no
# submodules and have no Rust toolchain, so the same revision is packaged here.
#
# `revision` is installed alongside the binaries so a reader can tell which
# compiler evaluated a record. Keep the flake input's revision equal to the
# submodule's.
{
  lib,
  llvmPackages,
  makeWrapper,
  rustPlatform,
  stdenv,
  revision,
  src,
}:
rustPlatform.buildRustPackage {
  pname = "zutai-toolchain";
  version = "0.1.0-${builtins.substring 0 7 revision}";
  inherit src;
  cargoLock.lockFile = "${src}/Cargo.lock";
  # Only the compiler driver and the runtime the driver links programs against.
  # The browser kernel, web tooling, and LSP are not part of this gate.
  cargoBuildFlags = [
    "-p"
    "zutai-cli"
    "-p"
    "zutai-rt"
  ];
  # The workspace's own test suites are qualified upstream, not re-run here.
  doCheck = false;
  nativeBuildInputs = [ makeWrapper ];
  postInstall = ''
    # `zutai-cli compile` probes ../lib/zutai/<rust target>/libzutai_rt.a
    # relative to its own executable, so the archive is installed exactly
    # there. Nix's host tuple is not always Rust's (`arm64-apple-darwin`
    # versus `aarch64-apple-darwin`), so the Rust target names the directory.
    install -Dm444 \
      "$(find target -name libzutai_rt.a -print -quit)" \
      "$out/lib/zutai/${stdenv.hostPlatform.rust.rustcTarget}/libzutai_rt.a"
    mkdir -p "$out/share/zutai"
    cp -r "$src/stdlib" "$out/share/zutai/stdlib"
    printf '%s\n' "${revision}" > "$out/share/zutai/revision"
    # `zutai-cli compile` drives `llc` and `clang` directly. They are bound
    # here rather than added to the dev shell, because the seL4 product build
    # resolves its own `clang` and linker from that PATH: a second LLVM there
    # broke `-fuse-ld=lld` for every cross-compiled component.
    wrapProgram "$out/bin/zutai-cli" \
      --set ZUTAI_LLC "${llvmPackages.llvm}/bin/llc" \
      --set ZUTAI_CLANG "${llvmPackages.clang}/bin/clang"
  '';
  meta = {
    description = "Zutai compiler, standard library, and native runtime at the pinned revision";
    mainProgram = "zutai-cli";
    platforms = lib.platforms.unix;
  };
}
