"""Record-integrity controls and a narrow root-writer qualification audit.

Synthetic controls judge validators, not a boot or a Rust semantic proof. The
source audit deliberately qualifies only the declared single-core, non-MCS ARM
root routes: it is a lexical refusal boundary, not a general output-flow proof.
"""

from __future__ import annotations

import re
import tempfile
import tomllib
from collections.abc import Callable
from pathlib import Path
from typing import NoReturn

from sel4_gate_markers import match_marker_contract

_PROBE = "[io-local-network-probe]"
_SERVICE_PREFIXES = ("[network-service]",)
# Frozen sel4-io-local declarations: one publisher destination, two loopback
# applications with default socket options, two clean session shutdowns.
# These values are not computed from product source or a captured transcript.
_ADDITIONAL_LOCAL = (
    (r"\[network-service\] authority destinations=1 rights=connect,send,recv", 1),
    (r"\[network-service\] declared socket_limit=1 listener_limit=0 dns_record_limit=0", 1),
    (r"\[network-service\] incarnation=1", 1),
    (
        r"\[network-service\] tcp options holder=unnamed keepalive_ms=0 nagle=1 hop_limit=64 idle_timeout_ms=10000",
        2,
    ),
    (r"\[network-service\] tcp congestion control=reno", 1),
    (
        r"\[network-service\] tcp accepted close unread=0 unsent=0 terminal=closed handles=1 bytes=4096",
        1,
    ),
    (r"\[network-service\] tcp listener closed children=0", 1),
    (r"\[network-service\] application bytes sent=4096 received=2048", 1),
    (r"\[network-service\] application bytes sent=2048 received=4096", 1),
    (r"\[network-service\] application buffers released", 2),
    (
        r"\[network-service\] observed requests=2 packets=0 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=0",
        1,
    ),
)


def additional_local_patterns() -> tuple[tuple[str, int], ...]:
    """Independent normal-path families and exact composition multiplicities."""
    return _ADDITIONAL_LOCAL


def local_control_transcript(gate: object, literal_for: Callable[[str], str]) -> str:
    """Synthetic positive records in service order, without cross-node ordering."""
    lines = [literal_for(pattern) for _, chain in gate.LOCAL_CHAINS for pattern in chain]
    startup = [
        literal_for(pattern) for pattern, count in _ADDITIONAL_LOCAL[:5] for _ in range(count)
    ]
    lines[1:1] = startup
    wait = next(
        index for index, line in enumerate(lines) if line.startswith("[network-service] wait ")
    )
    # Accepted-close/listener-close are service events before session teardown;
    # each byte report immediately precedes its own buffer-release report.
    tail = [literal_for(_ADDITIONAL_LOCAL[index][0]) for index in (5, 6, 7, 9, 8, 9)]
    lines[wait:wait] = tail
    frames = next(
        index
        for index, line in enumerate(lines)
        if line.startswith("[network-service] loopback frames=")
    )
    lines.insert(frames + 1, literal_for(_ADDITIONAL_LOCAL[-1][0]))
    return "\n".join(lines) + "\n"


_DEBUG_WRITE_CONTRACT = (
    "Each call submits one bounded payload to the console dispatcher.",
    "Callers must assemble a complete record before submission; fragments are not joined.",
    "The root and console dispatcher share record serialization on the qualified single-core non-MCS path.",
    "Send completion does not acknowledge emission.",
)
_HEALTH = "SLIME_GRAPH HEALTHY"
_RAW_OUTPUT = re.compile(
    r"\b(?:debug_print|debug_println|debug_print_helper|debug_put_char|DebugWrite|DebugPutChar|"
    r"seL4_DebugPutChar|seL4_DebugWrite)\b"
)


def _reject(message: str) -> NoReturn:
    raise ValueError(f"console record qualification: {message}")


