"""Operator-facing gate diagnostic exam; no gate implementation is replaced."""

from __future__ import annotations

import copy
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path

import devloop
import zutai_cli
from harness import ROOT

CONTRACT = ROOT / "contracts/devloop-diagnostics/v1/schema.zt"
RECEIPT_BYTES = 8192
OUTPUT_BYTES = 65536
FIELDS = ("requirements", "helper", "code", "policy", "inputs", "target", "image", "epoch")
DISCOVERY = re.compile(r"^SLIME_DEVLOOP_DIAGNOSTIC receipt=(\S+) run=(\S+)$", re.M)
TIMEOUT = 120


class ExamError(RuntimeError):
    """An expected diagnostic or refusal was not observed."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ExamError(message)


@dataclass
class Capture:
    receipt: dict
    output: bytes
    receipt_path: Path
    output_path: Path


def safe_path(root: Path, name: str) -> Path:
    require(isinstance(name, str) and bool(name), "empty diagnostic path")
    relative = Path(name)
    require(
        not relative.is_absolute() and ".." not in relative.parts and len(name.encode()) <= 240,
        "unsafe or overlong diagnostic path",
    )
    path = root / relative
    require(not any(p.is_symlink() for p in (path, *path.parents)), "symlink diagnostic path")
    require(path.is_file(), f"missing diagnostic: {name}")
    return path


def validate(
    capture: Capture,
    identity: dict,
    target: str,
    tokens: tuple[bytes, ...],
    *,
    truncated: bool = False,
    expected_output: bytes | None = None,
    expected_exit: int = 23,
) -> None:
    r = capture.receipt
    require(
        set(r)
        == {
            "formatVersion",
            "kind",
            "runKey",
            "identity",
            "justTarget",
            "recipeExitCode",
            "outputPath",
            "sha256",
            "capturedBytes",
            "limitBytes",
            "truncated",
        },
        "receipt fields differ from contract",
    )
    require(
        r["formatVersion"] == 1 and r["kind"] == "slime-devloop-diagnostics",
        "wrong receipt version/kind",
    )
    require(
        r["identity"] == identity and set(identity) == set(FIELDS),
        "misattributed execution identity",
    )
    bound = identity | {"gate": "just-target", "justTarget": target}
    key = hashlib.sha256(json.dumps(bound, sort_keys=True).encode()).hexdigest()
    require(r["runKey"] == key and r["justTarget"] == target, "misattributed recipe/run")
    require(
        type(r["recipeExitCode"]) is int and r["recipeExitCode"] == expected_exit,
        "recipe failure exit not retained exactly",
    )
    require(
        type(r["limitBytes"]) is int and 0 < r["limitBytes"] <= OUTPUT_BYTES,
        "unbounded output limit",
    )
    require(
        type(r["capturedBytes"]) is int and r["capturedBytes"] == len(capture.output),
        "wrong captured length",
    )
    require(0 < len(capture.output) <= r["limitBytes"], "empty or oversized diagnostic")
    require(r["sha256"] == hashlib.sha256(capture.output).hexdigest(), "diagnostic hash mismatch")
    require(type(r["truncated"]) is bool and r["truncated"] is truncated, "truncation not explicit")
    require(all(token in capture.output for token in tokens), "stdout/stderr diagnostic token lost")
    if expected_output is not None:
        require(
            capture.output == expected_output[: r["limitBytes"]],
            "raw recipe output changed or dropped",
        )
        require(truncated is (len(expected_output) > r["limitBytes"]), "incorrect truncation claim")


def failed_result(result: subprocess.CompletedProcess, evidence: dict | None) -> None:
    require(
        result.returncode != 0 and "E_EVIDENCE" in result.stderr,
        f"failing acceptance was not refused: exit={result.returncode}\n{result.stdout}\n{result.stderr}",
    )
    require(evidence is not None, "recipe failure has no observation")
    observations = evidence["observations"]
    require(
        len(observations) == 1 and observations[0]["id"] == "passed",
        "malformed observation transport",
    )
    require(
        observations[0]["value"]["kind"] == "bool"
        and observations[0]["value"]["boolValue"] is False,
        "recipe failure recorded as passing",
    )


def controls() -> int:
    identity = {field: f"exam-{field}" for field in FIELDS}
    output = b"OUT-1\nERR-1\n"
    bound = identity | {"gate": "just-target", "justTarget": "fixture_failure"}
    receipt = {
        "formatVersion": 1,
        "kind": "slime-devloop-diagnostics",
        "runKey": hashlib.sha256(json.dumps(bound, sort_keys=True).encode()).hexdigest(),
        "identity": identity,
        "justTarget": "fixture_failure",
        "recipeExitCode": 23,
        "outputPath": "build/output.log",
        "sha256": hashlib.sha256(output).hexdigest(),
        "capturedBytes": len(output),
        "limitBytes": OUTPUT_BYTES,
        "truncated": False,
    }
    honest = Capture(receipt, output, Path("receipt"), Path("output"))
    validate(honest, identity, "fixture_failure", (b"OUT-1", b"ERR-1"))
    mutations = {
        "dropped-stderr": lambda c: setattr(c, "output", b"OUT-1\n"),
        "misattributed-identity": lambda c: c.receipt["identity"].update(epoch="other"),
        "misattributed-recipe": lambda c: c.receipt.update(justTarget="other"),
        "overwritten-output": lambda c: setattr(c, "output", b"OUT-2\nERR-2\n"),
        "dropped-nontoken-bytes": lambda c: setattr(c, "output", b"OUT-1ERR-1"),
        "lost-exit": lambda c: c.receipt.update(recipeExitCode=0),
        "fabricated-nonzero-exit": lambda c: c.receipt.update(recipeExitCode=999),
        "unbounded-output": lambda c: c.receipt.update(limitBytes=OUTPUT_BYTES + 1),
        "hidden-truncation": lambda c: c.receipt.update(truncated=True),
        "missing-fields": lambda c: c.receipt.pop("sha256"),
    }
    refused = 0
    for name, mutate in mutations.items():
        altered = copy.deepcopy(honest)
        mutate(altered)
        if name in ("dropped-stderr", "overwritten-output", "dropped-nontoken-bytes"):
            altered.receipt.update(
                capturedBytes=len(altered.output), sha256=hashlib.sha256(altered.output).hexdigest()
            )
        try:
            validate(
                altered, identity, "fixture_failure", (b"OUT-1", b"ERR-1"), expected_output=output
            )
        except ExamError:
            refused += 1
        else:
            raise ExamError(f"control accepted {name}")
    with tempfile.TemporaryDirectory(prefix="diagnostic-control-") as temporary:
        try:
            safe_path(Path(temporary), "missing.log")
        except ExamError:
            refused += 1
        else:
            raise ExamError("control accepted absent log")
    good = {"observations": [{"id": "passed", "value": {"kind": "bool", "boolValue": False}}]}
    for name, result, evidence in (
        ("false-passing-result", subprocess.CompletedProcess([], 0, "", ""), good),
        (
            "malformed-observation",
            subprocess.CompletedProcess([], 1, "", "E_EVIDENCE"),
            {"observations": []},
        ),
        (
            "false-passing-observation",
            subprocess.CompletedProcess([], 1, "", "E_EVIDENCE"),
            {"observations": [{"id": "passed", "value": {"kind": "bool", "boolValue": True}}]},
        ),
    ):
        try:
            failed_result(result, evidence)
        except ExamError:
            refused += 1
        else:
            raise ExamError(f"control accepted {name}")
    print(f"devloop diagnostics validator controls: {refused} corrupted results refused")
    return refused


class Fixture:
    def __init__(self, root: Path):
        self.root = root
        self.environment = devloop._environment()
        self.environment["PYTHONDONTWRITEBYTECODE"] = "1"
        # Reuse the actual pinned compiler, not a fixture compiler or CLI mock.
        # Keep its installed wrapper: it names the native LLVM tools.
        self.identity = ""
        dependencies = root / "deps"
        dependencies.mkdir()
        (dependencies / "zutai").symlink_to(zutai_cli.ZUTAI_ROOT, target_is_directory=True)
        self.setup()
        shutil.copyfile(CONTRACT, self.root / "diagnostic-schema.zt")

    def run(
        self, *args: str, check: bool = True, output_limit: int = 4 * OUTPUT_BYTES
    ) -> subprocess.CompletedProcess:
        # Disk capture bounds memory even when a broken transport is noisy.
        with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
            result = subprocess.run(
                args,
                cwd=self.root,
                env=self.environment,
                stdout=stdout,
                stderr=stderr,
                timeout=TIMEOUT,
            )
            require(
                stdout.tell() + stderr.tell() <= output_limit, "operator output exceeded exam bound"
            )
            stdout.seek(0)
            stderr.seek(0)
            result = subprocess.CompletedProcess(
                args, result.returncode, stdout.read().decode(), stderr.read().decode()
            )
        if check:
            require(
                result.returncode == 0,
                f"fixture command failed: {args}\n{result.stdout}\n{result.stderr}",
            )
        return result

    def setup(self) -> None:
        # Preserve the real adapter's module closure, including future generated
        # receipt bindings. Do not copy bytecode or local build output.
        sources = [
            ROOT / "scripts/check/devloop-gate.py",
            *sorted((ROOT / "scripts/lib").glob("*.py")),
        ]
        for source in sources:
            require(not source.is_symlink(), f"symlink fixture source: {source}")
            destination = self.root / source.relative_to(ROOT)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        shutil.copytree(
            ROOT / "contracts/devloop-diagnostics", self.root / "contracts/devloop-diagnostics"
        )
        # Source policy belongs to devloop; the fixture narrows only authority.
        policy = json.loads((ROOT / ".devloop/policy.json").read_text())
        policy["codePaths"] = ["scripts", "Justfile", "fixture.py"]
        policy["approvalArgv"] = ["python3", "approval.py"]
        policy["gates"] = [g for g in policy["gates"] if g["id"] == "just-target"]
        (self.root / ".devloop").mkdir()
        (self.root / ".devloop/policy.json").write_text(json.dumps(policy))
        (self.root / "approval.py").write_text("print('{\"approved\": true}')\n")
        (self.root / "Justfile").write_text(
            '[positional-arguments]\ndevloop *ARGS:\n    python3 scripts/lib/devloop.py "$@"\n\nfixture_failure:\n    python3 fixture.py fail\n\nfixture_oversize:\n    python3 fixture.py large\n'
        )
        (self.root / "fixture.py").write_text("""import pathlib, sys
