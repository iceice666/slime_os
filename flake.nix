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
            ];

            OVMF_CODE = "${pkgs.OVMF.fd}/FV/OVMF_CODE.fd";
            OVMF_VARS = "${pkgs.OVMF.fd}/FV/OVMF_VARS.fd";

            RUSTUP_TOOLCHAIN = rustToolchain;

            shellHook = ''
              rustup toolchain install ${rustToolchain} \
                --profile minimal \
                --target x86_64-unknown-none,x86_64-unknown-uefi \
                --component clippy,rustfmt,llvm-tools-preview,rust-src \
                --no-self-update
            '';
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-rfc-style);
    };
}