def validate_local(transcript: str, gate: object) -> None:
    """Preserve producer order, then require whole records at pinned multiplicities.

    Admission remains a prefix contract because it carries additional boot
    fields. Unrelated boot records are not reinterpreted as local evidence.
    Every occurrence of a qualified local prefix, including broken extras next
    to an intact copy, is inspected instead of searching only for good copies.
    """
    chains = gate.LOCAL_CHAINS
    match_marker_contract(transcript, chains, gate.FAILURE_MARKERS, _reject)
    for label, chain in chains:
        if label == "local admission":
            for pattern in chain:
                if len(re.findall(pattern, transcript)) != 1:
                    _reject("local admission prefix must occur exactly once")
    original = tuple(
        pattern for label, chain in chains if label != "local admission" for pattern in chain
    )
    requirements = tuple((pattern, 1) for pattern in original) + _ADDITIONAL_LOCAL
    patterns = tuple(pattern for pattern, _ in requirements)
    counts = [0] * len(patterns)
    for line in transcript.split("\n"):
        # Stable producer stems make incomplete-prefix extras observable too.
        # Arbitrary bytes split before any recognizable stem are not attributed
        # to a producer; the source fixture independently judges whole writes.
        if "[io-local" in line and _PROBE not in line:
            _reject(f"fragmented local probe prefix: {line!r}")
        if "[network-service" in line and "[network-service]" not in line:
            _reject(f"fragmented local service prefix: {line!r}")
        if "SLIME_GRAPH HEALTH" in line and _HEALTH not in line:
            _reject(f"fragmented local health prefix: {line!r}")
        prefixes = (_PROBE, *_SERVICE_PREFIXES, _HEALTH)
        present = [prefix for prefix in prefixes if prefix in line]
        if not present:
            continue
        if len(present) != 1 or not line.startswith(present[0]):
            _reject(f"fragmented or prefixed local record: {line!r}")
        matches = [
            index
            for index, pattern in enumerate(patterns)
            if re.fullmatch(pattern, line) is not None
        ]
        if len(matches) != 1:
            _reject(f"malformed or unqualified local record: {line!r}")
        for value in re.findall(r"=([0-9]+)(?: |$)", line):
            if len(value) > 20 or int(value) > (1 << 64) - 1:
                _reject(f"local numeric field exceeds bounded u64 producer: {line!r}")
        counts[matches[0]] += 1
    for (pattern, expected), count in zip(requirements, counts, strict=True):
        if count != expected:
            _reject(f"local record requires exactly {expected} occurrences, got {count}: {pattern}")
    # Only service-local causal obligations are added; peer interleaving is free.
    records = transcript.split("\n")
    positions = {
        pattern: [index for index, line in enumerate(records) if re.fullmatch(pattern, line)]
        for pattern in patterns
    }
    startup = [pattern for pattern, _ in _ADDITIONAL_LOCAL[:5]] + [original[0]]
    shutdown = [original[1], original[2], _ADDITIONAL_LOCAL[-1][0]]
    for order in (startup, shutdown):
        for before, after in zip(order, order[1:], strict=False):
            if max(positions[before]) >= min(positions[after]):
                _reject("local service records violate causal producer order")
    before_cleanup = positions[original[1]][0]
    after_startup = positions[original[0]][0]
    for pattern, _ in _ADDITIONAL_LOCAL[5:10]:
        if not all(after_startup < position < before_cleanup for position in positions[pattern]):
            _reject("local session records must be between interface and cleanup")
    if positions[_ADDITIONAL_LOCAL[5][0]][0] >= positions[_ADDITIONAL_LOCAL[6][0]][0]:
        _reject("local accepted close must precede its listener close")
    if positions[_ADDITIONAL_LOCAL[6][0]][0] >= positions[_ADDITIONAL_LOCAL[8][0]][0]:
        _reject("local subscriber byte report must follow its listener close")
    released = positions[_ADDITIONAL_LOCAL[9][0]]
    sent = sorted(positions[_ADDITIONAL_LOCAL[7][0]] + positions[_ADDITIONAL_LOCAL[8][0]])
    if not sent[0] < released[0] < sent[1] < released[1]:
        _reject("each local session byte report must precede its buffer release")


