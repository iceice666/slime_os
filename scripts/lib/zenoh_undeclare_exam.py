"""Judge the Zenoh Profile 0 exam for a stock peer that ends its subscription, from what the Rust tests print.

A stock eclipse-zenoh 1.0.0 peer, listening, ends a session it subscribed in by sending an
UndeclareSubscriber that names the subscriber by id and nothing else: the DECLARE around it carries the
network QoS extension, and the undeclaration has no wire-expression extension because the codec leaves
that out when the expression is null. The stock-peer exam did not capture it, because the peer it
captured never undeclared. `PEER` is a whole session of that peer, captured once with a Profile 0
connector sending the demo's four samples, and its seventh batch is the undeclaration.

The Rust tests under `zenoh_undeclare::` print one `[zenoh-exam] undeclare ...` line for each thing they
did. This module owns what those lines must be, and nothing here reads the Rust:

* the eight batches the peer sent, replayed in order into a connector session;
* two scenarios: the whole session, and the bare form leaving the session as the full form does;
* the ten variants of the undeclaration the profile keeps refusing, each with the class it is refused as;
* the decoder's properties on each captured batch.

The undeclaration is also encoded here by `zenoh_wire`, the host reference, and must equal the captured
bytes, so the capture is not taken on trust from the peer that wrote it. Like the other judges, this one is
trusted only after it refuses a mutation of an honest transcript.
"""

from __future__ import annotations

import re
import shutil
import tempfile
from collections.abc import Callable
from pathlib import Path

import zenoh_exchange
import zenoh_wire
import zutai_cli
from harness import ROOT

SCHEMA = ROOT / "contracts" / "zenoh-stock-undeclare" / "v1" / "schema.zt"

# The batch bodies, without the two-byte stream length, a stock peer sent a Profile 0 connector: its
# INIT_ACK and OPEN_ACK, an OAM, the declaration of the key and then of the subscriber (through the alias),
# a KEEP_ALIVE, the undeclaration, and a CLOSE. The connector sent the demo's four samples between the
# subscriber's declaration and the keep-alive; the peer's session closed once it had received all four.
PEER = (
    ("undeclares-01-init-ack", "6109f1b587c059f7d5497d6ef8ecb74fdf0eb90a00022120d2f28ba089ba3c3ac361ef3760ff12344354a9ce9490efcf993db45c09c40f09"),
    ("undeclares-02-open-ack", "620afbccbf5a"),
    ("undeclares-03-oam", "25fbccbf5adf012108250203000210b587c059f7d5497d6ef8ecb74fdf0eb9020003010008a1a1a1a1a1a1a1a10200"),
    ("undeclares-04-declare-keyexpr", "25fcccbf5a9e21082001008101302f736c696d655f64656d6f2f636f756e7465722f736c696d655f64656d6f5f6d7367733a3a6d73673a3a6464735f3a3a436f756e7465725f2f5249485330315f61383266643566666362393664306131393761356164333638306431633465366261343361393632393238656364353932666235363565623831323935393562"),
    ("undeclares-05-declare-subscriber-declare-final", "25fdccbf5a9e21084201013e001a"),
    ("undeclares-06-keep-alive", "04"),
    ("undeclares-07-undeclare-subscriber", "25feccbf5a9e21080301"),
    ("undeclares-08-close", "0300"),
)
UNDECLARE = "undeclares-07-undeclare-subscriber"
DECLARE_SUBSCRIBER = "undeclares-05-declare-subscriber-declare-final"
SUBSCRIBER_ID = 1

OK_SCENARIOS = (
    "stock-undeclares-by-id",
    "bare-and-full-undeclare-agree",
)


class UndeclareError(Exception):
    pass


def _need(condition: bool, message: str) -> None:
    if not condition:
        raise UndeclareError(message)


def _sn_of(batch: bytes) -> int:
    _need(batch[0] == zenoh_wire.ID_FRAME | zenoh_wire.FLAG_R, "the captured undeclaration is not a reliable FRAME")
    sn, _at = zenoh_wire.read_leb128(batch, 1)
    return sn


