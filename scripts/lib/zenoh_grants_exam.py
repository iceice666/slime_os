"""Judge `contracts/zenoh-service-grants/v1`: what the Zenoh service may do for each holder.

The contract is judged by its own decoder, the way `zenoh_links_exam` judges the link limits: a record
is written as Zutai text, handed to the schema's `decodeGrants`, and must come out valid or invalid as
this module says. The honest record is also what the service core's exam (`zenoh_service_exam`) is
declared with, so the two cannot disagree about a grant.

The contract sets no holder, key or slot count of its own. Each case here is a declaration the
arithmetic accepts or refuses, and each boundary is held on both sides.
"""

from __future__ import annotations

import re
import shutil
import tempfile
from pathlib import Path

import zenoh_exchange
import zutai_cli
from harness import ROOT

SCHEMA = ROOT / "contracts" / "zenoh-service-grants" / "v1" / "schema.zt"
GENERATED = ROOT / "components" / "proto" / "src" / "zenoh_service_grants.rs"

# The keys the exam names k1 to k4. The first is the demo's, 129 bytes.
KEYS = (
    zenoh_exchange.demo()["key"],
    "0/slime_demo/extra/second",
    "0/slime_demo/extra/third",
    "0/slime_demo/extra/fourth",
)
HOLDERS = ("a", "b", "c")

# (holder, key index, publish, subscribe): the rows the service core is declared with.
ROWS = (
    (0, 0, True, True),
    (0, 1, False, True),
    (1, 0, False, True),
    (1, 2, True, False),
    (2, 1, False, True),
)
# holders, ring slots, sample bytes, key bytes, local subscriptions: the limits the core is declared with.
LIMITS = (3, 4, 12, 129, 3)

# The arithmetic of the contract, written again here from its comments so a change to either shows.
SLOT_OVERHEAD, ROW_OVERHEAD, HOLDER_OVERHEAD, SERVICE_FIXED = 32, 24, 64, 1024


class GrantsError(Exception):
    pass


def required_stack(holders: int, slots: int, sample: int, key: int, rows: int) -> int:
    return SERVICE_FIXED + holders * HOLDER_OVERHEAD + holders * slots * (sample + SLOT_OVERHEAD) + rows * (key + ROW_OVERHEAD)


def record(
    *,
    version: int = 1,
    holders: int = LIMITS[0],
    slots: int = LIMITS[1],
    sample: int = LIMITS[2],
    key_bytes: int = LIMITS[3],
    local: int = LIMITS[4],
    stack: int | None = None,
    rows: list[tuple[str, str, object, object]] | None = None,
    top: str = "",
    limit_extra: str = "",
    row_extra: str = "",
    drop_limit: str = "",
) -> str:
    rows = rows if rows is not None else [(HOLDERS[h], KEYS[k], p, s) for h, k, p, s in ROWS]
    if stack is None:
        stack = required_stack(holders, slots, sample, key_bytes, len(rows))

    def flag(value: object) -> str:
        return str(value).lower() if isinstance(value, bool) else f'"{value}"'

    limit_fields = {"holders": holders, "ringSlots": slots, "sampleBytes": sample, "keyBytes": key_bytes, "localSubscriptions": local, "stackBytes": stack}
    limit_fields.pop(drop_limit, None)
    limit_text = " ".join(f"{name} = {value};" for name, value in limit_fields.items()) + limit_extra
    row_text = "".join(
        f'    {{ holder = "{h}"; key = "{k}"; publish = {flag(p)}; subscribe = {flag(s)};{row_extra} }};\n' for h, k, p, s in rows
    )
    return f"{{\n  formatVersion = {version};{top}\n  limits = {{ {limit_text} }};\n  grants = [\n{row_text}  ];\n}}\n"