def _rust_code(text: str) -> str:
    """Mask comments and literals; retain code tokens and line locations.

    This handles nested Rust block comments, raw/byte strings, quoted strings,
    and character literals while retaining lifetime tokens. It is intentionally
    not a parser or an expansion of macros, aliases, includes, or cfg branches.
    """
    output: list[str] = []
    cursor = 0
    while cursor < len(text):
        start = cursor
        if text.startswith("//", cursor):
            end = text.find("\n", cursor)
            cursor = len(text) if end < 0 else end
        elif text.startswith("/*", cursor):
            cursor += 2
            depth = 1
            while cursor < len(text) and depth:
                if text.startswith("/*", cursor):
                    depth += 1
                    cursor += 2
                elif text.startswith("*/", cursor):
                    depth -= 1
                    cursor += 2
                else:
                    cursor += 1
            if depth:
                _reject("unterminated Rust block comment")
        else:
            raw = re.match(r'(?:br|cr|r)(#*)"', text[cursor:])
            quoted = re.match(r'(?:b|c)?"', text[cursor:])
            character = re.match(
                r"(?:b)?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'", text[cursor:]
            )
            if raw:
                terminator = '"' + raw.group(1)
                end = text.find(terminator, cursor + raw.end())
                if end < 0:
                    _reject("unterminated Rust raw string")
                cursor = end + len(terminator)
            elif quoted:
                cursor += quoted.end()
                while cursor < len(text):
                    char = text[cursor]
                    cursor += 1
                    if char == "\\":
                        cursor += 1
                    elif char == '"':
                        break
                else:
                    _reject("unterminated Rust string")
            elif character:
                cursor += character.end()
            else:
                output.append(text[cursor])
                cursor += 1
                continue
        output.append("".join("\n" if char == "\n" else " " for char in text[start:cursor]))
    return "".join(output)


def _read(root: Path, relative: str) -> str:
    path = root / relative
    if not path.is_file():
        _reject(f"required qualification source missing: {relative}")
    return path.read_text(encoding="utf-8")


def _require(code: str, pattern: str, description: str) -> None:
    if re.search(pattern, code) is None:
        _reject(description)


