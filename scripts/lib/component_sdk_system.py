"""CP15 system-image corpus export for the component SDK."""

from __future__ import annotations

import hashlib
import json
import shutil
from pathlib import Path


SYSTEM_NAME = "sel4-channel"
ARCHIVE_PATH = f"assets/system-{SYSTEM_NAME}.tar"
CLOSURE_PATH = f"contracts/system-image-closure/v1/closures/{SYSTEM_NAME}.zti"
TEST_RUN_PATH = f"contracts/system-test-run/v1/runs/{SYSTEM_NAME}.zti"

# A repository-shaped corpus preserves every canonical path embedded in the
# closure and every Cargo path dependency used by the builders. Build outputs
# and git metadata are excluded by the caller's copy ignore rule.
COPY_ROOTS = (
    "Cargo.lock",
    "Cargo.toml",
    "Justfile",
    "boot-contracts",
    "components",
    "contracts",
    "deps/rust-sel4",
    "deps/zutai",
    "just",
    "scripts/check",
    "scripts/build",
    "scripts/lib",
    "sel4/pins.toml",
    "slime-root",
)


def export_asset(destination: Path, source: Path, *, sdk_module) -> dict:
    """Write one deterministic system corpus and return its release-record row."""
    staging = destination.parent / f".{destination.name}-system"
    if staging.exists():
        shutil.rmtree(staging)
    staging.mkdir()
    try:
        for relative in COPY_ROOTS:
            origin = source / relative
            target = staging / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            if origin.is_dir():
                if not any(origin.iterdir()):
                    raise sdk_module.ComponentSdkError(
                        f"system-image export input is empty: {relative}; "
                        "run git submodule update in the export source"
                    )
                shutil.copytree(origin, target, ignore=sdk_module.COPY_IGNORE)
            elif origin.is_file():
                shutil.copyfile(origin, target)
            else:
                raise sdk_module.ComponentSdkError(
                    f"system-image export input is missing: {relative}"
                )
        prefix = staging / "contracts/system-image-closure/v1/inputs/sel4-prefix"
        sdk_module.canonicalize_prefix(prefix, source)
        source_needle = str(source).encode("utf-8")
        for path in sdk_module.tree_files(staging):
            if source_needle in path.read_bytes():
                raise sdk_module.ComponentSdkError(
                    f"{path.relative_to(staging)}: system corpus names its source checkout"
                )

        # Canonicalizing the prefix changes a declared closure input, so the
        # exported closure and test run are re-identified together. The source
        # closure stays untouched; the SDK record identifies this published key.
        from system_image_closure import compile_closure, compile_test_run

        closure_path = staging / CLOSURE_PATH
        closure_value = compile_closure(closure_path).value

        def rebind(reference: dict) -> None:
            artifact = staging / reference["path"]
            if reference["kind"] == "tree":
                reference["identity"] = sdk_module.tree_digest(artifact)
            else:
                reference["identity"] = hashlib.sha256(artifact.read_bytes()).hexdigest()

        rebind(closure_value["systemSpec"])
        for implementation in closure_value["implementations"]:
            rebind(implementation["artifact"])
        rebind(closure_value["target"]["prefix"])
        rebind(closure_value["root"]["implementation"])
        rebind(closure_value["loader"]["implementation"])
        for release_input in closure_value["releaseInputs"]:
            rebind(release_input["artifact"])

        # The published SDK release asset is copied verbatim, so its own
        # identity never moves; what must hold is that it still names the
        # exact canonicalized prefix this export just produced, for the same
        # profile. `canonicalize_prefix` rewrites the checkout-relative prefix
        # into the tree the release asset pins, so a mismatch here means the
        # published prefix and this corpus's prefix have diverged.
        sdk_release = staging / closure_value["target"]["sdkRelease"]["path"]
        released = json.loads(sdk_release.read_text(encoding="utf-8"))
        selected = [
            entry
            for entry in released["profiles"]
            if entry["profile"] == closure_value["target"]["profile"]
        ]
        if (
            len(selected) != 1
            or selected[0]["prefix"]["treeHash"] != closure_value["target"]["prefix"]["identity"]
        ):
            raise sdk_module.ComponentSdkError(
                "canonicalized prefix does not match the corpus SDK release asset"
            )
        closure_path.write_text(sdk_module.zti(closure_value) + "\n", encoding="utf-8")
        closure = compile_closure(closure_path)
        test_run_path = staging / TEST_RUN_PATH
        test_run_value = compile_test_run(test_run_path).value
        source_closure_identity = compile_closure(source / CLOSURE_PATH).identity.hex()
        if test_run_value["imageClosureIdentity"] != source_closure_identity:
            raise sdk_module.ComponentSdkError(
                "source test run does not reference the source closure"
            )
        test_run_value["imageClosureIdentity"] = closure.identity.hex()
        test_run_path.write_text(sdk_module.zti(test_run_value) + "\n", encoding="utf-8")
        test_run = compile_test_run(test_run_path)
        archive = destination / ARCHIVE_PATH
        archive.parent.mkdir(parents=True, exist_ok=True)
        sdk_module.canonical_tar(staging, archive)
        verify = staging.parent / f"{staging.name}-verify"
        shutil.rmtree(verify, ignore_errors=True)
        sdk_module.extract_canonical_tar(archive, verify)
        tree_hash = sdk_module.tree_digest(verify)
        shutil.rmtree(verify, ignore_errors=True)
        if tree_hash != sdk_module.tree_digest(staging):
            raise sdk_module.ComponentSdkError(
                "system corpus archive does not round-trip to its staged tree"
            )
        return {
            "name": SYSTEM_NAME,
            "archive": ARCHIVE_PATH,
            "archiveHash": hashlib.sha256(archive.read_bytes()).hexdigest(),
            "treeHash": sdk_module.tree_digest(staging),
            "closure": CLOSURE_PATH,
            "closureIdentity": closure.identity.hex(),
            "testRun": TEST_RUN_PATH,
            "testRunIdentity": test_run.identity.hex(),
        }
    finally:
        shutil.rmtree(staging, ignore_errors=True)