def contract_accepts(text: str, schema: str | None = None) -> bool:
    """Whether the contract's decoder takes `text` as a valid grants record, under `schema` if one is given."""
    (ROOT / "build").mkdir(exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="zenoh-grants-", dir=ROOT / "build"))
    try:
        if schema is None:
            shutil.copyfile(SCHEMA, scratch / "schema.zt")
        else:
            (scratch / "schema.zt").write_text(schema, encoding="utf-8")
        (scratch / "decode.zt").write_text(
            's ::= import "schema.zt"; env ::= import stdlib.env; load ::= import stdlib.load; '
            "main :: { env : Env; load : Load; } -> Validation DecodeIssue s.Grants ! { * env.GetEffects; * load.ZtiEffects; } "
            '= caps => [ path := env.get caps.env "SLIME_GRANTS_PATH" ?? ""; s.decodeGrants (load.zti caps.load path) ]; main\n'
        )
        path = scratch / "grants.zti"
        path.write_text(text)
        try:
            out = zutai_cli.evaluate("run", scratch / "decode.zt", input_path=path, env_var="SLIME_GRANTS_PATH")
        except zutai_cli.ZutaiError as error:
            raise GrantsError(f"the grants contract could not decode a record: {error}") from error
        return out.startswith("#valid")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def cases() -> list[tuple[str, str, bool]]:
    """(label, record, whether the contract accepts it). Each limit is held on both sides of its bound."""
    row = lambda h, k, p, s: (h, k, p, s)  # noqa: E731
    base = [(HOLDERS[h], KEYS[k], p, s) for h, k, p, s in ROWS]
    need = required_stack(*LIMITS[:4], len(base))
    out: list[tuple[str, str, bool]] = [
        ("the honest record", record(), True),
        ("a stack of exactly what the declaration needs", record(stack=need), True),
        ("a stack of one byte more than it needs", record(stack=need + 1), True),
        ("more holders declared than named", record(holders=4), True),
        ("more local subscriptions than subscribe keys", record(local=4), True),
        ("a key of exactly the declared key bytes", record(key_bytes=len(KEYS[0])), True),
        ("one row that publishes and subscribes", record(rows=[row("a", KEYS[0], True, True)]), True),
        ("a row that only publishes", record(rows=[row("a", KEYS[0], True, False)]), True),
        ("a holder name of 32 characters", record(rows=[row("a" * 32, KEYS[0], True, False)]), True),
        ("a holder named with hyphen, underscore and digits", record(rows=[row("node_1-b", KEYS[0], False, True)]), True),
        ("a key of an rmw_zenoh shape", record(rows=[row("a", "0/chatter/std_msgs::msg::dds_::String_/RIHS01_abcdef0123", True, True)]), True),
        ("a stack of one byte under what the declaration needs", record(stack=need - 1), False),
        ("format version 2", record(version=2), False),
        ("format version 0", record(version=0), False),
        ("no rows", record(rows=[]), False),
        ("zero holders", record(holders=0), False),
        ("65535 holders with the stack they need", record(holders=65535), True),
        ("65535 holders with the stack of three", record(holders=65535, stack=need), False),
        ("65536 holders", record(holders=65536), False),
        ("zero ring slots", record(slots=0), False),
        ("65536 ring slots", record(slots=65536), False),
        ("a zero-byte sample", record(sample=0), False),
        ("a sample one over 1048576 bytes", record(sample=1048577), False),
        ("a zero-byte key bound", record(key_bytes=0), False),
        ("a key bound over 65535", record(key_bytes=65536), False),
        ("zero local subscriptions", record(local=0), False),
        ("65536 local subscriptions", record(local=65536), False),
        ("a key one byte over the declared key bytes", record(key_bytes=len(KEYS[0]) - 1), False),
        ("a row with neither right", record(rows=[row("a", KEYS[0], False, False)]), False),
        ("the same holder and key twice", record(rows=[row("a", KEYS[0], True, False), row("a", KEYS[0], False, True)]), False),
        ("an empty key", record(rows=[row("a", "", True, False)]), False),
        ("a key with a star", record(rows=[row("a", "0/slime_demo/*", True, False)]), False),
        ("a key with a double star", record(rows=[row("a", "0/slime_demo/**", True, False)]), False),
        ("a key with a dollar", record(rows=[row("a", "0/slime_demo/$*", True, False)]), False),
        ("a key with a question mark", record(rows=[row("a", "0/slime_demo/a?b", True, False)]), False),
        ("a key with a hash", record(rows=[row("a", "0/slime_demo/a#b", True, False)]), False),
        ("a key with a leading slash", record(rows=[row("a", "/0/slime_demo", True, False)]), False),
        ("a key with a trailing slash", record(rows=[row("a", "0/slime_demo/", True, False)]), False),
        ("a key with an empty segment", record(rows=[row("a", "0//slime_demo", True, False)]), False),
        ("a key with a space", record(rows=[row("a", "0/slime demo", True, False)]), False),
        ("a key with a character outside ASCII", record(rows=[row("a", "0/slime_demo/caf\u00e9", True, False)]), False),
        ("an empty holder name", record(rows=[row("", KEYS[0], True, False)]), False),
        ("a holder name of 33 characters", record(rows=[row("a" * 33, KEYS[0], True, False)]), False),
        ("a holder name in capitals", record(rows=[row("Node", KEYS[0], True, False)]), False),
        ("a holder name with a space", record(rows=[row("no de", KEYS[0], True, False)]), False),
        ("a holder name with a dot", record(rows=[row("no.de", KEYS[0], True, False)]), False),
        ("four holders named with three declared", record(rows=[row("a", KEYS[0], True, False), row("b", KEYS[0], True, False), row("c", KEYS[0], True, False), row("d", KEYS[0], True, False)]), False),
        ("four subscribe keys with three local subscriptions", record(rows=[row("a", KEYS[0], False, True), row("a", KEYS[1], False, True), row("b", KEYS[2], False, True), row("c", KEYS[3], False, True)]), False),
        ("an unknown field at the top", record(top="\n  surprise = 1;"), False),
        ("an unknown field in the limits", record(limit_extra=" surprise = 1;"), False),
        ("an unknown field in a row", record(row_extra=" surprise = 1;"), False),
        ("a limits record with no stack", record(drop_limit="stackBytes"), False),
        ("a limits record with no holders", record(drop_limit="holders"), False),
        ("a right written as text", record(rows=[row("a", KEYS[0], "yes", False)]), False),
    ]
    # Four subscribe keys fit once the session can hold four of its own, so the bound is held both ways.
    out.append(("four subscribe keys with four local subscriptions", record(local=4, rows=[row("a", KEYS[0], False, True), row("a", KEYS[1], False, True), row("b", KEYS[2], False, True), row("c", KEYS[3], False, True)]), True))
    return out