def audit_routes(root: Path) -> None:
    """Fail closed unless the fixed qualification target uses the shared sink.

    The runtime helper separately compiles the exact module and exercises its
    lock/record semantics. This audit checks source tokens, target configuration,
    and the two existing root-writer routes; it does not claim arbitrary Rust
    data-flow analysis, component-output qualification, or SMP/MCS support.
    """
    library = _rust_code(_read(root, "slime-root/src/lib.rs"))
    binary = _rust_code(_read(root, "slime-root/src/main.rs"))
    diagnostic = _rust_code(_read(root, "slime-root/src/diagnostic.rs"))
    console = _rust_code(_read(root, "slime-root/src/console.rs"))
    _require(
        library,
        r"\bpub\s+mod\s+diagnostic\s*;",
        "library must declare the shared diagnostic module",
    )
    if re.search(r"\bmod\s+diagnostic\b", binary):
        _reject("binary must not instantiate a second diagnostic module/lock")
    _require(
        binary,
        r"\bslime_root\s*::\s*(?:diagnostic(?:_print(?:ln)?)?\b|\{[^;]*\bdiagnostic(?:_print(?:ln)?)?\b)",
        "binary must use the library diagnostic route",
    )
    _require(
        console,
        r"\bcrate\s*::\s*diagnostic\s*::\s*write\s*\(\s*bytes\s*\)",
        "console payload must use the shared diagnostic writer",
    )
    _require(diagnostic, r"\bpub\s+fn\s+write\s*\(", "diagnostic module must expose write")
    _require(diagnostic, r"\bpub\s+fn\s+print\s*\(", "diagnostic module must expose print")
    _require(
        diagnostic,
        r"\bsel4\s*::\s*r#yield\s*\(\s*\)",
        "diagnostic contention must reach the native kernel yield",
    )
    _require(
        diagnostic,
        r"\bsel4\s*::\s*debug_put_char\s*\(",
        "diagnostic output must reach the native kernel byte sink",
    )
    if re.search(
        r"\b(?:debug_print|debug_println|debug_print_helper|DebugWrite|DebugPutChar|seL4_DebugPutChar|seL4_DebugWrite)\b",
        diagnostic,
    ):
        _reject(
            "diagnostic module must emit through its native byte sink, not a raw formatter or syscall alternative"
        )
    if re.search(r"\b(?:cfg|cfg_attr|std|test)\b", diagnostic):
        _reject(
            "diagnostic module must not replace native output/yield through cfg, test, or std branches"
        )
    if re.search(r"\b(?:include|include_str|include_bytes)\s*!", diagnostic):
        _reject("diagnostic native implementation must be in the audited source")
    source_root = root / "slime-root/src"
    for path in sorted(source_root.rglob("*.rs")):
        if path == source_root / "diagnostic.rs":
            continue
        code = _rust_code(path.read_text(encoding="utf-8"))
        match = _RAW_OUTPUT.search(code)
        if match is not None:
            _reject(f"raw root output bypass in {path.relative_to(root)}: {match.group(0)}")

    transport = _read(root, "components/runtime/src/syscall/sel4_transport.rs")
    documentation = re.search(
        r"((?:^///[^\n]*\n)+)pub fn debug_write\s*\(", transport, re.MULTILINE
    )
    if documentation is None:
        _reject("debug_write must retain its frozen public record contract")
    contract = " ".join(
        line.removeprefix("///").strip() for line in documentation.group(1).splitlines()
    )
    contract = " ".join(contract.split())
    for clause in _DEBUG_WRITE_CONTRACT:
        if clause not in contract:
            _reject(f"debug_write public record contract missing clause: {clause}")
    if any(
        phrase in contract
        for phrase in ("single-threaded", "atomicity structural", "DebugWrite on ARM")
    ):
        _reject("debug_write public record contract retains obsolete serialization rationale")

    scheduler = _rust_code(_read(root, "slime-root/src/graph_runtime/console_runtime.rs"))
    priority_calls = re.findall(r"\btcb_set_sched_params\s*\(([^;]*)\)", scheduler)
    expected = re.compile(
        r"\s*sel4\s*::\s*init_thread\s*::\s*slot\s*::\s*TCB\s*\.\s*cap\s*\(\s*\)\s*,\s*255\s*,\s*255\s*"
    )
    if len(priority_calls) != 1 or expected.fullmatch(priority_calls[0]) is None:
        _reject("console dispatcher must have exactly the pinned 255/255 scheduling call")
    config = re.sub(r"#[^\n]*", "", _read(root, "sel4/config/qemu-arm-virt.cmake"))
    for key, value, kind in (
        ("KernelPlatform", '"qemu-arm-virt"', "STRING"),
        ("KernelIsMCS", "OFF", "BOOL"),
        ("KernelMaxNumNodes", "1", "STRING"),
    ):
        declarations = re.findall(rf"\bset\s*\(\s*{key}\s+([^)]*)\)", config)
        if (
            len(declarations) != 1
            or re.fullmatch(
                rf'{re.escape(value)}\s+CACHE\s+{kind}\s+""\s*(?:FORCE\s*)?', declarations[0]
            )
            is None
        ):
            _reject(f"qemu-arm-virt requires exact {key}={value}")
    try:
        pins = tomllib.loads(_read(root, "sel4/pins.toml"))
    except tomllib.TOMLDecodeError as error:
        _reject(f"invalid qualification pins: {error}")
    if pins.get("qemu_arm_virt", {}).get("cpus") != 1:
        _reject("qemu-arm-virt pins must request one CPU")
    kernel = _rust_code(_read(root, "deps/sel4/src/kernel/boot.c"))
    priorities = re.findall(r"\btcb\s*->\s*tcbPriority\s*=\s*([^;]+);", kernel)
    if priorities != ["seL4_MaxPrio"]:
        _reject("kernel boot root priority must be exactly seL4_MaxPrio")
    constants = _rust_code(_read(root, "deps/sel4/libsel4/include/sel4/constants.h"))
    _require(
        constants,
        r"\bseL4_MaxPrio\s*=\s*CONFIG_NUM_PRIORITIES\s*-\s*1\b",
        "kernel maximum priority must derive from CONFIG_NUM_PRIORITIES - 1",
    )
    kernel_config = re.sub(r"#[^\n]*", "", _read(root, "deps/sel4/config.cmake"))
    defaults = re.findall(
        r"\bconfig_string\s*\(\s*KernelNumPriorities\s+NUM_PRIORITIES\s+[^)]*\bDEFAULT\s+(\d+)\s+UNQUOTE\s*\)",
        kernel_config,
    )
    overrides = re.findall(r"\bset\s*\(\s*KernelNumPriorities\s+([^)]*)\)", config)
    if defaults != ["256"] or (
        overrides
        and (
            len(overrides) != 1
            or re.fullmatch(r'256\s+CACHE\s+STRING\s+""\s*(?:FORCE\s*)?', overrides[0]) is None
        )
    ):
        _reject("qualified kernel priority count must remain 256, making seL4_MaxPrio 255")


