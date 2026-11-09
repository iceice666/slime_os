#!/usr/bin/env python3
"""Judge one executable contract with its oracle and its negative controls.

    scripts/check/check-dbc.py contracts/<interface>/v<N>

The directory holds `schema.zt`, `clauses.zt` (an `Interface`), `oracle.zt`
(`inspect fixture observed -> Summary`), `fixture.zt` (stimulus only),
`bad-traces.zt` (tampers) and `mutations.zt`. For each host-sim exam the
interface names, this checker:

1. builds the adapter crate against the real implementation, feeds it the
   fixture, and requires zero violations from the oracle;
2. replays every tamper on the honest observation and requires the oracle to
   report the tamper's clause;
3. drops the last observation of the first case and requires the oracle to
   report `observation-missing`;
4. builds the adapter against a copy of the implementation with each
   mutation applied and requires the oracle to report the mutation's clause.

A clause is covered when the honest run exercised it and at least one
tamper or mutation made the oracle report it. The checker exits non-zero
when the honest run has a violation or any negative control is not refused.
It records `clauses`, `clausesChecked`, `clausesCovered`, `violations`,
`events`, `negativeControls`, `negativeControlsRefused` and one flag
`covered__<contract>__<clause>` per checked clause (`-` becomes `_`).
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))
import devloop_observations  # noqa: E402
import zutai_cli  # noqa: E402
from harness import ROOT  # noqa: E402

SOURCES = ("schema.zt", "oracle.zt", "fixture.zt")
# The tree under judgement. scripts/spec/ratchet sets it so the base's
# checker judges a pull request's head (ratchet R2).
TREE = Path(os.environ.get("DBC_TREE") or ROOT)
RUN = 'o ::= import "./oracle.zt";\nf ::= import "./fixture.zt";\nb ::= import "./observed.zt";\no.inspect f b\n'


def evaluate(path: Path) -> object:
    done = subprocess.run(
        [str(zutai_cli.binary()), "json", str(path)],
        env=os.environ | {"ZUTAI_STDLIB_ROOT": str(zutai_cli.STDLIB)},
        check=False,
        text=True,
        capture_output=True,
    )
    if done.returncode != 0:
        raise SystemExit(f"dbc: zutai-cli json {path.name} failed\n{done.stderr}")
    return json.loads(done.stdout)


def flag(contract: str, clause: str) -> str:
    return re.sub(r"[^A-Za-z0-9_]", "_", f"covered__{contract}__{clause}")


def build(adapter: Path, target: Path) -> Path:
    done = subprocess.run(
        ["cargo", "build", "--quiet", "--manifest-path", str(adapter / "Cargo.toml"), "--target-dir", str(target)],
        cwd=TREE,
        check=False,
        text=True,
        capture_output=True,
    )
    if done.returncode != 0:
        raise SystemExit(f"dbc: cargo build of {adapter} failed\n{done.stderr}")
    binaries = [p for p in (target / "debug").iterdir() if p.is_file() and os.access(p, os.X_OK) and p.suffix == ""]
    if len(binaries) != 1:
        raise SystemExit(f"dbc: expected one adapter binary in {target / 'debug'}, found {binaries}")
    return binaries[0]


def observe(binary: Path, fixture_json: str) -> str:
    done = subprocess.run([str(binary)], input=fixture_json, check=False, text=True, capture_output=True)
    if done.returncode != 0:
        raise SystemExit(f"dbc: adapter {binary.name} failed\n{done.stderr}")
    return done.stdout


class Oracle:
    """Evaluates the oracle on observations in a scratch copy of the interface sources."""

    def __init__(self, directory: Path, scratch: Path) -> None:
        self.scratch = scratch / "oracle"
        self.scratch.mkdir()
        for name in SOURCES:
            shutil.copyfile(directory / name, self.scratch / name)
        (self.scratch / "run.zt").write_text(RUN)

    def judge(self, observed: str) -> dict:
        (self.scratch / "observed.zt").write_text(observed)
        return evaluate(self.scratch / "run.zt")


def observation_text(value: dict) -> str:
    """Zutai source for one Observation as `zutai-cli json` printed it."""
    body = value["payload"]
    if value["tag"] == "pushed":
        outcome = body["outcome"] if isinstance(body["outcome"], str) else "#" + body["outcome"]["tag"]
        return f"#pushed {{ outcome = {outcome}; complete = {str(body['complete']).lower()}; }};"
    return f"#read {{ present = {str(body['present']).lower()}; length = {body['length']}; }};"


def render(cases: list[dict]) -> str:
    """An Observed document from the JSON of one; the inverse of the adapter's printing."""
    lines = ['s ::= import "./schema.zt";', "observed :: s.Observed = {", "  schema = s.observedVersion;", "  cases = {"]
    for case in cases:
        steps = " ".join(observation_text(o) for o in case["steps"]) or ";"
        lines.append(f"    {{ created = {str(case['created']).lower()}; complete = {str(case['complete']).lower()}; steps = {{ {steps} }}; }};")
    lines += ["  };", "};", "observed", ""]
    return "\n".join(lines)