def _frame_sn() -> int:
    """The sequence number of the captured undeclaration's FRAME."""
    return _sn_of(bytes.fromhex(dict(PEER)[UNDECLARE]))


def _qos_declare(*inner: bytes) -> bytes:
    """A DECLARE carrying the network QoS extension at the stock peer's default, around `inner`."""
    return bytes([zenoh_wire.ID_DECLARE | zenoh_wire.FLAG_Z, zenoh_wire.EXT_QOS_NETWORK, zenoh_wire.QOS_NETWORK_DEFAULT]) + b"".join(inner)


def _refusals() -> tuple[tuple[str, str, str], ...]:
    """(name, batch, class): the captured undeclaration with one named change, and the class it is refused as.

    All but the last two are refused by the decoder, before the session looks at them. The last two reach
    the session: an id it does not hold, and the captured undeclaration repeated.
    """
    sn = _frame_sn()
    key = zenoh_exchange.demo()["key"]
    full = zenoh_wire.undeclare_subscriber(SUBSCRIBER_ID, key, True)[1:]
    one = zenoh_wire.leb128(SUBSCRIBER_ID)
    u = bytes([zenoh_wire.ID_U_SUBSCRIBER])

    def frame(number: int, *messages: bytes) -> str:
        return zenoh_wire.frame(number, *messages).hex()

    return (
        ("refuse-undeclare-without-qos", frame(sn, bytes([zenoh_wire.ID_DECLARE]) + u + one), "malformed-message"),
        ("refuse-undeclare-full-form-with-qos", frame(sn, _qos_declare(full)), "unsupported-extension"),
        ("refuse-undeclare-reserved-bit-5", frame(sn, _qos_declare(bytes([zenoh_wire.ID_U_SUBSCRIBER | 0x20]) + one)), "malformed-message"),
        ("refuse-undeclare-reserved-bit-6", frame(sn, _qos_declare(bytes([zenoh_wire.ID_U_SUBSCRIBER | 0x40]) + one)), "malformed-message"),
        ("refuse-undeclare-without-id", frame(sn, _qos_declare(u)), "truncated"),
        ("refuse-undeclare-id-over-32-bits", frame(sn, _qos_declare(u + bytes.fromhex("ffffffff1f"))), "leb128-overflow"),
        ("refuse-undeclare-id-not-canonical", frame(sn, _qos_declare(u + bytes.fromhex("8100"))), "leb128-noncanonical"),
        ("refuse-undeclare-trailing-byte", frame(sn, _qos_declare(u + one), b"\x00"), "unsupported-message"),
        ("refuse-undeclare-unknown-id", frame(sn, _qos_declare(u + zenoh_wire.leb128(SUBSCRIBER_ID + 1))), "unknown-subscriber"),
        ("refuse-undeclare-repeated", frame(sn + 1, _qos_declare(u + one)), "unknown-subscriber"),
    )


REFUSED_SCENARIOS = tuple((name, cls) for name, _batch, cls in _refusals())


def refuse() -> tuple[tuple[str, str], ...]:
    return tuple((name, batch) for name, batch, _cls in _refusals())


def reference_agrees(
    peer: tuple[tuple[str, str], ...] = PEER,
    refused: tuple[tuple[str, str], ...] | None = None,
    build: Callable[[int], bytes] = zenoh_wire.undeclare_subscriber_by_id,
) -> None:
    """The host reference reproduces the captured undeclaration, and it undeclares the id the peer declared.

    The inputs are parameters so that `reference_controls` can corrupt each one in turn.
    """
    refused = refuse() if refused is None else refused
    captured = bytes.fromhex(dict(peer)[UNDECLARE])
    built = zenoh_wire.frame(_sn_of(captured), build(SUBSCRIBER_ID))
    _need(built == captured, f"the host reference builds {built.hex()}, the peer sent {captured.hex()}")
    declared = bytes.fromhex(dict(peer)[DECLARE_SUBSCRIBER])
    _need(declared.endswith(bytes.fromhex("4201013e001a")), "the captured subscriber declaration is not id 1 through alias 1")
    _need(len({name for name, _ in peer}) == len(peer), "a captured batch name is used twice")
    _need(len({name for name, _ in refused}) == len(refused), "a refused variant name is used twice")
    captured_set = {hexed for _name, hexed in peer}
    for name, batch in refused:
        _need(batch not in captured_set, f"{name} is a batch the peer sent")