def _audit_controls() -> int:
    """Independent temporary sources prove that qualification refuses bypasses."""
    baseline = {
        "slime-root/src/lib.rs": "pub mod diagnostic;\n",
        "slime-root/src/main.rs": 'fn main() { slime_root::diagnostic_println!("root"); }\n',
        "slime-root/src/console.rs": "fn payload(bytes: &[u8]) { crate::diagnostic::write(bytes); }\n",
        "slime-root/src/diagnostic.rs": "pub fn write(bytes: &[u8]) { for byte in bytes { sel4::debug_put_char(*byte); } sel4::r#yield(); }\npub fn print(args: core::fmt::Arguments) {}\n",
        "components/runtime/src/syscall/sel4_transport.rs": "".join(
            f"/// {clause}\n" for clause in _DEBUG_WRITE_CONTRACT
        )
        + "pub fn debug_write(bytes: &[u8]) -> i64 { 0 }\n",
        "slime-root/src/graph_runtime/console_runtime.rs": "fn schedule() { tcb.tcb_set_sched_params(sel4::init_thread::slot::TCB.cap(), 255, 255); }\n",
        "sel4/config/qemu-arm-virt.cmake": 'set(KernelPlatform "qemu-arm-virt" CACHE STRING "")\nset(KernelIsMCS OFF CACHE BOOL "")\nset(KernelMaxNumNodes 1 CACHE STRING "")\n',
        "sel4/pins.toml": "[qemu_arm_virt]\ncpus = 1\n",
        "deps/sel4/src/kernel/boot.c": "tcb->tcbPriority = seL4_MaxPrio;\n",
        "deps/sel4/libsel4/include/sel4/constants.h": "enum priorityConstants { seL4_MaxPrio = CONFIG_NUM_PRIORITIES - 1 };\n",
        "deps/sel4/config.cmake": 'config_string(KernelNumPriorities NUM_PRIORITIES "Priorities" DEFAULT 256 UNQUOTE)\n',
    }
    mutations = (
        ("raw print", "slime-root/src/console.rs", 'fn bad() { sel4::debug_print!("bad"); }'),
        ("raw alias", "slime-root/src/console.rs", "use sel4::debug_println as out;"),
        ("raw byte sink", "slime-root/src/console.rs", "fn bad() { sel4::debug_put_char(65); }"),
        (
            "private print helper",
            "slime-root/src/console.rs",
            "fn bad() { sel4::_private::printing::debug_print_helper(args); }",
        ),
        (
            "diagnostic formatter bypass",
            "slime-root/src/diagnostic.rs",
            'fn bad() { sel4::debug_println!("bad"); }',
        ),
        ("raw DebugWrite", "slime-root/src/console.rs", "use sel4::debug::DebugWrite as Writer;"),
        (
            "alternate direct syscall",
            "slime-root/src/console.rs",
            "fn bad() { seL4_DebugPutChar(65); }",
        ),
        ("duplicate root lock", "slime-root/src/main.rs", "mod diagnostic;"),
        ("test-only sink", "slime-root/src/diagnostic.rs", "#[cfg(test)] fn fake() {}"),
        (
            "conditional sink",
            "slime-root/src/diagnostic.rs",
            "#[cfg_attr(test, inline)] fn fake() {}",
        ),
        (
            "std-only yield",
            "slime-root/src/diagnostic.rs",
            "fn fake() { std::thread::yield_now(); }",
        ),
    )
    replacements = (
        (
            "missing debug_write public contract",
            "components/runtime/src/syscall/sel4_transport.rs",
            "pub fn debug_write(bytes: &[u8]) -> i64 { 0 }\n",
        ),
        (
            "obsolete debug_write rationale",
            "components/runtime/src/syscall/sel4_transport.rs",
            baseline["components/runtime/src/syscall/sel4_transport.rs"].replace(
                "pub fn debug_write",
                "/// The root graph is single-threaded, making atomicity structural.\npub fn debug_write",
            ),
        ),
        (
            "false emission acknowledgement",
            "components/runtime/src/syscall/sel4_transport.rs",
            baseline["components/runtime/src/syscall/sel4_transport.rs"].replace(
                "Send completion does not acknowledge emission.",
                "Send completion acknowledges emission.",
            ),
        ),
        ("missing library module", "slime-root/src/lib.rs", ""),
        ("missing binary shared route", "slime-root/src/main.rs", "fn main() {}"),
        ("missing console shared route", "slime-root/src/console.rs", "fn payload() {}"),
        (
            "fake native yield",
            "slime-root/src/diagnostic.rs",
            baseline["slime-root/src/diagnostic.rs"].replace(
                "sel4::r#yield();", "core::hint::spin_loop();"
            ),
        ),
        (
            "fake native byte sink",
            "slime-root/src/diagnostic.rs",
            baseline["slime-root/src/diagnostic.rs"].replace(
                "sel4::debug_put_char(*byte);", "drop(byte);"
            ),
        ),
        (
            "lower console priority",
            "slime-root/src/graph_runtime/console_runtime.rs",
            baseline["slime-root/src/graph_runtime/console_runtime.rs"].replace(
                "255, 255", "254, 254"
            ),
        ),
        (
            "MCS target",
            "sel4/config/qemu-arm-virt.cmake",
            baseline["sel4/config/qemu-arm-virt.cmake"].replace(
                "KernelIsMCS OFF", "KernelIsMCS ON"
            ),
        ),
        (
            "SMP target",
            "sel4/config/qemu-arm-virt.cmake",
            baseline["sel4/config/qemu-arm-virt.cmake"].replace(
                "KernelMaxNumNodes 1", "KernelMaxNumNodes 2"
            ),
        ),
        ("SMP launch", "sel4/pins.toml", "[qemu_arm_virt]\ncpus = 2\n"),
        ("lower kernel root priority", "deps/sel4/src/kernel/boot.c", "tcb->tcbPriority = 254;\n"),
        (
            "different kernel maximum priority",
            "deps/sel4/libsel4/include/sel4/constants.h",
            "enum priorityConstants { seL4_MaxPrio = 254 };\n",
        ),
        (
            "different kernel priority count",
            "deps/sel4/config.cmake",
            baseline["deps/sel4/config.cmake"].replace("DEFAULT 256", "DEFAULT 128"),
        ),
        (
            "profile priority-count override",
            "sel4/config/qemu-arm-virt.cmake",
            baseline["sel4/config/qemu-arm-virt.cmake"]
            + 'set(KernelNumPriorities 128 CACHE STRING "")\n',
        ),
    )
    with tempfile.TemporaryDirectory(prefix="slime-console-audit-") as temporary:
        root = Path(temporary)
        for relative, source in baseline.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(source, encoding="utf-8")
        # Diagnostic words in prose/literals do not grant a raw output route.
        console = root / "slime-root/src/console.rs"
        console.write_text(
            baseline["slime-root/src/console.rs"]
            + '// sel4::debug_println!("ignored");\n/* nested /* DebugWrite */ comment */\nconst TEXT: &str = r#"sel4::debug_put_char(1)"#;\n',
            encoding="utf-8",
        )
        audit_routes(root)
        console.write_text(baseline["slime-root/src/console.rs"], encoding="utf-8")
        for description, relative, addition in mutations:
            path = root / relative
            path.write_text(baseline[relative] + addition, encoding="utf-8")
            try:
                audit_routes(root)
            except ValueError:
                pass
            else:
                _reject(f"source audit accepted {description}")
            finally:
                path.write_text(baseline[relative], encoding="utf-8")
        for description, relative, replacement in replacements:
            path = root / relative
            path.write_text(replacement, encoding="utf-8")
            try:
                audit_routes(root)
            except ValueError:
                pass
            else:
                _reject(f"source audit accepted {description}")
            finally:
                path.write_text(baseline[relative], encoding="utf-8")
        missing = root / "slime-root/src/diagnostic.rs"
        missing.unlink()
        try:
            audit_routes(root)
        except ValueError:
            pass
        else:
            _reject("source audit accepted a missing native diagnostic module")
    return len(mutations) + len(replacements) + 1