# A contract with one of its checks removed. Each is run on the record that check exists to refuse, and
# the decoder must then take it: a judge that cannot tell the weakened contract from the real one proves
# nothing about either. (label, text removed, text put in its place, the case that must flip).
SCHEMA_MUTATIONS = (
    ("a contract that allows a holder and key twice", "&& pairsDistinct rec.grants", "&& true", "the same holder and key twice"),
    ("a contract that does not count holders", "nameCount (holderNames rec.grants (l.empty ())) <= rec.limits.holders", "true", "four holders named with three declared"),
    ("a contract that does not bound subscribe keys by local subscriptions", "nameCount (subscribeKeys rec.grants (l.empty ())) <= rec.limits.localSubscriptions", "true", "four subscribe keys with three local subscriptions"),
    ("a contract that does not check the stack", "lim.stackBytes >= requiredStack lim rows", "lim.stackBytes >= 0", "a stack of one byte under what the declaration needs"),
    ("a contract that reads a field it does not name", "if closedGrants data then", "if true then", "an unknown field at the top"),
    ("a contract that allows wildcards in a key", "t.length (withoutChars keyChars key) == 0", "true", "a key with a star"),
    ("a contract that allows an empty key segment", "segmentsNonEmpty (t.split \"/\" key)", "true", "a key with an empty segment"),
    ("a contract that allows a row with no right", "(g.publish || g.subscribe)", "true", "a row with neither right"),
    ("a contract that allows any holder name", "t.length (withoutChars nameChars name) == 0", "true", "a holder name in capitals"),
    ("a contract that ignores the format version", "rec.formatVersion == formatVersion", "true", "format version 2"),
)


def mutation_controls() -> int:
    """Each weakened contract takes the record its removed check refuses, where the real one does not."""
    text = SCHEMA.read_text(encoding="utf-8")
    by_label = {label: (record_text, wanted) for label, record_text, wanted in cases()}
    caught = 0
    for label, old, new, case in SCHEMA_MUTATIONS:
        if text.count(old) != 1:
            raise GrantsError(f"control: {label}: the text it removes appears {text.count(old)} times in the schema, not once")
        record_text, wanted = by_label[case]
        if wanted:
            raise GrantsError(f"control: {label}: the case {case!r} is one the contract accepts")
        if contract_accepts(record_text, text.replace(old, new)) == wanted:
            raise GrantsError(f"control: {label} was not seen: the weakened contract still refused {case!r}")
        caught += 1
    return caught


def control_counts() -> tuple[int, int]:
    """(records the contract must accept and does, records it must refuse and does)."""
    accepted = refused = 0
    for label, text, wanted in cases():
        if contract_accepts(text) != wanted:
            raise GrantsError(f"control: the grants contract {'refused' if wanted else 'accepted'}: {label}")
        accepted, refused = accepted + wanted, refused + (not wanted)
    return accepted, refused