def reference_controls() -> int:
    """Each input `reference_agrees` guards is corrupted in turn, and must be refused for its own reason."""
    reference_agrees()

    def declaring(old: str, new: str) -> tuple[tuple[str, str], ...]:
        return tuple((name, hexed.replace(old, new) if name == DECLARE_SUBSCRIBER else hexed) for name, hexed in PEER)

    honest = refuse()
    cases: list[tuple[str, dict, str]] = [
        ("a host reference that undeclares another subscriber id", {"build": lambda sub_id: zenoh_wire.undeclare_subscriber_by_id(sub_id + 1)}, "the host reference builds"),
        ("a captured subscriber that is id 2", {"peer": declaring("4201013e001a", "4202013e001a")}, "is not id 1 through alias 1"),
        ("a captured subscriber through alias 2", {"peer": declaring("4201013e001a", "4201023e001a")}, "is not id 1 through alias 1"),
        ("a captured batch name used twice", {"peer": (*PEER[:-1], (PEER[0][0], PEER[-1][1]))}, "a captured batch name is used twice"),
        ("a refused variant name used twice", {"refused": (honest[0], honest[0], *honest[2:])}, "a refused variant name is used twice"),
        ("a refused variant that is a batch the peer sent", {"refused": ((honest[0][0], PEER[-1][1]), *honest[1:])}, "is a batch the peer sent"),
    ]
    for label, changes, reason in cases:
        try:
            reference_agrees(**changes)
        except UndeclareError as error:
            _need(reason in str(error), f"control: {label} was refused for another reason: {error}")
            continue
        raise UndeclareError(f"control: the reference check accepted {label}")
    return len(cases)


def good_lines() -> list[str]:
    reference_agrees()
    return (
        [f"undeclare replay {name} ok" for name, _ in PEER]
        + [f"undeclare ok {name}" for name in OK_SCENARIOS]
        + [f"undeclare property {name} ok" for name, _ in PEER]
        + [f"undeclare refused {name} {cls}" for name, cls in REFUSED_SCENARIOS]
    )


def _exactly(label: str, wanted: tuple[str, ...], seen: list[str]) -> None:
    missing = [name for name in wanted if name not in seen]
    extra = [name for name in seen if name not in wanted]
    repeated = sorted({name for name in seen if seen.count(name) > 1})
    _need(not missing, f"undeclare {label} missing: {', '.join(missing[:4])}")
    _need(not extra, f"undeclare {label} not in the exam: {', '.join(extra[:4])}")
    _need(not repeated, f"undeclare {label} reported more than once: {', '.join(repeated[:4])}")


def judge(lines: list[str]) -> tuple[int, int]:
    """Check a transcript against the exam; returns (cases, refusals), a case being a scenario or a replay."""
    shapes = {("undeclare", "ok"): 3, ("undeclare", "replay"): 4, ("undeclare", "property"): 4, ("undeclare", "refused"): 4}
    split = [line.split() for line in lines]
    for line, fields in zip(lines, split, strict=True):
        _need(fields[:1] == ["undeclare"], f"undeclare: a record that is not an undeclare exam line: {line!r}")
        kind = tuple(fields[:2])
        _need(kind in shapes, f"undeclare: unrecognised line: {line}")
        _need(len(fields) == shapes[kind], f"undeclare: a {kind[1]} line has {len(fields)} fields, expected {shapes[kind]}: {line}")
        if kind in (("undeclare", "replay"), ("undeclare", "property")):
            _need(fields[3] == "ok", f"undeclare: a {kind[1]} line must end in ok: {line}")
    names = [name for name, _ in PEER]
    done = [f[2] for f in split if f[:2] == ["undeclare", "ok"]]
    replays = [f[2] for f in split if f[:2] == ["undeclare", "replay"]]
    properties = [f[2] for f in split if f[:2] == ["undeclare", "property"]]
    refused = [(f[2], f[3]) for f in split if f[:2] == ["undeclare", "refused"]]
    _exactly("scenarios", OK_SCENARIOS, done)
    _need(done == list(OK_SCENARIOS), f"undeclare scenarios are out of order: {done}, the exam runs them as {list(OK_SCENARIOS)}")
    _exactly("replays", tuple(names), replays)
    _need(replays == names, f"undeclare replays are out of order: {replays}, the peer sent {names}")
    _exactly("properties", tuple(names), properties)
    _need(properties == names, f"undeclare properties held for {properties}, the exam requires every captured batch in capture order")
    _exactly("refusals", tuple(name for name, _ in REFUSED_SCENARIOS), [name for name, _ in refused])
    wrong = [f"{name}={cls} (expected {dict(REFUSED_SCENARIOS)[name]})" for name, cls in refused if cls != dict(REFUSED_SCENARIOS)[name]]
    _need(not wrong, f"undeclare refusal classes differ: {', '.join(wrong)}")
    _need([name for name, _ in refused] == [name for name, _ in REFUSED_SCENARIOS], "undeclare refusals are out of order")
    return len(done) + len(replays), len(refused)


