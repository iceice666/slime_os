"""Judge ambient monotonic clock reads: no read right, no root round trip, one clock domain.

Three judges, each with its own refusal controls:

* `judge_transcript` reads the sel4-clock-authority boot. The root never serves
  or refuses a monotonic or rate read (labels 44 and 70) on behalf of any task,
  so every reading probe read the counter itself; and the timer holder's own
  reads bracket a root-armed delay, so the counter it reads is the one the
  root's timers count.
* `judge_static` reads the tracked sources and generated bindings. No system
  spec, baseline, derived composition or manifest schema names
  `monotonicRead`, the generation rights vocabulary no longer defines bit 26,
  and the clock-authority binding is format 2.
* `judge_builder` drives the generation builder's clock-authority encoder. A
  row that carries a `monotonicRead` key, true or false, is refused rather than
  ignored, and so is a row that declares no remaining authority.
"""
from __future__ import annotations

import re
from collections.abc import Callable, Mapping

READ_LABELS = (44, 70)
RETIRED_RIGHT_BIT = 26
RETIRED_FIELD = "monotonicRead"
RETIRED_RIGHT_NAME = "clockMonotonicRead"
CLOCK_AUTHORITY_VERSION = 2

DOMAIN = re.compile(r"^\[clock-probe:timer\] domain armed=(\d+) delay=(\d+) woke=(\d+)$", re.M)
SERVED = re.compile(r"^SLIME_CLOCK served task=(\d+) label=(\d+) ", re.M)
UNDECLARED = re.compile(r"^SLIME_GRAPH service refused task=(\d+) label=(\d+) class=undeclared", re.M)
RIGHT_DEFINITION = re.compile(r'^\s*right "CLOCK_MONOTONIC_READ"', re.M)

# Paths, relative to the repository root, whose text must not name the retired
# field. A directory entry covers every tracked file beneath it.
FIELD_FREE_PATHS = (
    "contracts/system-spec/v1/schema.zt",
    "contracts/system-spec/v1/systems",
    "contracts/system-spec/v1/baselines",
    "contracts/system-spec/v1/sources",
    "contracts/generation-manifest/v1/schema.zt",
    "contracts/generation-manifest/v1/compositions",
)
RIGHTS_VOCABULARY = "contracts/generation/v5/vocab/rights.zt"


class JudgeError(Exception):
    """A judged fact is false."""


def require(condition: bool, message: str) -> None:
    if not condition:
        raise JudgeError(message)


def judge_transcript(transcript: str) -> dict[str, int]:
    """Refuse a root-served read and a timer domain the direct counter disagrees with."""
    served = sorted({(task, label) for task, label in SERVED.findall(transcript) if int(label) in READ_LABELS})
    require(not served, f"the root served a monotonic or rate read for task/label {served}; ambient reads never reach the root")
    refused = sorted({(task, label) for task, label in UNDECLARED.findall(transcript) if int(label) in READ_LABELS})
    require(not refused, f"the root refused a monotonic or rate read as undeclared for task/label {refused}; no read right remains to lack")
    domains = DOMAIN.findall(transcript)
    require(len(domains) == 1, f"expected exactly one [clock-probe:timer] domain line, saw {len(domains)}")
    armed, delay, woke = (int(value) for value in domains[0])
    require(delay > 0, "the timer holder armed a zero delay, which brackets nothing")
    require(woke >= armed + delay, f"the timer holder read {woke} on expiry, before armed {armed} plus delay {delay}; its counter is not the root timer's")
    return {"armed": armed, "delay": delay, "woke": woke}


def _field_free(texts: Mapping[str, str]) -> list[str]:
    return sorted(
        path
        for path, text in texts.items()
        if any(path == root or path.startswith(root + "/") for root in FIELD_FREE_PATHS) and RETIRED_FIELD in text
    )


def judge_static(texts: Mapping[str, str], bindings: Mapping[str, object]) -> int:
    """Refuse any surviving declaration of the read right; return the number of files examined.

    `texts` maps repository-relative paths to file text and must include every
    tracked file under `FIELD_FREE_PATHS` and the rights vocabulary. `bindings`
    is the generated `scripts/lib/boot_contracts.py` namespace.
    """
    examined = [path for path in texts if any(path == root or path.startswith(root + "/") for root in FIELD_FREE_PATHS)]
    require(any(path.startswith("contracts/generation-manifest/v1/compositions/") for path in examined), "no derived composition was examined")
    require(any(path.startswith("contracts/system-spec/v1/systems/") for path in examined), "no system spec was examined")
    naming = _field_free(texts)
    require(not naming, f"{RETIRED_FIELD} is still declared in {naming[:4]}")
    vocabulary = texts.get(RIGHTS_VOCABULARY)
    require(vocabulary is not None, f"{RIGHTS_VOCABULARY} was not examined")
    require(RIGHT_DEFINITION.search(vocabulary) is None, "the rights vocabulary still defines CLOCK_MONOTONIC_READ")
    by_name = bindings.get("GENERATION_RIGHT_BY_MANIFEST_NAME")
    require(isinstance(by_name, Mapping), "the generated bindings carry no rights table")
    require(RETIRED_RIGHT_NAME not in by_name, f"the generated rights table still names {RETIRED_RIGHT_NAME}")
    require(all(value != 1 << RETIRED_RIGHT_BIT for value in by_name.values()), f"the generated rights table still assigns bit {RETIRED_RIGHT_BIT}")
    every = bindings.get("GENERATION_RIGHT_ALL")
    require(isinstance(every, int) and not every & (1 << RETIRED_RIGHT_BIT), f"GENERATION_RIGHT_ALL still admits bit {RETIRED_RIGHT_BIT}")
    require("GENERATION_RIGHT_CLOCK_MONOTONIC_READ" not in bindings, "the generated bindings still export GENERATION_RIGHT_CLOCK_MONOTONIC_READ")
    version = bindings.get("CLOCK_AUTHORITY_VERSION")
    require(version == CLOCK_AUTHORITY_VERSION, f"the clock-authority binding is format {version}, not {CLOCK_AUTHORITY_VERSION}")
    return len(examined)


