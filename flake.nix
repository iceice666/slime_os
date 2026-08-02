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
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          rustToolchain = "nightly-2026-05-26";
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
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              gcc
              just
              lldb
              qemu
              rustup
              limine
              xorriso
              OVMF
              mtools
              dosfstools
              cargo-deny
              cargo-machete
              ruff
              typos
            ];

            OVMF_CODE = "${pkgs.OVMF.fd}/FV/OVMF_CODE.fd";
            OVMF_VARS = "${pkgs.OVMF.fd}/FV/OVMF_VARS.fd";
            AAVMF_CODE = "${aavmf}/FV/AAVMF_CODE.fd";
            AAVMF_VARS = "${aavmf}/FV/AAVMF_VARS.fd";

            RUSTUP_TOOLCHAIN = rustToolchain;

            shellHook = ''
              rustup toolchain install ${rustToolchain} \
                --profile minimal \
                --target ${nixpkgs.lib.concatStringsSep "," rustTargets} \
                --component clippy,rustfmt,llvm-tools-preview,rust-src,miri \
                --no-self-update
            '';
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