def transcript_mutations() -> list[tuple[str, list[str]]]:
    good = good_lines()
    names = [name for name, _ in PEER]
    first_replay = good.index(f"undeclare replay {names[0]} ok")
    at_undeclare = good.index(f"undeclare replay {UNDECLARE} ok")
    at_close = good.index(f"undeclare replay {names[-1]} ok")

    def without(line: str) -> list[str]:
        return [other for other in good if other != line]

    def swapped(first: int, second: int) -> list[str]:
        out = list(good)
        out[first], out[second] = out[second], out[first]
        return out

    out: list[tuple[str, list[str]]] = [
        ("an empty transcript", []),
        ("a line that is not an undeclare exam line", [*good, "stock ok stock-connects-open-and-deliver"]),
        ("an unrecognised kind of line", [*good, "undeclare delivered 1 00 00"]),
        ("a replay line with an extra field", [good[first_replay] + " extra", *good[first_replay + 1 :]]),
        ("a replay of a batch the peer did not send", [*good, "undeclare replay undeclares-09-extra ok"]),
        ("the undeclaration replayed twice", [*good, f"undeclare replay {UNDECLARE} ok"]),
        ("the first two replays swapped", swapped(first_replay, first_replay + 1)),
        ("the undeclaration and the close swapped", swapped(at_undeclare, at_close)),
        ("a replay that did not end in ok", [*good[:first_replay], good[first_replay].replace(" ok", " failed"), *good[first_replay + 1 :]]),
        ("the scenarios swapped", swapped(good.index(f"undeclare ok {OK_SCENARIOS[0]}"), good.index(f"undeclare ok {OK_SCENARIOS[1]}"))),
        ("a property that failed", [line.replace(f"property {UNDECLARE} ok", f"property {UNDECLARE} FAILED") for line in good]),
        ("a refusal reported twice", [*good, good[-1]]),
        ("a refusal reported with a class it does not have", [*good, "undeclare refused refuse-undeclare-elsewhere truncated"]),
    ]
    for name in names:
        out.append((f"the replay of {name} missing", without(f"undeclare replay {name} ok")))
    for name in OK_SCENARIOS:
        out.append((f"the scenario {name} missing", without(f"undeclare ok {name}")))
    out.append(("the undeclaration's property missing", without(f"undeclare property {UNDECLARE} ok")))
    classes = sorted({cls for _name, cls in REFUSED_SCENARIOS})
    for name, cls in REFUSED_SCENARIOS:
        out.append((f"{name} missing", without(f"undeclare refused {name} {cls}")))
        other = next(candidate for candidate in classes if candidate != cls)
        out.append((f"{name} refused as {other}", [line.replace(f"refused {name} {cls}", f"refused {name} {other}") for line in good]))
    return out


