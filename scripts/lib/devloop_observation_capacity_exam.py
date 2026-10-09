"""Real-CLI observation-capacity qualification; never implements a gate."""

from __future__ import annotations

import copy
import hashlib
import json
import re
import shutil
import subprocess
import tempfile
import time
from pathlib import Path

import zutai_cli
from devloop_diagnostics_exam import (
    Fixture,
    ExamError,
    require,
    safe_path,
    Capture,
    validate as validate_diagnostic,
)
from harness import ROOT
import devloop_capacity_audit as audit

LIMIT = 65536
MEMORY_DELTA = 32 * 1024 * 1024
DISCOVERY = re.compile(r"^SLIME_DEVLOOP_CAPACITY receipt=(\S+) run=(\S+)$", re.M)
DIAGNOSTIC = re.compile(r"^SLIME_DEVLOOP_DIAGNOSTIC receipt=(\S+) run=(\S+)$", re.M)
SCHEMA = ROOT / "contracts/observation-capacity-refusal/v1/schema.zt"
FIXTURE_RECIPE = "fixture_capacity"
FIELDS = ("requirements", "helper", "code", "policy", "inputs", "target", "image", "epoch")


def normalize(data: bytes) -> bytes:
    return (
        data.decode("utf-8", "surrogateescape")
        .replace("\r\n", "\n")
        .replace("\r", "\n")
        .encode("utf-8", "surrogateescape")
    )


def digest_for(stdout_bytes: int, stderr: bytes) -> str:
    digest = hashlib.sha256()
    chunk = b"O" * 8192
    while stdout_bytes:
        part = chunk[: min(len(chunk), stdout_bytes)]
        digest.update(part)
        stdout_bytes -= len(part)
    digest.update(normalize(stderr))
    return digest.hexdigest()


def validate_refusal(
    record: dict,
    identity: dict,
    key: str,
    observed: int,
    diagnostic_path: str,
    diagnostic_digest: str,
    expected_exit: int = 0,
) -> None:
    expected = {
        "formatVersion",
        "kind",
        "runKey",
        "gate",
        "justTarget",
        "identity",
        "outcome",
        "stderrLimitBytes",
        "stderrObservedBytes",
        "recipeExitCode",
        "diagnosticReceiptPath",
        "diagnosticReceiptSha256",
        "finishedAt",
        "reuseUntil",
    }
    require(set(record) == expected, "capacity receipt fields differ from contract")
    require(
        record["formatVersion"] == 1 and record["kind"] == "slime-observation-capacity-refusal",
        "wrong capacity schema",
    )
    require(
        record["outcome"] == "stderr-capacity" and record["gate"] == "just-observations",
        "wrong capacity classification/gate",
    )
    require(
        record["identity"] == identity
        and record["runKey"] == key
        and record["justTarget"] == "fixture_capacity",
        "capacity identity/recipe mismatch",
    )
    require(
        record["stderrLimitBytes"] == LIMIT
        and record["stderrObservedBytes"] == observed
        and observed > LIMIT,
        "capacity raw-byte count mismatch",
    )
    require(record["recipeExitCode"] == expected_exit, "capacity recipe exit mismatch")
    require(
        record["diagnosticReceiptPath"] == diagnostic_path
        and record["diagnosticReceiptSha256"] == diagnostic_digest,
        "capacity diagnostic binding mismatch",
    )
    require(
        type(record["finishedAt"]) is int and record["reuseUntil"] == record["finishedAt"] + 3600,
        "capacity lifetime differs from existing window",
    )