p = pathlib.Path("counter")
n = int(p.read_text()) + 1 if p.exists() else 1
p.write_text(str(n))
print("STDOUT-DIAGNOSTIC-%d" % n, flush=True)
print("STDERR-DIAGNOSTIC-%d" % n, file=sys.stderr, flush=True)
if sys.argv[1] == "large":
    sys.stderr.write("X" * 1048576)
    sys.stderr.flush()
sys.exit(23)
""")
        # Borrow a valid admitted body's canonical spec; retain no identity,
        # consumer metadata, or evidence from the real work-item store.
        item = ROOT / ".tasks/items/01a11bda-a60d-7234-a3b9-34f11911cf37.md"
        payload = item.read_text().split("```zti\n", 1)[1].split("```", 1)[0]
        (self.root / "spec.zti").write_text(payload)
        self.run("myque", "init")
        admitted = self.run(
            "just",
            "devloop",
            "admit",
            "spec.zti",
            "--title",
            "Diagnostic exam fixture",
            "--kind",
            "bug",
            "--admission",
            "diagnostic-exam-fixture",
            "--policy",
            ".devloop/policy.json",
        )
        self.identity = json.loads(admitted.stdout)["id"]
        self.run("just", "devloop", "start", self.identity, "--policy", ".devloop/policy.json")
        (self.root / "inputs.json").write_text('{"justTarget": "fixture_failure"}')

    def evidence(self) -> list[dict]:
        # MyQue owns the envelope. Its consumer record is decoded by the
        # installed devloop parser, rather than a second envelope parser.
        item = json.loads(self.run("myque", "api", "get", self.identity).stdout)
        code = "import json,sys; from devloop.core import record; print(json.dumps(record(json.load(sys.stdin))))"
        result = subprocess.run(
            ["python3", "-c", code],
            cwd=self.root,
            env=self.environment,
            input=json.dumps(item),
            text=True,
            capture_output=True,
            timeout=TIMEOUT,
        )
        require(result.returncode == 0, f"cannot read fixture evidence: {result.stderr}")
        return json.loads(result.stdout)["evidence"]

    def reference(self, target: str, number: int) -> tuple[int, bytes]:
        counter = self.root / "counter"
        previous = counter.read_text() if counter.exists() else None
        counter.write_text(str(number - 1))
        result = self.run("just", target, check=False, output_limit=2 * 1048576)
        if previous is None:
            counter.unlink()
        else:
            counter.write_text(previous)
        return result.returncode, (result.stdout + result.stderr).encode()

    def gate(self, image: str) -> tuple[subprocess.CompletedProcess, dict | None]:
        result = self.run(
            "just",
            "devloop",
            "gate",
            self.identity,
            "qualification-passes",
            "--policy",
            ".devloop/policy.json",
            "--inputs",
            "inputs.json",
            "--target",
            "diagnostic-exam",
            "--image",
            image,
            check=False,
        )
        entries = self.evidence()
        return result, entries[-1] if entries else None

    def discover(
        self,
        result: subprocess.CompletedProcess,
        evidence: dict,
        target: str,
        tokens: tuple[bytes, ...],
        *,
        truncated: bool = False,
        expected_output: bytes | None = None,
        expected_exit: int = 23,
    ) -> Capture:
        matches = DISCOVERY.findall(result.stdout + "\n" + result.stderr)
        require(len(matches) == 1, "operator CLI did not expose exactly one diagnostic receipt")
        name, key = matches[0]
        path = safe_path(self.root, name)
        require(path.stat().st_size <= RECEIPT_BYTES, "oversized diagnostic receipt")
        # Receipt uses the newly declared Zutai immediate format. Parsing and
        # field derivation come from that schema, not handwritten offsets.
        checker = self.root / "decode-receipt.zt"
        checker.write_text(
            f's ::= import "diagnostic-schema.zt";\nmain :: Load -> Validation DecodeIssue s.DiagnosticReceipt ! {{ load.zti : Path -> Data; }}\n  = load => s.decodeReceipt (loadZti {json.dumps(str(path))});\nmain\n'
        )
        decoded = json.loads(self.run(str(zutai_cli.binary()), "json", str(checker)).stdout)
        require(
            isinstance(decoded, dict) and decoded.get("tag") == "valid",
            f"receipt does not decode through schema: {decoded}",
        )
        # The compiler's first-order JSON projection supplies the typed record.
        receipt = decoded["payload"]["value"]
        output_path = safe_path(self.root, receipt["outputPath"])
        require(
            output_path.stat().st_size <= OUTPUT_BYTES, "diagnostic capture exceeds contract bound"
        )
        capture = Capture(receipt, output_path.read_bytes(), path, output_path)
        require(key == receipt["runKey"], "discovery run differs from receipt")
        validate(
            capture,
            evidence["identity"],
            target,
            tokens,
            truncated=truncated,
            expected_output=expected_output,
            expected_exit=expected_exit,
        )
        return capture


def transport_controls() -> None:
    """Corrupt real adapter stdout only in fixture-only negative controls."""
    with tempfile.TemporaryDirectory(prefix="diagnostic-transport-controls-") as temporary:
        fixture = Fixture(Path(temporary))
        wrapper = fixture.root / "transport.py"
        wrapper.write_text("""import json, subprocess, sys