def transcript_controls() -> int:
    good = good_lines()
    cases, refusals = judge(good)
    _need((cases, refusals) == (len(OK_SCENARIOS) + len(PEER), len(REFUSED_SCENARIOS)), f"undeclare: the honest transcript is {cases} cases and {refusals} refusals")
    refused = 0
    for label, lines in transcript_mutations():
        if lines == good:
            raise UndeclareError(f"control: {label} did not change the transcript")
        try:
            judge(lines)
        except UndeclareError:
            refused += 1
            continue
        raise UndeclareError(f"control: the undeclare judge accepted {label}")
    return refused


# ---------------------------------------------------------------------------- the hand-off record

DECODED_ENTRY = re.compile(r'\s*\{ hex = "([^"\\]*)";\s+name = "([^"\\]*)" \}\s*')


def fixture_zti() -> str:
    """The captured batches and the refused variants as one `contracts/zenoh-stock-undeclare/v1` record."""

    def rows(group: tuple[tuple[str, str], ...]) -> str:
        return "".join(f'    {{ name = "{name}"; hex = "{hexed}"; }};\n' for name, hexed in group)

    return f"{{\n  formatVersion = 1;\n  peer = [\n{rows(PEER)}  ];\n  refuse = [\n{rows(refuse())}  ];\n}}\n"


def _decoded_rows(decoded: str, group: str) -> list[tuple[str, str]]:
    """The (name, hex) entries of one group in the decoder's own output, which spells every entry one way
    however the source wrote it. The whole group is consumed: an entry the reader cannot parse is refused."""
    start = re.search(rf"\b{group} = \[", decoded)
    _need(start is not None, f"undeclare hand-off: the decoded record has no {group} group")
    assert start is not None
    position, rows = start.end(), []
    if decoded[position : position + 1] == "]":
        return rows
    while True:
        entry = DECODED_ENTRY.match(decoded, position)
        _need(entry is not None, f"undeclare hand-off: {group} holds an entry the checker cannot read at offset {position}")
        assert entry is not None
        rows.append((entry.group(2), entry.group(1)))
        position = entry.end()
        if decoded[position : position + 1] == "]":
            return rows
        _need(decoded[position : position + 1] == ";", f"undeclare hand-off: {group} is not a list of entries at offset {position}")
        position += 1


def validate_handoff(text: str) -> None:
    """Decode a hand-off with the contract's own decoder, then hold the values the schema names."""
    (ROOT / "build").mkdir(exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="zenoh-undeclare-", dir=ROOT / "build"))
    try:
        shutil.copyfile(SCHEMA, scratch / "schema.zt")
        (scratch / "decode.zt").write_text(
            's ::= import "schema.zt"; env ::= import stdlib.env; load ::= import stdlib.load; '
            "main :: { env : Env; load : Load; } -> Validation DecodeIssue s.Session ! { * env.GetEffects; * load.ZtiEffects; } "
            '= caps => [ path := env.get caps.env "SLIME_ZENOH_UNDECLARE_FIXTURES_PATH" ?? ""; s.decodeSession (load.zti caps.load path) ]; main\n'
        )
        record = scratch / "fixtures.zti"
        record.write_text(text)
        try:
            decoded = zutai_cli.evaluate("run", scratch / "decode.zt", input_path=record, env_var="SLIME_ZENOH_UNDECLARE_FIXTURES_PATH")
        except zutai_cli.ZutaiError as error:
            raise UndeclareError(f"undeclare hand-off does not decode under contracts/zenoh-stock-undeclare/v1: {error}") from error
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    _need(decoded.startswith("#valid"), f"undeclare hand-off is refused by contracts/zenoh-stock-undeclare/v1: {decoded[:200]}")
    version = re.search(r"\bformatVersion = (\d+)", decoded)
    _need(version is not None and int(version.group(1)) == 1, "undeclare hand-off: formatVersion is not 1")
    for group, count in (("peer", 8), ("refuse", 10)):
        rows = _decoded_rows(decoded, group)
        _need(len(rows) == count, f"undeclare hand-off: {group} has {len(rows)} rows, the contract fixes {count}")
        for name, hexed in rows:
            _need(
                len(name.encode("utf-8")) <= 64 and len(hexed) % 2 == 0 and len(hexed) // 2 <= 512 and re.fullmatch(r"[0-9a-f]*", hexed) is not None,
                f"undeclare hand-off: {group}/{name} breaks the contract's bounds or is not lowercase hexadecimal",
            )