# What the contract's source states, held here and in the generated Rust constants.
CONSTANTS = {
    "FORMAT_VERSION": "formatVersion",
    "MAX_KEY_BYTES": "maxKeyBytes",
    "MAX_COUNT": "maxCount",
    "MAX_SAMPLE_BYTES": "maxSampleBytes",
    "MAX_HOLDER_NAME_BYTES": "maxHolderNameBytes",
    "SLOT_OVERHEAD_BYTES": "slotOverheadBytes",
    "ROW_OVERHEAD_BYTES": "rowOverheadBytes",
    "HOLDER_OVERHEAD_BYTES": "holderOverheadBytes",
    "SERVICE_FIXED_BYTES": "serviceFixedBytes",
}


def schema_constants() -> dict[str, int]:
    text = SCHEMA.read_text(encoding="utf-8")
    found = {}
    for rust, zutai in CONSTANTS.items():
        match = re.search(rf"^{zutai} :: Int = (\d+);", text, re.M)
        if match is None:
            raise GrantsError(f"the schema does not state {zutai}")
        found[rust] = int(match.group(1))
    return found


def check_generated() -> int:
    """The Rust constants the implementation generates equal the schema's, or the implementation has not landed."""
    if not GENERATED.is_file():
        raise GrantsError(f"{GENERATED.relative_to(ROOT)} does not exist; the grants bindings have not landed")
    text = GENERATED.read_text(encoding="utf-8")
    wanted = schema_constants()
    for rust, value in wanted.items():
        match = re.search(rf"pub const {rust}: [a-z0-9]+ = (\d+);", text)
        if match is None or int(match.group(1)) != value:
            raise GrantsError(f"{GENERATED.relative_to(ROOT)} has {rust} = {match.group(1) if match else 'nothing'}, the schema states {value}")
    return len(wanted)


def arithmetic_controls() -> int:
    """The Python restatement of the stack arithmetic and the schema's constants agree, and a drift in either is caught."""
    constants = schema_constants()
    refused = 0
    for rust, expected in (("SLOT_OVERHEAD_BYTES", SLOT_OVERHEAD), ("ROW_OVERHEAD_BYTES", ROW_OVERHEAD), ("HOLDER_OVERHEAD_BYTES", HOLDER_OVERHEAD), ("SERVICE_FIXED_BYTES", SERVICE_FIXED)):
        if constants[rust] != expected:
            raise GrantsError(f"the schema states {rust} = {constants[rust]}, the exam restates {expected}")
    for label, drift in (("a constant that drifts", {"SLOT_OVERHEAD_BYTES": SLOT_OVERHEAD + 1}), ("a second constant that drifts", {"SERVICE_FIXED_BYTES": SERVICE_FIXED - 1})):
        changed = {**constants, **drift}
        if all(changed[name] == value for name, value in (("SLOT_OVERHEAD_BYTES", SLOT_OVERHEAD), ("ROW_OVERHEAD_BYTES", ROW_OVERHEAD), ("HOLDER_OVERHEAD_BYTES", HOLDER_OVERHEAD), ("SERVICE_FIXED_BYTES", SERVICE_FIXED))):
            raise GrantsError(f"control: {label} was not seen")
        refused += 1
    return refused


def judge_cases() -> int:
    """The records judged against the contract and the constants checked against the generated Rust."""
    accepted, refused = control_counts()
    return accepted + refused + len(CONSTANTS)


def all_controls() -> int:
    """Every refused record, every weakened contract and every drifted constant: the corruptions refused."""
    _accepted, refused = control_counts()
    return refused + mutation_controls() + arithmetic_controls()


# What the admitted item fixes, held here as well so the gate stays correct after the item is completed
# and retired to .tasks/terminal. While the item is active its stored body is compared with these.
PINNED_CASES = 64
PINNED_CONTROLS = 54
ADMISSION = "slime-zenoh-service-grants-"


def item_pins() -> tuple[int, int]:
    """The case count and the control count the admitted item fixes, read from its stored body, or (0, 0)."""
    for path in sorted((ROOT / ".tasks" / "items").glob("*.md")):
        text = path.read_text(encoding="utf-8")
        if "zenoh_grants_check" not in text or ADMISSION not in text or re.search(r"^state: cancelled$", text, re.M):
            continue
        found = re.search(r'id = "all-cases-observed";.*?intValue = (\d+);', text, re.S)
        controls = re.search(r'id = "exactly-\d+-controls";.*?observation = "negativeControlsRefused";.*?intValue = (\d+);', text, re.S)
        if found and controls:
            return int(found.group(1)), int(controls.group(1))
    return 0, 0