def mutant(directory: Path, adapter: Path, mutation: dict, scratch: Path) -> Path:
    """A copy of the adapter wired to a copy of the mutated crate, outside every workspace."""
    file = TREE / mutation["file"]
    crate = next(p for p in file.parents if (p / "Cargo.toml").is_file())
    root = scratch / mutation["id"]
    shutil.copytree(crate, root / "crate", ignore=shutil.ignore_patterns("target"))
    manifest = (root / "crate" / "Cargo.toml").read_text()
    manifest = re.sub(r"\n\[dev-dependencies\][^\[]*", "\n", manifest)
    manifest = re.sub(r"\n\[lints\]\nworkspace = true\n", "\n", manifest)
    (root / "crate" / "Cargo.toml").write_text(manifest + "\n[workspace]\n")
    target = root / "crate" / file.relative_to(crate)
    source = target.read_text()
    if source.count(mutation["find"]) != 1:
        raise SystemExit(f"dbc: mutation {mutation['id']}: `find` occurs {source.count(mutation['find'])} times in {mutation['file']}")
    target.write_text(source.replace(mutation["find"], mutation["replace"]))
    shutil.copytree(adapter, root / "adapter", ignore=shutil.ignore_patterns("target"))
    adapter_manifest = (root / "adapter" / "Cargo.toml").read_text()
    relative = os.path.relpath(crate, adapter)
    if relative not in adapter_manifest:
        raise SystemExit(f"dbc: {adapter}/Cargo.toml does not name {relative}")
    (root / "adapter" / "Cargo.toml").write_text(adapter_manifest.replace(relative, "../crate"))
    return root / "adapter"


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    directory = (TREE / sys.argv[1]).resolve()
    interface = evaluate(directory / "clauses.zt")
    fixture = evaluate(directory / "fixture.zt")
    tampers = evaluate(directory / "bad-traces.zt")
    mutations = evaluate(directory / "mutations.zt")
    clauses = [(c["id"], cl["id"], cl["how"]) for c in interface["contracts"] for cl in c["clauses"]]
    checked = {(c, cl) for c, cl, how in clauses if how == "#oracle"}
    clause_ids = {cl for _, cl in checked}
    if len(clause_ids) != len(checked):
        raise SystemExit("dbc: two contracts of one interface check the same clause id; coverage is keyed by clause id")
    fixture_json = json.dumps(fixture)
    failures: list[str] = []
    refused = attempted = 0
    exercised: Counter[str] = Counter()
    refused_clauses: set[str] = set()
    violations = events = 0

    with tempfile.TemporaryDirectory(prefix="slime-dbc-") as temporary:
        scratch = Path(temporary)
        oracle = Oracle(directory, scratch)
        for exam in interface["exams"]:
            if exam["backend"] != "hostSim":
                continue
            adapter = TREE / exam["adapter"]
            honest = observe(build(adapter, TREE / "build" / "dbc" / interface["id"]), fixture_json)
            summary = oracle.judge(honest)
            violations += len(summary["violations"])
            for violation in summary["violations"]:
                failures.append(f"honest run: {violation['clause']} at {violation['case']} step {violation['step']}: {violation['detail']}")
            exercised.update(summary["exercised"])
            events += len(summary["exercised"])
            (scratch / "oracle" / "observed.zt").write_text(honest)
            observed = evaluate(scratch / "oracle" / "observed.zt")

            def control(label: str, cases: list[dict], clause: str) -> None:
                nonlocal refused, attempted
                attempted += 1
                reported = {v["clause"] for v in oracle.judge(render(cases))["violations"]}
                if clause in reported:
                    refused += 1
                    refused_clauses.add(clause)
                else:
                    failures.append(f"{label}: the oracle reported {sorted(reported) or 'nothing'}, not {clause}")

            for tamper in tampers:
                cases = json.loads(json.dumps(observed["cases"]))
                cases[tamper["case"]]["steps"][tamper["step"]] = tamper["observation"]
                control(f"tamper {tamper['clause']} at case {tamper['case']} step {tamper['step']}", cases, tamper["clause"])
            dropped = json.loads(json.dumps(observed["cases"]))
            dropped[0]["steps"].pop()
            control("dropped observation", dropped, "observation-missing")
            for mutation in mutations:
                binary = build(mutant(directory, adapter, mutation, scratch / "mutants"), scratch / "mutants" / mutation["id"] / "target")
                cases = evaluate_observed(oracle, observe(binary, fixture_json))
                control(f"mutation {mutation['id']}", cases, mutation["clause"])

    covered = {(c, cl) for c, cl in checked if exercised[cl] and cl in refused_clauses}
    unknown = sorted(({t["clause"] for t in tampers} | {m["clause"] for m in mutations}) - clause_ids)
    if unknown:
        failures.append(f"negative controls name clauses the interface does not check: {unknown}")
    for failure in failures:
        print(f"dbc: FAIL {failure}", file=sys.stderr)
    print(
        f"dbc: {interface['id']}: {len(clauses)} clause(s), {len(checked)} checked, {len(covered)} covered; "
        f"{violations} violation(s) in {events} judgement(s); {refused}/{attempted} negative control(s) refused"
    )
    devloop_observations.record(
        clauses=len(clauses),
        clausesChecked=len(checked),
        clausesCovered=len(covered),
        violations=violations,
        events=events,
        negativeControls=attempted,
        negativeControlsRefused=refused,
        **{flag(c, cl): (c, cl) in covered for c, cl in sorted(checked)},
    )
    return 1 if failures else 0


def evaluate_observed(oracle: Oracle, text: str) -> list[dict]:
    (oracle.scratch / "observed.zt").write_text(text)
    return evaluate(oracle.scratch / "observed.zt")["cases"]


if __name__ == "__main__":
    raise SystemExit(main())