def handoff_controls() -> int:
    """The contract's decoder accepts the honest hand-off, spelled either way, and refuses each corrupted one."""
    honest = fixture_zti()
    last = f'    {{ name = "{PEER[-1][0]}"; hex = "{PEER[-1][1]}"; }};\n'
    respelled = honest.replace(last, f'    {{\n      name = "{PEER[-1][0]}";\n      hex = "{PEER[-1][1]}";\n    }};\n', 1).replace("formatVersion = 1;", "formatVersion =\n    1;", 1)
    _need(respelled != honest, "control: the respelled honest hand-off equals the original")
    validate_handoff(honest)
    validate_handoff(respelled)
    close_hex = PEER[-1][1]
    mutations = [
        ("a missing group", honest.replace("  refuse = [", "  refuze = [")),
        ("a wrong format version", honest.replace("formatVersion = 1;", "formatVersion = 2;")),
        ("a peer group of the wrong size", honest.replace(last, "", 1)),
        ("a refuse group of the wrong size", honest.replace(f'    {{ name = "{refuse()[0][0]}"; hex = "{refuse()[0][1]}"; }};\n', "", 1)),
        ("a name of 64 two-byte characters, 128 bytes", honest.replace(f'"{PEER[-1][0]}"', '"' + "\u00e9" * 64 + '"', 1)),
        ("a hex string with an odd length", honest.replace(f'hex = "{close_hex}"', f'hex = "{close_hex[:-1]}"', 1)),
        ("a hex string in capitals", honest.replace(f'hex = "{close_hex}"', 'hex = "03AB"', 1)),
        ("a batch of 513 bytes", honest.replace(f'hex = "{close_hex}"', 'hex = "' + "00" * 513 + '"', 1)),
        ("a row with no hex", honest.replace('hex = "', 'hxx = "', 1)),
        ("a hex that is not text", honest.replace('hex = "', 'hex = 7; zz = "', 1)),
        ("an unknown extra field", honest.replace("formatVersion = 1;", "formatVersion = 1;\n  surprise = 1;")),
        ("an unknown extra field on the same line as a known one", honest.replace("formatVersion = 1;", "formatVersion = 1; surprise = 1;", 1)),
        ("an unknown extra field on the opening line", honest.replace("{\n  formatVersion", "{ surprise = 1;\n  formatVersion", 1)),
        ("an unknown extra field inside an entry", honest.replace(last, f'    {{ name = "{PEER[-1][0]}"; hex = "{close_hex}"; extra = 1; }};\n', 1)),
        ("an unknown extra field written over several lines", honest.replace("formatVersion = 1;", "formatVersion = 1;\n  surprise =\n    1;", 1)),
        ("a wrong format version written over several lines", honest.replace("formatVersion = 1;", "formatVersion =\n    2;", 1)),
        ("a ninth peer entry written over several lines", honest.replace("  peer = [\n", '  peer = [\n    {\n      name = "extra";\n      hex = "00";\n    };\n', 1)),
        ("an eleventh refuse entry written over several lines", honest.replace("  refuse = [\n", '  refuse = [\n    {\n      name = "extra";\n      hex = "00";\n    };\n', 1)),
    ]
    for label, text in mutations:
        if text == honest:
            raise UndeclareError(f"control: {label} did not change the hand-off")
        try:
            validate_handoff(text)
        except UndeclareError:
            continue
        raise UndeclareError(f"control: {label} was accepted by the undeclare fixture contract")
    return len(mutations)


def all_controls() -> int:
    return transcript_controls() + handoff_controls() + reference_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. While the item is active its stored body is compared with these; once
# it is gone these still bind. Both counts are exact.
PINNED_CASES = 20
PINNED_CONTROLS = 68
ADMISSION = "slime-zenoh-profile0-stock-undeclare-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_undeclare_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        cases = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if cases and controls:
            return int(cases.group(1)), int(controls.group(1))
    return 0, 0