def judge_builder(validate: Callable[[dict], object], manifest: dict) -> tuple[int, int]:
    """Require the encoder to accept the honest table and refuse every retired-field or empty row.

    `validate` refuses by raising SystemExit. Returns (accepted, refused).
    """
    rows = manifest.get("clockAuthority") or []
    require(rows, "the derived clock-authority manifest declares no clock authority")
    try:
        validate(manifest)
    except SystemExit as error:
        raise JudgeError(f"the builder refused the derived clock-authority table: {error}") from None
    accepted, refused = 1, 0
    candidates: list[tuple[str, list[dict]]] = []
    for index, row in enumerate(rows):
        for value in (True, False):
            mutated = [dict(other, **({RETIRED_FIELD: value} if position == index else {})) for position, other in enumerate(rows)]
            candidates.append((f"{row['holder']} {RETIRED_FIELD}={value}", mutated))
    holder = next(entry["name"] for entry in manifest["instances"] if entry["name"] not in {row["holder"] for row in rows})
    empty = {"holder": holder, "timerUse": False, "simulatedRead": False, "simulatedAdvance": False, "timerQuota": 0}
    candidates.append((f"{holder} with no authority", [*rows, empty]))
    candidates.append((f"{holder} with only {RETIRED_FIELD}", [*rows, dict(empty, **{RETIRED_FIELD: True})]))
    for name, mutated in candidates:
        try:
            validate({**manifest, "clockAuthority": mutated})
        except SystemExit:
            refused += 1
            continue
        raise JudgeError(f"the builder accepted the clock-authority row {name}")
    return accepted, refused


# --- refusal controls -------------------------------------------------------

HONEST_TRANSCRIPT = "\n".join(
    (
        "SLIME_CLOCK served task=3 label=45 result=1 live=1",
        "SLIME_CLOCK served task=4 label=47 result=0 live=0",
        "SLIME_GRAPH service refused task=6 label=45 class=undeclared",
        "SLIME_CLOCK malformed task=6 label=44 words=1 expected=Some(0)",
        "[clock-probe:timer] domain armed=1000 delay=500 woke=1600",
    )
)


def _honest_texts() -> dict[str, str]:
    return {
        "contracts/system-spec/v1/schema.zt": "ClockAuthority :: type { holder : Text; timerUse : Bool; };",
        "contracts/system-spec/v1/systems/sel4-clock-authority.zti": "{ holder = \"clock-timer\"; timerUse = true; }",
        "contracts/generation-manifest/v1/compositions/sel4-clock-authority.zti": "{ holder = \"clock-timer\"; timerUse = true; }",
        "contracts/syscall-abi/v1/schema.zt": "-- gated on the retired monotonicRead right before format 2",
        RIGHTS_VOCABULARY: '  -- Bit 26 was CLOCK_MONOTONIC_READ, retired.\n  right "CLOCK_TIMER_USE" 27 "clockTimerUse" "recorded";\n',
    }


def _honest_bindings() -> dict[str, object]:
    return {
        "GENERATION_RIGHT_BY_MANIFEST_NAME": {"clockTimerUse": 1 << 27, "clockSimulatedRead": 1 << 28},
        "GENERATION_RIGHT_ALL": (1 << 27) | (1 << 28),
        "CLOCK_AUTHORITY_VERSION": CLOCK_AUTHORITY_VERSION,
    }


def _honest_manifest() -> dict:
    return {
        "instances": [{"name": "clock-timer"}, {"name": "clock-denied"}],
        "clockAuthority": [{"holder": "clock-timer", "timerUse": True, "simulatedRead": False, "simulatedAdvance": False, "timerQuota": 2}],
    }


def _strict_validate(manifest: dict) -> None:
    for row in manifest["clockAuthority"]:
        if RETIRED_FIELD in row:
            raise SystemExit("retired field")
        if not (row["timerUse"] or row["simulatedRead"] or row["simulatedAdvance"]):
            raise SystemExit("no authority")