def controls() -> None:
    identity = {field: field for field in FIELDS}
    base = {
        "formatVersion": 1,
        "kind": "slime-observation-capacity-refusal",
        "runKey": "k",
        "gate": "just-observations",
        "justTarget": "fixture_capacity",
        "identity": identity,
        "outcome": "stderr-capacity",
        "stderrLimitBytes": LIMIT,
        "stderrObservedBytes": LIMIT + 1,
        "recipeExitCode": 0,
        "diagnosticReceiptPath": "d",
        "diagnosticReceiptSha256": "h",
        "finishedAt": 100,
        "reuseUntil": 3700,
    }
    validate_refusal(base, identity, "k", LIMIT + 1, "d", "h")
    changes = {
        "classification": ("outcome", "recipe-failure"),
        "gate": ("gate", "just-target"),
        "raw-count": ("stderrObservedBytes", LIMIT),
        "bound": ("stderrLimitBytes", LIMIT + 1),
        "lifetime": ("reuseUntil", 999999),
        "binding": ("diagnosticReceiptSha256", "other"),
        "key": ("runKey", "other"),
        "fabricated-exit": ("recipeExitCode", 999),
        "future-time": ("finishedAt", 999999999999),
    }
    for name, (field, value) in changes.items():
        record = copy.deepcopy(base)
        record[field] = value
        try:
            validate_refusal(record, identity, "k", LIMIT + 1, "d", "h")
        except ExamError:
            pass
        else:
            raise ExamError(f"capacity judge accepted {name}")
    print(f"observation capacity receipt controls: {len(changes)} corrupt results refused")
    output = b"O\nE\n"
    diagnostic_identity = {field: field for field in FIELDS}
    run_key = hashlib.sha256(
        json.dumps(
            diagnostic_identity | {"gate": "just-observations", "justTarget": "fixture_capacity"},
            sort_keys=True,
        ).encode()
    ).hexdigest()
    diagnostic_record = {
        "formatVersion": 1,
        "kind": "slime-devloop-diagnostics",
        "runKey": run_key,
        "identity": diagnostic_identity,
        "justTarget": "fixture_capacity",
        "recipeExitCode": 0,
        "outputPath": "output.log",
        "sha256": hashlib.sha256(output).hexdigest(),
        "capturedBytes": len(output),
        "limitBytes": LIMIT,
        "truncated": False,
    }
    validate_diagnostic(
        Capture(diagnostic_record, output, Path("r"), Path("o")),
        diagnostic_identity,
        "fixture_capacity",
        (),
        expected_exit=0,
        gate="just-observations",
    )
    print("capacity diagnostic control: honest observation-gate key accepted")
    audit.check_controls()


