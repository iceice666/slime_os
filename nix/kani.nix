# Kani, pinned to the version `just kani_io_proofs` asserts, from the upstream
# release bundle rather than a source build.
#
# Why the bundle and not a source build: `deps/rust-sel4/hacking/nix/scope/kani/`
# builds kani-0.67.0 from source, but it is reachable only through rust-sel4's
# own `crateUtils`/`vendorLockfile`/`assembleRustToolchain` scope, which this
# flake does not instantiate — and that expression is gated on
# `hostPlatform.isx86_64` in both shells that consume it, so it would not serve
# this repository's aarch64 hosts anyway. The bundle is the same artifact
# upstream's own `cargo kani setup` installs, identified by the sha256 GitHub
# publishes per asset, so pinning it is a pin on bytes rather than on a
# rebuildable but unpinned toolchain graph.
#
# Why `rust-bin` and not the shell's rustup toolchains: the bundle's
# `kani-compiler` is a pre-built `rustc_driver` client. It dynamically links
# `librustc_driver-<hash>.dylib`/`.so` and codegens against the matching
# sysroot, so it runs only against the *exact* nightly it was built with —
# recorded in the bundle as `rustc-version`. Substituting any other nightly is a
# dyld/ld failure, not a warning, so `rustToolchainDate` below must equal that
# recording. `assertToolchainMatch` turns a mismatch into a build-time failure
# with both versions named, since the runtime symptom is an unreadable loader
# error.
{
  lib,
  stdenvNoCC,
  fetchurl,
  makeWrapper,
  rust-bin,
  autoPatchelfHook,
  stdenv,
}:

let
  version = "0.67.0";

  # The nightly the published bundles are built against. Upstream records it in
  # the bundle itself; `assertToolchainMatch` below proves this string still
  # agrees with the bytes we fetched, so bumping `version` without bumping this
  # fails the build instead of producing a broken shell.
  rustToolchainDate = "2025-11-21";
  rustcVersion = "rustc 1.93.0-nightly (53732d5e0 2025-11-20)";

  # One prebuilt bundle per supported host, keyed by the Rust target triple
  # upstream names its assets with. Hashes are the `digest` field GitHub
  # publishes for each release asset, converted to SRI.
  bundles = {
    aarch64-darwin = {
      target = "aarch64-apple-darwin";
      hash = "sha256-f9C3ETCqN70eNG66Z1qtCem2bks4P3B/xlczDOsp7uw=";
    };
    x86_64-darwin = {
      target = "x86_64-apple-darwin";
      hash = "sha256-45TD2UDtnfT2fvU4jFYS8b9j5nFnCxk4RNa1nJJUS8k=";
    };
    aarch64-linux = {
      target = "aarch64-unknown-linux-gnu";
      hash = "sha256-l0Eo9E3UNhigbSHl/m2f9nGI3lmG/gvFe1NLDkY577k=";
    };
    x86_64-linux = {
      target = "x86_64-unknown-linux-gnu";
      hash = "sha256-O196/TtRYD7nINt7wbxP5GtaT1022q2ZOcS0xli1GsA=";
    };
  };

  inherit (stdenvNoCC.hostPlatform) system;

  bundle =
    bundles.${system} or (throw "kani ${version}: upstream publishes no release bundle for ${system}");

  # `minimal` is the whole requirement: `kani-compiler` brings its own
  # `librustc_driver` client code, and needs `rustc`, `cargo`, and the host
  # `rust-std` sysroot beside it. No clippy/rustfmt/miri.
  rustToolchain = rust-bin.nightly.${rustToolchainDate}.minimal;