result = subprocess.run(["python3", "scripts/check/devloop-gate.py", "just-target"], input=sys.stdin.read(), text=True, capture_output=True)
sys.stderr.write(result.stderr)
if sys.argv[1] == "malformed":
    print("not observation JSON")
else:
    values = json.loads(result.stdout)
    values[0]["value"]["boolValue"] = True
    print(json.dumps(values))
""")
        policy_path = fixture.root / ".devloop/policy.json"
        policy = json.loads(policy_path.read_text())
        refused = 0
        for mode in ("malformed", "passing"):
            policy["gates"][0]["argv"] = ["python3", "transport.py", mode]
            policy_path.write_text(json.dumps(policy))
            before = (
                int((fixture.root / "counter").read_text())
                if (fixture.root / "counter").exists()
                else 0
            )
            result, evidence = fixture.gate(mode)
            require(
                int((fixture.root / "counter").read_text()) == before + 1,
                "transport control never executed real recipe",
            )
            if mode == "malformed":
                require(
                    result.returncode != 0
                    and "E_EVIDENCE" not in result.stderr
                    and ("E_INPUT" in result.stderr or "Expecting value" in result.stderr),
                    "actual CLI did not refuse malformed observation transport",
                )
            else:
                require(
                    result.returncode == 0
                    and evidence is not None
                    and evidence["observations"][0]["value"]["boolValue"] is True,
                    "passing transport mutation did not reach actual CLI",
                )
                try:
                    failed_result(result, evidence)
                except ExamError:
                    pass
                else:
                    raise ExamError("judge accepted mutated true observation through actual CLI")
            refused += 1
        print(f"devloop diagnostics transport controls: {refused} actual CLI mutations judged")


def receipt_decode_control() -> None:
    """Exercise the actual typed decoder even while qualification is red."""
    with tempfile.TemporaryDirectory(prefix="diagnostic-receipt-control-") as temporary:
        root = Path(temporary)
        shutil.copyfile(CONTRACT, root / "diagnostic-schema.zt")
        source = root / "receipt.zti"
        source.write_text(
            '{formatVersion=1;kind="slime-devloop-diagnostics";runKey="k";identity={requirements="r";helper="h";code="c";policy="p";inputs="i";target="t";image="m";epoch="e";};justTarget="fixture";recipeExitCode=1;outputPath="build/out";sha256="s";capturedBytes=12;limitBytes=65536;truncated=false;}'
        )
        checker = root / "decode.zt"
        checker.write_text(
            's ::= import "diagnostic-schema.zt"; main :: Load -> Validation DecodeIssue s.DiagnosticReceipt ! { load.zti : Path -> Data; } = load => s.decodeReceipt (loadZti "receipt.zti"); main'
        )
        for valid in (True, False):
            if not valid:
                source.write_text(
                    source.read_text().replace("recipeExitCode=1", 'recipeExitCode="wrong"')
                )
            environment = devloop._environment()
            environment["ZUTAI_STDLIB_ROOT"] = str(zutai_cli.STDLIB)
            result = subprocess.run(
                [str(zutai_cli.binary()), "json", str(checker)],
                cwd=root,
                env=environment,
                capture_output=True,
                text=True,
                timeout=TIMEOUT,
            )
            require(result.returncode == 0, f"typed receipt decoder did not run: {result.stderr}")
            decoded = json.loads(result.stdout)
            require(
                decoded.get("tag") == ("valid" if valid else "invalid"),
                "typed receipt decoder accepted wrong field kind",
            )
            if valid:
                require(
                    decoded["payload"]["value"]["recipeExitCode"] == 1,
                    "typed receipt JSON projection changed",
                )
        print(
            "devloop diagnostics receipt controls: valid receipt decoded; wrong field kind refused"
        )


def qualification() -> None:
    findings = devloop.check_toolchain()
    require(not findings, "; ".join(findings))
    with tempfile.TemporaryDirectory(prefix="devloop-diagnostics-exam-") as temporary:
        fixture = Fixture(Path(temporary))
        exit_code, raw = fixture.reference("fixture_failure", 1)
        result, evidence = fixture.gate("first")
        failed_result(result, evidence)
        first = fixture.discover(
            result,
            evidence,
            "fixture_failure",
            (b"STDOUT-DIAGNOSTIC-1", b"STDERR-DIAGNOSTIC-1"),
            expected_output=raw,
            expected_exit=exit_code,
        )
        original = (first.receipt_path.read_bytes(), first.output_path.read_bytes())
        result, evidence = fixture.gate("first")
        failed_result(result, evidence)
        again = fixture.discover(
            result,
            evidence,
            "fixture_failure",
            (b"STDOUT-DIAGNOSTIC-1", b"STDERR-DIAGNOSTIC-1"),
            expected_output=raw,
            expected_exit=exit_code,
        )
        require((fixture.root / "counter").read_text() == "1", "cached failure reran the recipe")
        require(
            again.receipt_path == first.receipt_path and again.output == first.output,
            "cached failure lost original diagnostics",
        )
        exit_code, raw = fixture.reference("fixture_failure", 2)
        result, evidence = fixture.gate("second")
        failed_result(result, evidence)
        second = fixture.discover(
            result,
            evidence,
            "fixture_failure",
            (b"STDOUT-DIAGNOSTIC-2", b"STDERR-DIAGNOSTIC-2"),
            expected_output=raw,
            expected_exit=exit_code,
        )
        require(
            second.receipt_path != first.receipt_path and second.output_path != first.output_path,
            "new run overwrote old diagnostic paths",
        )
        require(
            (first.receipt_path.read_bytes(), first.output_path.read_bytes()) == original,
            "new run overwrote prior failure bytes",
        )
        (fixture.root / "inputs.json").write_text('{"justTarget": "fixture_oversize"}')
        exit_code, raw = fixture.reference("fixture_oversize", 3)
        result, evidence = fixture.gate("oversize")
        failed_result(result, evidence)
        fixture.discover(
            result,
            evidence,
            "fixture_oversize",
            (b"STDOUT-DIAGNOSTIC-3", b"STDERR-DIAGNOSTIC-3"),
            truncated=True,
            expected_output=raw,
            expected_exit=exit_code,
        )
        before = int((fixture.root / "counter").read_text())
        for bad in ('{"justTarget": "unknown"}', "{}", "[]", "{", '{"justTarget": 1}'):
            (fixture.root / "inputs.json").write_text(bad)
            previous_entries = fixture.evidence()
            result, evidence = fixture.gate("malformed")
            require(
                result.returncode != 0 and not DISCOVERY.search(result.stdout + result.stderr),
                "malformed target accepted or claimed a capture",
            )
            entries = fixture.evidence()
            require(
                len(entries) in (len(previous_entries), len(previous_entries) + 1)
                and entries[: len(previous_entries)] == previous_entries,
                "malformed request mutated historical evidence",
            )
            if len(entries) > len(previous_entries):
                require(
                    entries[-1]["passed"] is False and entries[-1]["observations"] == [],
                    "request refusal disguised as recipe evidence",
                )
            require(
                int((fixture.root / "counter").read_text()) == before,
                "malformed target executed a recipe",
            )
        # A regular file instead of build defeats writes regardless of uid.
        shutil.rmtree(fixture.root / "build")
        (fixture.root / "build").write_text("capture cannot create directories here")
        (fixture.root / "inputs.json").write_text('{"justTarget": "fixture_failure"}')
        previous_entries = fixture.evidence()
        result, evidence = fixture.gate("storage-failure")
        require(
            result.returncode != 0 and not DISCOVERY.search(result.stdout + result.stderr),
            "capture failure accepted or claimed complete receipt",
        )
        entries = fixture.evidence()
        require(
            len(entries) in (len(previous_entries), len(previous_entries) + 1)
            and entries[: len(previous_entries)] == previous_entries,
            "capture failure mutated historical evidence",
        )
        if len(entries) > len(previous_entries):
            require(
                entries[-1]["identity"]["image"] == "storage-failure"
                and entries[-1]["passed"] is False
                and entries[-1]["observations"] == [],
                "capture error reported as executed recipe evidence",
            )
        require(
            re.search(
                r"diagnostic.*(fail|refus|unavailable|error)", result.stdout + result.stderr, re.I
            )
            is not None,
            "capture failure was silent",
        )
        print(
            "devloop diagnostics exam: failure, cache, identity, bounds, malformed inputs and capture failure qualified"
        )


def main(*, controls_only: bool = False) -> int:
    controls()
    receipt_decode_control()
    transport_controls()
    if not controls_only:
        qualification()
    return 0