class CapacityFixture(Fixture):
    def setup(self) -> None:
        super().setup()
        shutil.copytree(
            ROOT / "contracts/observation-capacity-refusal",
            self.root / "contracts/observation-capacity-refusal",
        )
        policy_path = self.root / ".devloop/policy.json"
        policy = json.loads(policy_path.read_text())
        real = json.loads((ROOT / ".devloop/policy.json").read_text())
        policy["gates"] += [g for g in real["gates"] if g["id"] == "just-observations"]
        policy_path.write_text(json.dumps(policy))
        with (self.root / "Justfile").open("a") as justfile:
            justfile.write("\nfixture_capacity:\n    @python3 fixture.py\n")
        (self.root / "fixture.py").write_text("""import json, os, pathlib, sys, time
sys.path.insert(0, "scripts/lib")
import devloop_observations
p=pathlib.Path("counter"); n=int(p.read_text())+1 if p.exists() else 1; p.write_text(str(n))
s=json.loads(pathlib.Path("settings.json").read_text())
def emit(fd,count,pattern=b"E"):
    chunk=(pattern*((8192+len(pattern)-1)//len(pattern)))[:8192]
    while count:
        data=chunk[:min(len(chunk),count)]
        if s.get("slow"):time.sleep(0.0005)
        written=os.write(fd,data);count-=written
if s.get("alternate"):
    out=s.get("stdout",0);err=s["stderr"]
    while out or err:
        part=min(out,4096);emit(1,part,b"O");out-=part
        part=min(err,4096);emit(2,part,bytes.fromhex(s.get("pattern","45")));err-=part
elif s.get("split"):
    for fd,data in ((1,b"A\\r\\nB\\r"+bytes.fromhex("e99baa")+b"\\xff"),(2,b"X\\r\\n"+bytes.fromhex("c3a9")+b"\\xfe\\r")):
        for byte in data:os.write(fd,bytes([byte]));time.sleep(0.001)
elif s.get("mixed"):
    emit(2,65537);emit(1,s["stdout"],b"O");emit(2,s["stderr"]-65537)
else:
    emit(1,s.get("stdout",0),b"O")
    emit(2,s["stderr"],bytes.fromhex(s.get("pattern","45")))
devloop_observations.record(casesObserved=1)
sys.exit(s.get("exit",0))
""")
        source = self.root / "observation.zti"
        payload = (
            (self.root / "spec.zti")
            .read_text()
            .replace('gate = "just-target"', 'gate = "just-observations"')
        )
        payload = payload.replace(
            "  ];\n  nonGoals = [",
            """    {
      gate = "just-target";
      id = "target-passes";
      mandatory = false;
      mode = "automated";
      predicate = "recipe-passed";
      rationale = "Cross-gate capacity isolation fixture";
    };
  ];
  nonGoals = [""",
            1,
        )
        source.write_text(payload)
        admitted = self.run(
            "just",
            "devloop",
            "admit",
            "observation.zti",
            "--title",
            "Observation capacity fixture",
            "--kind",
            "bug",
            "--admission",
            "observation-capacity-fixture",
            "--policy",
            ".devloop/policy.json",
        )
        self.identity = json.loads(admitted.stdout)["id"]
        self.run("just", "devloop", "start", self.identity, "--policy", ".devloop/policy.json")
        (self.root / "inputs.json").write_text('{"justTarget":"fixture_capacity"}')
        self.audit_runs = []
        self.invocation_bounds = {}
        self.snapshots = {}
        self.current_settings = {}
        self.previous_evidence = []
        self.expected_gate = "just-observations"

    def allowed(self, path: Path) -> int | None:
        if path == Path("/dev/tty"):
            return 0
        try:
            relative = path.relative_to(self.root)
        except ValueError:
            parts = path.parts
            if (
                len(parts) >= 3
                and parts[-2].startswith("slime-devloop-relay-")
                and parts[-1] == "messages"
            ):
                return 16384
            return None
        text = relative.as_posix()
        if text == ".":
            return 0
        if text == "counter":
            return 32
        if text == "build/zutai-cache":
            return 0
        if text.startswith("build/devloop-diagnostics/"):
            parts = relative.parts
            if len(parts) == 3 and re.fullmatch(r"[0-9a-f]{64}-[A-Za-z0-9_-]+", parts[2]):
                return 0
            if len(parts) == 4 and re.fullmatch(r"[0-9a-f]{64}-[A-Za-z0-9_-]+", parts[2]):
                if path.name == "output.log":
                    return LIMIT
                if path.name == "receipt.zti":
                    return 8192
            return None
        if text.startswith("build/devloop-observation-capacity/"):
            if len(relative.parts) == 3 and re.fullmatch(r"[0-9a-f]{64}\.zti", path.name):
                return 8192
            return None
        if text.startswith("build/devloop-gate-runs/"):
            return (
                65536
                if len(relative.parts) == 3
                and re.fullmatch(r"[0-9a-f]{64}\.json(?:\.partial)?", path.name)
                else None
            )
        if text.startswith("build/devloop-observations/"):
            return (
                65536
                if path.name in ("fixture_capacity.json", "fixture_capacity.json.partial")
                and len(relative.parts) == 3
                else None
            )
        if text in (
            "build",
            "build/devloop-diagnostics",
            "build/devloop-observation-capacity",
            "build/devloop-gate-runs",
            "build/devloop-observations",
        ):
            return 0
        return None

    def invoke(
        self,
        image: str,
        settings: dict,
        *,
        acceptance: str = "qualification-passes",
        target: str = "capacity-exam",
    ) -> tuple[subprocess.CompletedProcess, dict | None]:
        (self.root / "settings.json").write_text(json.dumps(settings))
        self.current_settings = settings
        self.expected_gate = "just-target" if acceptance == "target-passes" else "just-observations"
        self.previous_evidence = self.evidence()
        before_counter = (
            int((self.root / "counter").read_text()) if (self.root / "counter").exists() else 0
        )
        started = int(time.time())
        command = [
            "just",
            "devloop",
            "gate",
            self.identity,
            acceptance,
            "--policy",
            ".devloop/policy.json",
            "--inputs",
            "inputs.json",
            "--target",
            target,
            "--image",
            image,
        ]
        observed = audit.run(command, cwd=self.root, env=self.environment, timeout=240)
        observed.assert_no_spool(self.allowed)
        self.audit_runs.append(observed)
        finished = int(time.time())
        entries = self.evidence()
        self.validate_artifacts(observed, entries[-1]["identity"] if entries else None)
        require(
            entries[: len(self.previous_evidence)] == self.previous_evidence,
            "request rewrote historical evidence",
        )
        require(
            len(entries) == len(self.previous_evidence) + 1,
            "request did not append exactly one outcome",
        )
        counter = int((self.root / "counter").read_text())
        require(counter in (before_counter, before_counter + 1), "recipe ran more than once")
        for path, raw in self.snapshots.items():
            require(path.read_bytes() == raw, "follow-up overwrote retained artifact")
        if counter == before_counter + 1:
            self.invocation_bounds[image] = (started, finished)
        return observed.completed, entries[-1] if entries else None

    def validate_artifacts(
        self, observed, identity: dict | None, *, legacy_store: bool = False
    ) -> None:
        paths = {m.path for m in observed.mutations if m.path is not None}
        relay_paths = {p for p in paths if p.parent.name.startswith("slime-devloop-relay-")}
        require(len(relay_paths) <= 1, "extra relay files can conceal spool fragments")
        relay_events = [m for m in observed.mutations if m.path in relay_paths]
        require(
            all(
                any(arg == "scripts/check/devloop-gate.py" for arg in m.executable)
                for m in relay_events
            ),
            "relay written outside known adapter",
        )
        require(
            sum(m.operation in ("open", "openat", "creat") for m in relay_events) <= 3,
            "repeated relay opens conceal fragments",
        )
        require(
            not any(p.parent.name.startswith("diagnostic-read-") for p in paths),
            "adapter decoder temp files can conceal a spool",
        )
        require(identity is not None, "audit cannot attribute artifacts without execution identity")
        recipes = ("tasks_check", "docs_check") if legacy_store else ("fixture_capacity",)
        keys = {
            hashlib.sha256(
                json.dumps(
                    identity
                    | {
                        "gate": "work-item-store" if legacy_store else self.expected_gate,
                        "justTarget": name,
                    },
                    sort_keys=True,
                ).encode()
            ).hexdigest()
            for name in recipes
        }
        text = observed.completed.stdout + "\n" + observed.completed.stderr
        diagnostic_lines = DIAGNOSTIC.findall(text)
        announced = {self.root / Path(name) for name, _ in diagnostic_lines}
        require(
            all(key in keys for _, key in diagnostic_lines), "discovery key not expected execution"
        )
        output_pairs = set()
        for receipt in announced:
            require(
                any(receipt.parent.name.startswith(key + "-") for key in keys),
                "diagnostic path not bound to expected run",
            )
            receipt = safe_path(self.root, receipt.relative_to(self.root).as_posix())
            record = self.decode(receipt, diagnostic=True)
            line_keys = {key for name, key in diagnostic_lines if self.root / Path(name) == receipt}
            require(
                line_keys == {record["runKey"]} and record["runKey"] in keys,
                "diagnostic key differs from discovery",
            )
            output = safe_path(self.root, record["outputPath"])
            require(
                output == receipt.parent / "output.log", "diagnostic output is not exact sibling"
            )
            output_pairs |= {receipt, output}
        rawfiles = {
            p
            for p in paths
            if "devloop-diagnostics" in p.parts and p.name in ("receipt.zti", "output.log")
        }
        require(rawfiles <= output_pairs, "unannounced bounded fragments can hide spool")
        expected_cache_paths = {
            self.root / "build/devloop-gate-runs" / (key + suffix)
            for key in keys
            for suffix in (".json", ".json.partial")
        }
        capacity_lines = DISCOVERY.findall(text)
        expected_capacity_paths = {
            self.root / Path(name) for name, key in capacity_lines if key in keys
        }
        for path in paths:
            if "devloop-gate-runs" in path.parts and path.name != "devloop-gate-runs":
                require(path in expected_cache_paths, "cache fragment not bound to current run")
            if (
                "devloop-observation-capacity" in path.parts
                and path.name != "devloop-observation-capacity"
            ):
                require(
                    path in expected_capacity_paths,
                    "capacity fragment not bound to current discovery",
                )
        require(
            len(expected_capacity_paths) <= 1 and len(capacity_lines) <= 1,
            "extra capacity receipt fragments",
        )
        require(
            len(announced) <= (2 if legacy_store else 1), "too many captures for one invocation"
        )
        if legacy_store:
            discoveries = DIAGNOSTIC.findall(text)
            require(
                len(discoveries) == 2
                and len(announced) == 2
                and {key for _, key in discoveries} == keys,
                "legacy receipts are not distinct per recipe",
            )
            seen_recipes = set()
            for name, key in discoveries:
                record = self.decode(self.root / Path(name), diagnostic=True)
                require(
                    record["identity"] == identity and record["runKey"] == key,
                    "legacy receipt attribution mismatch",
                )
                seen_recipes.add(record["justTarget"])
            require(seen_recipes == set(recipes), "legacy recipe receipt omitted or duplicated")

    def decode(self, path: Path, *, diagnostic: bool = False) -> dict:
        decoder = self.root / "decode-capacity.zt"
        contract_path = (
            "contracts/devloop-diagnostics/v1/schema.zt"
            if diagnostic
            else "contracts/observation-capacity-refusal/v1/schema.zt"
        )
        type_name = "DiagnosticReceipt" if diagnostic else "CapacityRefusal"
        function = "decodeReceipt" if diagnostic else "decodeRefusal"
        decoder.write_text(
            f's ::= import "{contract_path}"; main :: Load -> Validation DecodeIssue s.{type_name} ! {{ load.zti : Path -> Data; }} = load => s.{function} (loadZti '
            + json.dumps(str(path))
            + "); main"
        )
        decoded = json.loads(self.run(str(zutai_cli.binary()), "json", str(decoder)).stdout)
        require(decoded.get("tag") == "valid", "capacity receipt fails typed schema")
        return decoded["payload"]["value"]

    def overflow(
        self, result: subprocess.CompletedProcess, evidence: dict | None, raw_count: int
    ) -> tuple[Path, bytes, dict]:
        text = result.stdout + "\n" + result.stderr
        require(
            result.returncode != 0 and re.search(r"capacity|stderr.*(bound|limit)", text, re.I),
            "overflow not refused as capacity",
        )
        require(
            evidence is not None and evidence["passed"] is False and evidence["observations"] == [],
            "overflow fabricated recipe/passing observations",
        )
        matches = DISCOVERY.findall(text)
        diagnostics = DIAGNOSTIC.findall(text)
        require(
            len(matches) == 1 and len(diagnostics) == 1, "overflow discovery missing/duplicated"
        )
        name, key = matches[0]
        diag_name, diag_key = diagnostics[0]
        require(key == diag_key, "capacity and diagnostic run differ")
        receipt = safe_path(self.root, name)
        diagnostic = safe_path(self.root, diag_name)
        require(receipt.stat().st_size <= 8192, "oversized replay receipt")
        record = self.decode(receipt)
        identity = evidence["identity"]
        bound = identity | {"gate": "just-observations", "justTarget": "fixture_capacity"}
        require(
            key == hashlib.sha256(json.dumps(bound, sort_keys=True).encode()).hexdigest(),
            "capacity key omits identity/gate",
        )
        expected_exit, expected_output = self.reference_raw(self.current_settings)
        validate_refusal(
            record,
            identity,
            key,
            raw_count,
            diag_name,
            hashlib.sha256(diagnostic.read_bytes()).hexdigest(),
            expected_exit,
        )
        bounds = self.invocation_bounds[identity["image"]]
        require(bounds[0] <= record["finishedAt"] <= bounds[1], "refusal finish time fabricated")
        diagnostic_record = self.decode(diagnostic, diagnostic=True)
        output = safe_path(self.root, diagnostic_record["outputPath"])
        capture = Capture(diagnostic_record, output.read_bytes(), diagnostic, output)
        validate_diagnostic(
            capture,
            identity,
            "fixture_capacity",
            (),
            truncated=True,
            expected_exit=expected_exit,
            gate="just-observations",
        )
        require(
            capture.output == expected_output, "retained diagnostic prefix differs from raw oracle"
        )
        for path in (receipt, diagnostic, output):
            self.snapshots[path] = path.read_bytes()
        return receipt, receipt.read_bytes(), record

    def reference_raw(self, settings: dict) -> tuple[int, bytes]:
        counter = self.root / "counter"
        prior = counter.read_bytes()
        with tempfile.TemporaryFile() as out, tempfile.TemporaryFile() as err:
            result = subprocess.run(
                ["just", FIXTURE_RECIPE],
                cwd=self.root,
                env=self.environment,
                stdout=out,
                stderr=err,
                timeout=240,
            )
            out.seek(0)
            stdout = out.read(LIMIT)
            err.seek(0)
            stderr = err.read(max(0, LIMIT - len(stdout)))
        counter.write_bytes(prior)
        return result.returncode, stdout + stderr