in
stdenvNoCC.mkDerivation {
  pname = "kani";
  inherit version;

  src = fetchurl {
    url = "https://github.com/model-checking/kani/releases/download/kani-${version}/kani-${version}-${bundle.target}.tar.gz";
    inherit (bundle) hash;
  };

  sourceRoot = "kani-${version}";

  nativeBuildInputs = [
    makeWrapper
  ]
  # The Linux bundles are ordinary glibc ELFs built on a distro host, so their
  # interpreter and RPATH are wrong in a Nix store. This is the same problem
  # upstream's `os_hacks.rs` patchelfs at setup time, done declaratively.
  # Darwin's Mach-O binaries carry a usable `@loader_path` RPATH and need none
  # of it.
  ++ lib.optional stdenv.hostPlatform.isLinux autoPatchelfHook;

  # `kani-compiler` links `librustc_driver`, and CBMC's binaries link the C++
  # standard library. On Linux `autoPatchelfHook` resolves both from here.
  buildInputs = [
    rustToolchain
    stdenv.cc.cc.lib
  ];

  dontConfigure = true;
  dontBuild = true;

  # Both are load-bearing on a *prebuilt* bundle, not tidiness.
  #
  # `dontStrip`: `lib/` holds Rust `.rlib` archives whose crate metadata lives
  # in an object-file section. `strip` discards it, and the next run fails with
  # `E0786: found invalid metadata files for crate 'core'` -- 422 errors that
  # look like a source problem and are not. This bit us once; the symptom is
  # far from the cause, hence the note.
  #
  # `dontPatchShebangs`: `library/` and `playback/` are verification inputs
  # Kani feeds to its own compiler. Rewriting anything in them changes what is
  # verified.
  dontStrip = true;
  dontPatchShebangs = true;

  # `kani-driver` runs its own `cbmc`, `goto-cc`, `goto-instrument`, and
  # `kissat` by bare name off `PATH`; the release proxy is what normally
  # prepends the bundle's `bin/`. There is no proxy here, so the wrapper does
  # it. Without this the run dies at `Failed to invoke goto-cc`.
  #
  # `goto-cc` in turn preprocesses `kani_lib.c` by exec'ing a bare `gcc`, so a
  # C compiler must be on `PATH` under *that name*. Upstream gets one from the
  # user's ambient environment; relying on that would make the gate pass or
  # fail according to what else the developer happens to have installed, which
  # is the opposite of the point.
  #
  # The `gcc` shim below is why this is not simply `stdenv.cc`: on Darwin
  # `stdenv.cc` is a *clang* wrapper exporting `cc`/`clang` and no `gcc` at
  # all, so `goto-cc` dies with `execvp gcc failed` even with it on `PATH`.
  # Only preprocessing is being asked for, which clang does identically.
  installPhase = ''
    runHook preInstall

    # `$out/lib/kani/kani-<version>` is the layout `kani-driver` expects to be
    # invoked from: it derives every asset path from its own argv0's directory,
    # and `setup.rs` appends `kani-<version>` to `KANI_HOME`. Naming the same
    # directory in both places lets one tree serve both.
    home="$out/lib/kani/kani-${version}"
    mkdir -p "$home"
    cp -R bin lib library no_core playback "$home/"
    cp rust-toolchain-version rustc-version license-notes.txt "$home/"

    # `setup.rs` symlinks the matching toolchain here. Doing it at build time
    # is what removes the imperative `cargo kani setup` step: the toolchain is
    # a store path, so the link can never dangle or point at a drifted rustup
    # install.
    ln -s ${rustToolchain} "$home/toolchain"

    # Named `gcc` because that is the literal string `goto-cc` execs; the
    # target is whatever `stdenv.cc` provides on this platform (clang on
    # Darwin, gcc on Linux). Kept in its own directory so the wrapper can put
    # it *last* on PATH and never shadow a real toolchain.
    mkdir -p "$out/libexec/kani-cc"
    ln -s ${stdenv.cc}/bin/cc "$out/libexec/kani-cc/gcc"

    mkdir -p $out/bin
    for bin in kani cargo-kani; do
      makeWrapper "$home/bin/kani-driver" "$out/bin/$bin" \
        --argv0 "$bin" \
        --set KANI_HOME "$out/lib/kani" \
        --prefix PATH : "$home/bin:${rustToolchain}/bin:${stdenv.cc}/bin" \
        --suffix PATH : "$out/libexec/kani-cc"
    done

    runHook postInstall
  '';

  # Both checks are on the fetched bytes, so a `version` bump that silently
  # changes the required toolchain fails here rather than at first use.
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    got_toolchain="$(cat rust-toolchain-version)"
    want_toolchain="nightly-${rustToolchainDate}-${bundle.target}"
    if [ "$got_toolchain" != "$want_toolchain" ]; then
      echo "kani ${version} bundle wants toolchain '$got_toolchain'," >&2
      echo "  but nix/kani.nix pins '$want_toolchain'." >&2
      echo "  Set rustToolchainDate to the bundle's date." >&2
      exit 1
    fi

    got_rustc="$(cat rustc-version)"
    if [ "$got_rustc" != "${rustcVersion}" ]; then
      echo "kani ${version} bundle wants '$got_rustc'," >&2
      echo "  but nix/kani.nix pins '${rustcVersion}'." >&2
      exit 1
    fi

    have_rustc="$(${rustToolchain}/bin/rustc --version)"
    if [ "$have_rustc" != "$got_rustc" ]; then
      echo "kani ${version} needs '$got_rustc'," >&2
      echo "  but rust-bin.nightly.\"${rustToolchainDate}\" is '$have_rustc'." >&2
      echo "  kani-compiler links this toolchain's librustc_driver; it cannot" >&2
      echo "  run against another build." >&2
      exit 1
    fi

    # `--version` is deliberately not the check here. It passed on a build
    # whose `.rlib` metadata `strip` had destroyed, because it never compiles
    # anything. Verifying a real harness is what exercises the sysroot, the
    # toolchain symlink, and the CBMC binaries on `PATH` -- the three things
    # that can be individually broken while the wrapper still runs.
    export HOME="$TMPDIR/home"
    mkdir -p "$HOME/proof/src"
    cat > "$HOME/proof/Cargo.toml" <<'EOF'
    [package]
    name = "kani-smoke"
    version = "0.0.0"
    edition = "2021"
    [workspace]
    EOF
    cat > "$HOME/proof/src/lib.rs" <<'EOF'
    #[cfg(kani)]
    #[kani::proof]
    fn slot_index_is_modular() {
        let n: u32 = kani::any();
        kani::assume(n > 0 && n.is_power_of_two());
        let seq: u64 = kani::any();
        assert!((seq % u64::from(n)) < u64::from(n));
    }
    EOF
    (cd "$HOME/proof" && $out/bin/cargo-kani 2>&1 | tee smoke.log)
    grep -q '^VERIFICATION:- SUCCESSFUL' "$HOME/proof/smoke.log" \
      || { echo "kani ${version}: smoke harness did not verify" >&2; exit 1; }

    runHook postInstallCheck
  '';

  passthru = {
    inherit rustToolchain;
    toolchainName = "nightly-${rustToolchainDate}";
  };

  meta = {
    description = "Bit-precise model checker for Rust (upstream release bundle)";
    homepage = "https://github.com/model-checking/kani";
    license = with lib.licenses; [
      mit
      asl20
    ];
    platforms = lib.attrNames bundles;
    mainProgram = "cargo-kani";
  };
}