def _ignoring_validate(manifest: dict) -> None:
    for row in manifest["clockAuthority"]:
        if not (row["timerUse"] or row["simulatedRead"] or row["simulatedAdvance"] or row.get(RETIRED_FIELD)):
            raise SystemExit("no authority")


def _accepting_validate(manifest: dict) -> None:
    del manifest


def _refusing_validate(manifest: dict) -> None:
    del manifest
    raise SystemExit("refused")


def _mutated(mapping: Mapping[str, object], **changes: object) -> dict[str, object]:
    return {**mapping, **changes}


def controls() -> int:
    """Run every honest case and every refusal control; return the number refused."""
    judge_transcript(HONEST_TRANSCRIPT)
    judge_static(_honest_texts(), _honest_bindings())
    require(judge_builder(_strict_validate, _honest_manifest()) == (1, 4), "the strict model builder did not refuse exactly the four mutated tables")
    texts, bindings = _honest_texts(), _honest_bindings()
    composition = "contracts/generation-manifest/v1/compositions/sel4-clock-authority.zti"
    rights = bindings["GENERATION_RIGHT_BY_MANIFEST_NAME"]
    assert isinstance(rights, dict)
    cases: list[tuple[str, Callable[[], object]]] = [
        ("served monotonic read", lambda: judge_transcript(HONEST_TRANSCRIPT + "\nSLIME_CLOCK served task=2 label=44 result=7 live=0")),
        ("served rate read", lambda: judge_transcript(HONEST_TRANSCRIPT + "\nSLIME_CLOCK served task=2 label=70 result=62500000 live=0")),
        ("undeclared monotonic read", lambda: judge_transcript(HONEST_TRANSCRIPT + "\nSLIME_GRAPH service refused task=6 label=44 class=undeclared")),
        ("undeclared rate read", lambda: judge_transcript(HONEST_TRANSCRIPT + "\nSLIME_GRAPH service refused task=6 label=70 class=undeclared")),
        ("missing domain line", lambda: judge_transcript(HONEST_TRANSCRIPT.replace("[clock-probe:timer] domain", "[clock-probe:timer] other"))),
        ("doubled domain line", lambda: judge_transcript(HONEST_TRANSCRIPT + "\n[clock-probe:timer] domain armed=1000 delay=500 woke=1600")),
        ("zero delay", lambda: judge_transcript(HONEST_TRANSCRIPT.replace("delay=500", "delay=0"))),
        ("woke before deadline", lambda: judge_transcript(HONEST_TRANSCRIPT.replace("woke=1600", "woke=1499"))),
        ("field in a system spec", lambda: judge_static(_mutated(texts, **{"contracts/system-spec/v1/systems/sel4-clock-authority.zti": "monotonicRead = true;"}), bindings)),
        ("field in a baseline", lambda: judge_static(_mutated(texts, **{"contracts/system-spec/v1/baselines/sel4-io-tcp.zti": "monotonicRead = false;"}), bindings)),
        ("field in a derived composition", lambda: judge_static(_mutated(texts, **{composition: "monotonicRead = true;"}), bindings)),
        ("field in the manifest schema", lambda: judge_static(_mutated(texts, **{"contracts/generation-manifest/v1/schema.zt": "monotonicRead : Bool;"}), bindings)),
        ("no composition examined", lambda: judge_static({path: text for path, text in texts.items() if "compositions" not in path}, bindings)),
        ("right still defined", lambda: judge_static(_mutated(texts, **{RIGHTS_VOCABULARY: 'right "CLOCK_MONOTONIC_READ" 26 "clockMonotonicRead" "recorded";'}), bindings)),
        ("binding still names the right", lambda: judge_static(texts, _mutated(bindings, GENERATION_RIGHT_BY_MANIFEST_NAME={**rights, RETIRED_RIGHT_NAME: 1 << 26}))),
        ("binding reuses bit 26", lambda: judge_static(texts, _mutated(bindings, GENERATION_RIGHT_BY_MANIFEST_NAME={**rights, "renamed": 1 << 26}))),
        ("RIGHT_ALL admits bit 26", lambda: judge_static(texts, _mutated(bindings, GENERATION_RIGHT_ALL=int(bindings["GENERATION_RIGHT_ALL"]) | 1 << 26))),  # type: ignore[arg-type]
        ("binding exports the constant", lambda: judge_static(texts, _mutated(bindings, GENERATION_RIGHT_CLOCK_MONOTONIC_READ=1 << 26))),
        ("clock-authority format 1", lambda: judge_static(texts, _mutated(bindings, CLOCK_AUTHORITY_VERSION=1))),
        ("builder ignores the retired field", lambda: judge_builder(_ignoring_validate, _honest_manifest())),
        ("builder accepts everything", lambda: judge_builder(_accepting_validate, _honest_manifest())),
        ("builder refuses the honest table", lambda: judge_builder(_refusing_validate, _honest_manifest())),
    ]
    for name, case in cases:
        try:
            case()
        except JudgeError:
            continue
        raise JudgeError(f"control {name!r} was accepted")
    return len(cases)