def legacy_store_qualification() -> None:
    with tempfile.TemporaryDirectory(prefix="capacity-legacy-store-") as temporary:
        fixture = CapacityFixture(Path(temporary))
        policy_path = fixture.root / ".devloop/policy.json"
        policy = json.loads(policy_path.read_text())
        real = json.loads((ROOT / ".devloop/policy.json").read_text())
        policy["gates"] += [g for g in real["gates"] if g["id"] == "work-item-store"]
        policy_path.write_text(json.dumps(policy))
        source = fixture.root / "legacy.zti"
        source.write_text(
            (fixture.root / "spec.zti")
            .read_text()
            .replace('gate = "just-target"', 'gate = "work-item-store"')
            .replace('observation = "passed"', 'observation = "tasksCheckPassed"')
        )
        admitted = fixture.run(
            "just",
            "devloop",
            "admit",
            "legacy.zti",
            "--title",
            "Legacy capacity fixture",
            "--kind",
            "bug",
            "--admission",
            "legacy-capacity-fixture",
            "--policy",
            ".devloop/policy.json",
        )
        fixture.identity = json.loads(admitted.stdout)["id"]
        fixture.run(
            "just", "devloop", "start", fixture.identity, "--policy", ".devloop/policy.json"
        )
        with (fixture.root / "Justfile").open("a") as justfile:
            justfile.write(
                "\\ntasks_check:\\n    @python3 fixture.py tasks\\n\\ndocs_check:\\n    @python3 fixture.py docs\\n".replace(
                    "\\n", "\n"
                )
            )
        (fixture.root / "fixture.py").write_text(
            'import os,pathlib,sys; failed=pathlib.Path("fail").exists(); os.write(1,b"Z"*131072); print("work-item check passed: 3 items, 7 validated through devloop"); print("LEGACY-FAIL" if failed else "LEGACY-PASS"); sys.exit(1 if failed else 0)'
        )
        for failed in (False, True):
            if failed:
                (fixture.root / "fail").touch()
            command = [
                "just",
                "devloop",
                "gate",
                fixture.identity,
                "qualification-passes",
                "--policy",
                ".devloop/policy.json",
                "--inputs",
                "inputs.json",
                "--target",
                "legacy-store",
                "--image",
                str(failed),
            ]
            observed = audit.run(command, cwd=fixture.root, env=fixture.environment, timeout=240)
            observed.assert_no_spool(fixture.allowed)
            evidence = fixture.evidence()[-1]
            fixture.validate_artifacts(observed, evidence["identity"], legacy_store=True)
            require(
                (observed.completed.returncode != 0) is failed,
                "legacy CLI acceptance differs from checker exits",
            )
            values = {v["id"]: v["value"] for v in evidence["observations"]}
            require(
                values["tasksCheckPassed"]["boolValue"] is not failed
                and values["docsCheckPassed"]["boolValue"] is not failed,
                "legacy exit semantics changed",
            )
            require(
                values["specDrivenItemsValidated"]["intValue"] == 7,
                "legacy parser clipped complete text",
            )
            require(
                len(DIAGNOSTIC.findall(observed.completed.stdout + observed.completed.stderr)) == 2,
                "legacy recipes did not each expose receipt",
            )
        print(
            "capacity legacy store: full parser and success/failure diagnostics without spool qualified"
        )


