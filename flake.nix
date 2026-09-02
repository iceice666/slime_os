{
  description = "SlimeOS Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Kani's release bundle ships pre-built binaries linked against one exact
    # nightly (`rustc-version` in the bundle), which no nixpkgs channel
    # carries. `rust-bin.nightly."<date>"` is the only pinned source of that
    # build, so the proof gate's toolchain comes from here rather than from the
    # imperative `rustup` installs the shell hook does for the product
    # toolchains.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          # Kani is a separate shell, not a `default` package: the bundle is
          # ~325 MB and only one gate uses it, so every other `nix develop`
          # (and the five CI jobs that never run a proof) must not pay for it.
          kani = pkgs.callPackage ./nix/kani.nix { };
          # Workspace host crates use this toolchain. The seL4 root, child,
          # and loader use the independent pin in `sel4/pins.toml`.
          rustToolchain = "nightly-2026-05-26";
          sel4Pins = builtins.fromTOML (builtins.readFile ./sel4/pins.toml);
          sel4RustToolchain = sel4Pins.rust_sel4.toolchain;
          # Every target any gate builds for. `RUSTUP_TOOLCHAIN` below pins the
          # toolchain by name, which makes rustup ignore the `targets` list in
          # `rust-toolchain.toml` — so this list, not that one, is what a fresh
          # `nix develop` actually installs. They must agree.
          rustTargets = [
            "x86_64-unknown-none"
            "aarch64-unknown-none"
          ];
          # The exact GNU AArch64 cross toolchain the pinned seL4 kernel and
          # kernel loader are built with (`CROSS_COMPILER_PREFIX`, `CC`).
          #
          # `pkgsCross.aarch64-multiplatform.stdenv.cc` is not the same *shape*
          # on every system: on `aarch64-darwin` and `x86_64-linux` it is a
          # cross `gcc-wrapper` whose `targetPrefix` is
          # `aarch64-unknown-linux-gnu-`, and on `aarch64-linux` it is a
          # *native* `gcc-wrapper` whose `targetPrefix` is empty. Only the
          # derivation differs; the wrappers inject the same flags.
          crossCC = pkgs.pkgsCross.aarch64-multiplatform.stdenv.cc;
          # P3's upstream seL4 reference kernel and loader use the matching GNU
          # RISC-V toolchain, pinned by an absolute wrapper path for the same
          # reproducibility reason as AArch64.
          riscvCrossCC = pkgs.pkgsCross.riscv64.stdenv.cc;
          # P6.1's pc99 kernel. `pkgsCross.gnu64` is the x86-64 GNU/Linux
          # toolchain: on `x86_64-linux` it resolves to the native wrapper and
          # on the AArch64 hosts to a cross wrapper, which is the same shape
          # difference `crossCC` documents above and is equally harmless
          # because both inject the same flags. It is named here so
          # `X86_64_COMPILER_PREFIX` is one exact store path rather than
          # whatever `gcc` the shell's `PATH` happens to resolve.
          x86CC = pkgs.pkgsCross.gnu64.stdenv.cc;
          # The seL4 build drives host Python generators (bitfield, invocation,
          # hardware/DTS) through a bare `python3`.
          sel4Python = pkgs.python3.withPackages (ps: [
            ps.jinja2
            ps.pyyaml
            ps.lxml
            ps.ply
            ps.pyfdt
            ps.jsonschema
            ps.setuptools
          ]);
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                gcc
                llvmPackages.clang
                llvmPackages.lld
                just
                lldb
                qemu
                rustup
              ]
              ++ [
                cargo-deny
                cargo-machete
                ruff
                typos
                # seL4 product build: kernel configure/build, cross compilation,
                # bindgen, device-tree handling, and the `xmllint` the kernel's
                # syscall/invocation header generators validate their XML with.
                cmake
                ninja
                dtc
                libxml2.bin
                crossCC
                riscvCrossCC
                x86CC
                sel4Python
              ];

            # `sel4-sys` generates the libsel4 bindings with bindgen, which
            # resolves libclang at run time rather than at link time.
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
            # The prefix `scripts/build/build-sel4.py` passes to CMake and uses
            # for `CC`; it must name the toolchain the pinned prefix was built
            # with, since the loader links against those objects.
            #
            # Absolute, not a bare triple prefix. A bare prefix is resolved
            # against `PATH`, and the entry that wins is not the same
            # derivation on every system: `aarch64-linux` puts the *native*
            # wrapper first — which exports no `aarch64-unknown-linux-gnu-gcc`
            # at all — so the prefixed name falls through to the **unwrapped**
            # GCC, while Darwin resolves the cross wrapper. Different binaries,
            # so different injected flags and a different `as`. Naming the
            # wrapper's `bin/` by store path makes every host run the same
            # compiler driver and the same assembler (B21).
            CROSS_COMPILER_PREFIX = "${crossCC}/bin/${crossCC.targetPrefix}";
            RISCV64_CROSS_COMPILER_PREFIX = "${riscvCrossCC}/bin/${riscvCrossCC.targetPrefix}";
            # P6.1's pc99 kernel. Unlike AArch64 and RISC-V this is not a cross
            # toolchain — an x86-64 seL4 kernel is built by an ordinary
            # ELF-targeting x86-64 GCC — but it is exported by absolute store
            # path for exactly the same reason: `[observed_prefix_qemu_pc99]`
            # must bind one compiler and assembler rather than whichever `gcc`
            # the ambient `PATH` resolves first.
            X86_64_COMPILER_PREFIX = "${x86CC}/bin/${x86CC.targetPrefix}";

            # Freestanding C components use Clang's target driver and LLD.
            # Do not inherit mkShell's ambient CC: on Linux it is GCC, which
            # rejects `--target=aarch64-none-elf` before compiling anything.
            SLIME_COMPONENT_CC = "${pkgs.llvmPackages.clang}/bin/clang";

            RUSTUP_TOOLCHAIN = rustToolchain;

            shellHook = ''
              rustup toolchain install ${rustToolchain} \
                --profile minimal \
                --target ${nixpkgs.lib.concatStringsSep "," rustTargets} \
                --component clippy,rustfmt,llvm-tools-preview,rust-src,miri \
                --no-self-update
              # The seL4 artifacts build with the rust-sel4 pin from
              # sel4/pins.toml, not the legacy toolchain above. `-Z build-std`
              # needs rust-src; the scripts select it per invocation via
              # RUSTUP_TOOLCHAIN, so it is installed but not made default.
              rustup toolchain install ${sel4RustToolchain} \
                --profile minimal \
                --component rust-src,rustfmt,clippy \
                --no-self-update
            '';
          };

          # `nix develop .#kani --command just kani_io_proofs`. Deliberately
          # minimal: the proof gate needs `just`, `cargo-kani`, and the
          # bundle's own toolchain, and nothing it verifies touches libsel4,
          # QEMU, or the cross compiler. `RUSTUP_TOOLCHAIN` is left unset on
          # purpose — `kani-driver` selects the toolchain its bundle links
          # against, and an inherited value from the default shell would send
          # it at the wrong nightly.
          kani = pkgs.mkShell {
            packages = [
              pkgs.just
              kani
            ];
          };
        }
      );

      # Exposed so CI can realize and cache the bundle as its own step, and so
      # `nix build .#kani` reports the pin's integrity without entering a shell.
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
        in
        {
          kani = pkgs.callPackage ./nix/kani.nix { };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
