# The pinned devloop distribution, as `just tasks_check` consumes it.
#
# devloop publishes no flake, so its source is a plain input and this file
# builds the two programs the repository's gates call: the Python CLI that owns
# admission, validation, evidence, and rendering, and the Rust bridge that
# decodes and formats immediate-mode Zutai data. Both come from one pinned
# revision so their helper identity and payload framing agree.
#
# `zutai-cli` deliberately does *not* come from here. The repository already
# builds it from the `deps/zutai` submodule (`scripts/lib/zutai_cli.py`), which
# is the revision every generated binding and contract check resolves against;
# `scripts/lib/devloop.py` asserts that revision equals devloop's own pin
# rather than putting a second compiler on `PATH`.
{
  lib,
  python3,
  rustPlatform,
  src,
  version,
}:
let
  # One Git fetch backs every Zutai crate devloop locks at its pinned revision,
  # so they share this digest. Nix reports the expected value on a mismatch.
  zutaiSource = "sha256-Q4nWyu6TDqbi0sRkp8sczC1dn2iCbb1I+0Nm0BJolsM=";
  bridge = rustPlatform.buildRustPackage {
    pname = "devloop-zutai";
    inherit version src;
    cargoLock = {
      lockFile = "${src}/Cargo.lock";
      # devloop depends on the Zutai crates by Git revision, so Cargo's lock
      # alone does not fix their contents for Nix.
      # Every crate devloop takes from the Zutai repository at its pinned
      # revision. One entry per locked git package, as Nix requires.
      outputHashes = {
        "zutai-eval-0.1.0" = zutaiSource;
        "zutai-hir-0.1.0" = zutaiSource;
        "zutai-im-0.1.0" = zutaiSource;
        "zutai-im-syntax-0.1.0" = zutaiSource;
        "zutai-package-0.1.0" = zutaiSource;
        "zutai-semantic-0.1.0" = zutaiSource;
        "zutai-syntax-0.1.0" = zutaiSource;
        "zutai-thir-0.1.0" = zutaiSource;
        "zutai-tlc-0.1.0" = zutaiSource;
        "zutai-types-0.1.0" = zutaiSource;
      };
    };
    # The bridge has no test binary of its own; devloop's behavior tests live
    # in Python and run against the installed pair.
    doCheck = false;
    meta = {
      description = "Immediate-mode Zutai bridge for devloop admission";
      license = lib.licenses.bsd3;
      mainProgram = "devloop-zutai";
    };
  };

  cli = python3.pkgs.buildPythonApplication {
    pname = "devloop";
    inherit version src;
    pyproject = true;
    build-system = [ python3.pkgs.setuptools ];
    dependencies = [ python3.pkgs.pyyaml ];
    # The published tests require the pinned compiler, its standard library,
    # its native runtime archive, and LLVM. Those are qualified in devloop's
    # own release, not re-derived in this closure.
    doCheck = false;
    # `devloop` resolves its contracts, helpers, and skill relative to the
    # installed package, or under `sys.prefix/share/devloop`. Neither holds for
    # a Python application in Nix, whose interpreter prefix is a different
    # store path, so the assets are installed beside the package and mirrored
    # under `share/` for skill discovery.
    postInstall = ''
      mkdir -p "$out/share/devloop"
      for asset in contracts helpers skills; do
        cp -r "$src/$asset" "$out/${python3.sitePackages}/$asset"
        cp -r "$src/$asset" "$out/share/devloop/$asset"
      done
    '';
    # Validation shells out to the bridge by name.
    makeWrapperArgs = [ "--prefix PATH : ${bridge}/bin" ];
    meta = {
      description = "UUID-bound requirements and evidence tooling for MyQue";
      license = lib.licenses.bsd3;
      mainProgram = "devloop";
    };
  };
in
cli.overrideAttrs (previous: {
  passthru = (previous.passthru or { }) // { inherit bridge; };
})