def qualification() -> None:
    with tempfile.TemporaryDirectory(prefix="observation-capacity-exam-") as temporary:
        fixture = CapacityFixture(Path(temporary))
        for count in (LIMIT - 1, LIMIT):
            result, evidence = fixture.invoke(f"boundary-{count}", {"stderr": count})
            require(
                result.returncode == 0 and evidence is not None, "under-bound observation refused"
            )
            values = {v["id"]: v["value"] for v in evidence["observations"]}
            require(
                values["passed"]["boolValue"] is True
                and values["transcriptDigest"]["textValue"] == digest_for(0, b"E" * count),
                "under-bound digest changed",
            )
        # The strict UTF-8 baseline may refuse here before capacity replay;
        # this is reported as the actual first failed obligation.
        split_settings = {"stderr": 0, "split": True}
        result, evidence = fixture.invoke("split-normalization", split_settings)
        expected = hashlib.sha256(
            normalize(b"A\r\nB\r" + "雪".encode() + b"\xff")
            + normalize(b"X\r\n" + "é".encode() + b"\xfe\r")
        ).hexdigest()
        values = {v["id"]: v["value"] for v in evidence["observations"]}
        require(
            result.returncode == 0 and values["transcriptDigest"]["textValue"] == expected,
            f"split byte/newline digest oracle differs: exit={result.returncode}; stdout={result.stdout}; stderr={result.stderr}",
        )
        settings = {"stderr": LIMIT + 1}
        result, evidence = fixture.invoke("overflow", settings)
        receipt, original, record = fixture.overflow(result, evidence, LIMIT + 1)
        counter = (fixture.root / "counter").read_bytes()
        before = fixture.evidence()
        result, evidence = fixture.invoke("overflow", settings)
        again, raw, _ = fixture.overflow(result, evidence, LIMIT + 1)
        require(
            again == receipt
            and raw == original
            and (fixture.root / "counter").read_bytes() == counter,
            "cached overflow reran/overwrote",
        )
        require(
            fixture.evidence()[: len(before)] == before,
            "cached overflow rewrote historical evidence",
        )
        result, evidence = fixture.invoke("changed-image", settings)
        fixture.overflow(result, evidence, LIMIT + 1)
        require(
            (fixture.root / "counter").read_bytes() != counter and receipt.read_bytes() == original,
            "new identity reused/overwrote refusal",
        )
        # Age only this isolated receipt with Zutai's own structural formatter,
        # not an independently handwritten persistence codec.
        transform = fixture.root / "age-refusal.zt"
        transform.write_text(
            's ::= import "contracts/observation-capacity-refusal/v1/schema.zt"; d ::= import '
            + json.dumps(receipt.relative_to(fixture.root).as_posix())
            + "; d with { finishedAt = d.finishedAt - 7200; reuseUntil = d.reuseUntil - 7200; }"
        )
        aged = fixture.run(str(zutai_cli.binary()), "run", str(transform)).stdout
        receipt.write_text(aged + "\n")
        fixture.snapshots[receipt] = receipt.read_bytes()
        counter = (fixture.root / "counter").read_bytes()
        result, evidence = fixture.invoke("overflow", settings)
        fresh, _, _ = fixture.overflow(result, evidence, LIMIT + 1)
        require(
            fresh != receipt and (fixture.root / "counter").read_bytes() != counter,
            "expired overflow did not rerun",
        )
        counter = (fixture.root / "counter").read_bytes()
        result, target_evidence = fixture.invoke(
            "overflow", {"stderr": LIMIT + 1}, acceptance="target-passes"
        )
        require(
            result.returncode == 0
            and target_evidence["observations"][0]["value"]["boolValue"] is True,
            "observation overflow contaminated just-target",
        )
        require(
            (fixture.root / "counter").read_bytes() != counter,
            "cross-gate request reused observation refusal",
        )
        result, _ = fixture.invoke(
            "reverse-gate", {"stderr": LIMIT + 1}, acceptance="target-passes"
        )
        counter = (fixture.root / "counter").read_bytes()
        result, evidence = fixture.invoke("reverse-gate", {"stderr": LIMIT + 1})
        fixture.overflow(result, evidence, LIMIT + 1)
        require(
            (fixture.root / "counter").read_bytes() != counter,
            "just-target result contaminated observation overflow",
        )
        base_policy = (fixture.root / ".devloop/policy.json").read_bytes()
        base_inputs = (fixture.root / "inputs.json").read_bytes()
        code_file = fixture.root / "fixture.py"
        base_code = code_file.read_bytes()
        for field in ("target", "code", "policy", "inputs"):
            result, before_entry = fixture.invoke("field-base-" + field, {"stderr": LIMIT + 1})
            fixture.overflow(result, before_entry, LIMIT + 1)
            counter = int((fixture.root / "counter").read_text())
            if field == "code":
                code_file.write_bytes(base_code + b"\n# identity control\n")
            elif field == "policy":
                (fixture.root / ".devloop/policy.json").write_bytes(base_policy + b"\n")
            elif field == "inputs":
                (fixture.root / "inputs.json").write_bytes(base_inputs + b"\n")
            result, changed = fixture.invoke(
                "field-base-" + field,
                {"stderr": LIMIT + 1},
                target="capacity-other" if field == "target" else "capacity-exam",
            )
            fixture.overflow(result, changed, LIMIT + 1)
            diffs = {
                name
                for name in FIELDS
                if changed["identity"][name] != before_entry["identity"][name]
            }
            require(diffs == {field}, "identity control changed wrong fields: " + repr(diffs))
            require(
                int((fixture.root / "counter").read_text()) == counter + 1,
                "identity change did not execute exactly once",
            )
            code_file.write_bytes(base_code)
            (fixture.root / ".devloop/policy.json").write_bytes(base_policy)
            (fixture.root / "inputs.json").write_bytes(base_inputs)
        result, before_entry = fixture.invoke("epoch-field", {"stderr": LIMIT + 1})
        fixture.overflow(result, before_entry, LIMIT + 1)
        counter = int((fixture.root / "counter").read_text())
        fixture.run("myque", "reopen", fixture.identity)
        fixture.run(
            "just", "devloop", "start", fixture.identity, "--policy", ".devloop/policy.json"
        )
        result, changed = fixture.invoke("epoch-field", {"stderr": LIMIT + 1})
        fixture.overflow(result, changed, LIMIT + 1)
        require(
            {name for name in FIELDS if changed["identity"][name] != before_entry["identity"][name]}
            == {"epoch"},
            "epoch control changed extra identity fields",
        )
        require(
            int((fixture.root / "counter").read_text()) == counter + 1, "fresh epoch reused refusal"
        )
        print(
            "capacity identities: target/code/policy/inputs/image/epoch/gate independently exercised; pinned helper unchanged"
        )
        for image, settings in (
            ("large-success", {"stderr": 8 << 20}),
            ("large-fail", {"stderr": 8 << 20, "exit": 23}),
            ("mixed-success", {"stderr": (8 << 20) + LIMIT + 1, "stdout": 8 << 20, "mixed": True}),
            ("reverse-mixed", {"stderr": 8 << 20, "stdout": 8 << 20}),
            ("reverse-mixed-fail", {"stderr": 8 << 20, "stdout": 8 << 20, "exit": 23}),
            ("alternating", {"stderr": 8 << 20, "stdout": 8 << 20, "alternate": True}),
            (
                "mixed-fail",
                {"stderr": (8 << 20) + LIMIT + 1, "stdout": 8 << 20, "mixed": True, "exit": 23},
            ),
            ("multibyte", {"stderr": LIMIT + 2, "pattern": "c3a9"}),
            ("crlf", {"stderr": LIMIT + 2, "pattern": "0d0a"}),
        ):
            result, evidence = fixture.invoke(image, settings)
            # just adds error text for nonzero recipes; count from independent run.
            count = settings["stderr"]
            if settings.get("exit"):
                saved_counter = (fixture.root / "counter").read_bytes()
                reference = fixture.run("just", FIXTURE_RECIPE, check=False, output_limit=32 << 20)
                (fixture.root / "counter").write_bytes(saved_counter)
                count = len(reference.stderr.encode())
            fixture.overflow(result, evidence, count)
        for image, settings in (
            ("memory-stdout", {"stderr": 0, "stdout": 128 << 20, "slow": True}),
            ("memory-stderr", {"stderr": 128 << 20, "slow": True}),
            (
                "memory-mixed",
                {"stderr": 128 << 20, "stdout": 128 << 20, "mixed": True, "slow": True},
            ),
        ):
            result, evidence = fixture.invoke(image, settings)
            fixture.audit_runs[-1].require_memory(MEMORY_DELTA)
            if settings["stderr"] > LIMIT:
                fixture.overflow(result, evidence, settings["stderr"])
            else:
                require(result.returncode == 0, "streaming stdout failed")
                values = {v["id"]: v["value"] for v in evidence["observations"]}
                require(
                    values["transcriptDigest"]["textValue"] == digest_for(settings["stdout"], b""),
                    "streaming stdout digest mismatch",
                )
        print(
            "observation capacity exam: boundaries, drain, raw accounting, memory, replay and filesystem audit qualified"
        )
    legacy_store_qualification()


def main(*, controls_only: bool = False) -> int:
    try:
        controls()
        if not controls_only:
            qualification()
    except audit.AuditError as error:
        raise ExamError(str(error)) from error
    return 0