def check_controls(root: Path, gate: object, literal_for: Callable[[str], str]) -> int:
    """Exercise independent validator/audit controls without qualifying product."""
    del root  # Product audit is deliberately a separate qualification-only call.
    patterns = tuple(pattern for _, chain in gate.LOCAL_CHAINS for pattern in chain)
    if len(patterns) != 22:
        _reject(f"local record contract has {len(patterns)} markers, expected 22")
    baseline = local_control_transcript(gate, literal_for)
    lines = baseline.rstrip("\n").split("\n")
    validate_local(baseline, gate)
    evaluated = 0

    def refuse(description: str, transcript: str) -> None:
        nonlocal evaluated
        try:
            validate_local(transcript, gate)
        except ValueError:
            evaluated += 1
        else:
            _reject(f"local validator accepted {description}")

    for index in range(len(lines)):
        refuse(f"missing marker {index}", "\n".join(lines[:index] + lines[index + 1 :]) + "\n")
    for label, chain in gate.LOCAL_CHAINS:
        if len(chain) > 1:
            first, second = (lines.index(literal_for(pattern)) for pattern in chain[:2])
            reordered = lines.copy()
            reordered[first], reordered[second] = reordered[second], reordered[first]
            refuse(f"reordered {label}", "\n".join(reordered) + "\n")
    refuse("duplicate admission prefix", baseline + lines[0] + "additional=boot-fields\n")
    qualified = lines[1:]
    for index, line in enumerate(qualified):
        prefix = next(
            prefix for prefix in (_PROBE, *_SERVICE_PREFIXES, _HEALTH) if line.startswith(prefix)
        )
        midpoint = max(len(prefix), len(line) // 2)
        stem_end = {_PROBE: len("[io-local-network-"), _HEALTH: len("SLIME_GRAPH HEALTH")}.get(
            prefix, len("[network-service")
        )
        refuse(
            f"split prefix extra marker {index}",
            baseline + line[:stem_end] + "\n" + line[stem_end:] + "\n",
        )
        refuse(
            f"split marker {index}",
            baseline.replace(line, line[:midpoint] + "\n" + line[midpoint:], 1),
        )
        refuse(f"duplicate marker {index}", baseline + line + "\n")
        refuse(f"prefixed extra marker {index}", baseline + "fragment " + line + "\n")
        refuse(f"suffix extra marker {index}", baseline + line + " fragment\n")
        refuse(
            f"altered payload extra marker {index}",
            baseline + re.sub(r"=\d+", "=999999", line, count=1) + "\n",
        )
        refuse(
            f"split extra marker {index}",
            baseline + line[:midpoint] + "\n" + line[midpoint:] + "\n",
        )
    for pattern in gate.FAILURE_MARKERS:
        refuse(f"explicit failure {pattern}", baseline + literal_for(pattern) + "\n")
    refuse("probe debug status outside failure path", baseline + _PROBE + " debug status=7\n")
    refuse("unknown service family", baseline + "[network-service] ungraded record=1\n")
    huge = baseline.replace(
        "[network-service] wait wakes=7", "[network-service] wait wakes=18446744073709551616", 1
    )
    if huge == baseline:
        wait_pattern = gate.LOCAL_CHAINS[1][1][1]
        wait_line = literal_for(wait_pattern)
        huge = baseline.replace(
            wait_line, re.sub(r"wakes=\d+", "wakes=18446744073709551616", wait_line), 1
        )
    refuse("unbounded numeric field", huge)
    reordered = lines.copy()
    sent_index = reordered.index(literal_for(_ADDITIONAL_LOCAL[7][0]))
    release_index = reordered.index(literal_for(_ADDITIONAL_LOCAL[9][0]))
    reordered[sent_index], reordered[release_index] = (
        reordered[release_index],
        reordered[sent_index],
    )
    refuse("buffer release before its session byte report", "\n".join(reordered) + "\n")
    reordered = lines.copy()
    accepted, listener = (
        reordered.index(literal_for(_ADDITIONAL_LOCAL[index][0])) for index in (5, 6)
    )
    reordered[accepted], reordered[listener] = reordered[listener], reordered[accepted]
    refuse("listener close before accepted close", "\n".join(reordered) + "\n")
    reordered = lines.copy()
    subscriber = reordered.index(literal_for(_ADDITIONAL_LOCAL[8][0]))
    pair = reordered[subscriber : subscriber + 2]
    del reordered[subscriber : subscriber + 2]
    listener = reordered.index(literal_for(_ADDITIONAL_LOCAL[6][0]))
    reordered[listener:listener] = pair
    refuse("subscriber byte report before listener close", "\n".join(reordered) + "\n")
    refuse("local invalidation", baseline + "[network-service] application session invalidated\n")
    refuse("local abort", baseline + "[network-service] application aborted sessions_released=1\n")
    for before, after in (
        (_ADDITIONAL_LOCAL[0][0], _ADDITIONAL_LOCAL[1][0]),
        (_ADDITIONAL_LOCAL[2][0], _ADDITIONAL_LOCAL[4][0]),
    ):
        reordered = lines.copy()
        first, second = (reordered.index(literal_for(pattern)) for pattern in (before, after))
        reordered[first], reordered[second] = reordered[second], reordered[first]
        refuse("reordered additional service startup", "\n".join(reordered) + "\n")
    audit_count = _audit_controls()
    print(
        f"seL4 gate control check: console records rejected {evaluated} mutations; source route audit rejected {audit_count} independent fixtures (no product qualification)"
    )
    return evaluated + audit_count
