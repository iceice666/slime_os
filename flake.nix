{
  description = "SlimeOS Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
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
          pkgs = nixpkgs.legacyPackages.${system};
          # The retained legacy custom-kernel gates build with this toolchain.
          # It is not the seL4 toolchain: `deps/rust-sel4` pins its own, and
          # `sel4/pins.toml` is the single source of truth for that pin.
          rustToolchain = "nightly-2026-05-26";
          sel4Pins = builtins.fromTOML (builtins.readFile ./sel4/pins.toml);
          sel4RustToolchain = sel4Pins.rust_sel4.toolchain;
          # Every target any gate builds for. `RUSTUP_TOOLCHAIN` below pins the
          # toolchain by name, which makes rustup ignore the `targets` list in
          # `rust-toolchain.toml` — so this list, not that one, is what a fresh
          # `nix develop` actually installs. They must agree.
          rustTargets = [
            "x86_64-unknown-none"
            "x86_64-unknown-uefi"
            "aarch64-unknown-none"
            "aarch64-unknown-uefi"
          ];
          # AArch64 UEFI firmware for `qemu-system-aarch64 -machine virt`.
          # `pkgs.OVMF` is built for the host, so the AArch64 build has to be
          # named explicitly even when the host is already AArch64.
          aavmf = pkgs.pkgsCross.aarch64-multiplatform.OVMF.fd;
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
                just
                lldb
                qemu
                rustup
              ]
              # nixpkgs' `limine` package (the UEFI bootloader installer CLI,
              # used only by the legacy x86_64 custom-kernel ISO build) is
              # marked badPlatforms on Darwin. The seL4/AArch64 path this
              # shell otherwise supports never invokes it, so it is dropped
              # there rather than blocking the whole shell. Dropping it changes
              # this shell's derivation hash, and therefore the `-frandom-seed`
              # nixpkgs derives from it, but that no longer reaches the seL4
              # kernel: `scripts/build/build-sel4.py` builds it with the shell's
              # compiler flags removed and a fixed seed of its own (B19). Nor
              # does the per-platform difference in what `crossCC` resolves to,
              # since that build states its own frame-pointer policy (B20) —
              # `aarch64-darwin` and `aarch64-linux` produce a byte-identical
              # `kernel.elf`.
              ++ lib.optional (!stdenv.isDarwin) limine
              ++ [
                xorriso
                OVMF
                mtools
                dosfstools
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
                sel4Python
              ];

            OVMF_CODE = "${pkgs.OVMF.fd}/FV/OVMF_CODE.fd";
            OVMF_VARS = "${pkgs.OVMF.fd}/FV/OVMF_VARS.fd";
            AAVMF_CODE = "${aavmf}/FV/AAVMF_CODE.fd";
            AAVMF_VARS = "${aavmf}/FV/AAVMF_VARS.fd";

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
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
