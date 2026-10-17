"""Host-check actual local-network producers at their debug_write operation boundary.

Only producer functions and anchored output statements come from the product tree.
Expected records and the operation-level validator live in this frozen grader. A
repair may replace an anchored statement sequence with one Line::<N>::new()
expression ending in emit(); it must retain the literal and original local inputs.
The lexical route audit rejects detached marker helpers and qualified-prefix
bypasses; it is not a formal control-flow or reachability proof. Synthetic controls
establish grader sensitivity, not product qualification.
"""

from pathlib import Path
import re
import subprocess
import tempfile

from console_record_controls import _rust_code
from console_record_exam import _balanced


_PROBE = "components/testkit/io-local-network-probe/src/main.rs"
_NETWORK = "components/services/network-service/src/main.rs"
_LINE = "components/lib/src/console_line.rs"
_FAULT_INJECTION = r'''
fn inject_fault() -> ! {
    #[cfg(target_arch = "aarch64")]
    unsafe {
        core::arch::asm!("str xzr, [{address}]", address = in(reg) 0usize, options(nostack));
    }
    #[cfg(target_arch = "x86_64")]
    unsafe {
        core::arch::asm!("mov qword ptr [{address}], 0", address = in(reg) 0usize, options(nostack));
    }
    #[cfg(target_arch = "riscv64")]
    unsafe {
        core::arch::asm!("sd zero, 0({address})", address = in(reg) 0usize, options(nostack));
    }
    fail(b"fault injection returned")
}
'''
_LOCAL_MAIN_OUTPUTS = (
    'b"[network-service] tcp congestion control="',
    'b"[network-service] application session invalidated',
    'b"[network-service] application bytes sent="',
    'b"[network-service] application buffers released',
    'b"[network-service] application aborted sessions_released="',
    'b"[network-service] wait wakes="',
    'b"[network-service] loopback frames="',
)
_EXCLUDED_MAIN_OUTPUTS = (
    'b"[network-service] DNS seed admitted',
    'b"[network-service] client death handles="',
    'b"[network-service] tcp timeout handles="',
    'b"[network-service] driver reset requests="',
    'b"[network-service] injected in-flight fault',
    'b"[network-service] tcp listener refusals unadmitted="',
    'b"[network-service] tcp peaks rx_bytes="',
    'b"[network-service] application connection handles live="',
)


def _function(source: str, name: str, *, required: bool = True) -> str:
    match = re.search(rf"\bfn {re.escape(name)}\s*\(", source)
    if match is None:
        if required:
            raise ValueError(f"actual producer function absent: {name}")
        return ""
    brace = source.index("{", match.start())
    return source[match.start() : brace] + _balanced(source, brace, "{", "}")


def _statement_end(source: str, start: int) -> int:
    """Find a statement end without mistaking strings or nested calls for it."""
    cursor = start
    while cursor < len(source):
        char = source[cursor]
        if source[cursor : cursor + 2] == "//":
            newline = source.find("\n", cursor)
            cursor = len(source) if newline == -1 else newline + 1
            continue
        if char == '"':
            cursor += 1
            while cursor < len(source):
                if source[cursor] == "\\":
                    cursor += 2
                elif source[cursor] == '"':
                    cursor += 1
                    break
                else:
                    cursor += 1
            continue
        if char in "([{":
            region = _balanced(source, cursor, char, {"(": ")", "[": "]", "{": "}"}[char])
            cursor += len(region)
            continue
        if char == ";":
            return cursor + 1
        cursor += 1
    raise ValueError("actual output statement has no terminator")


def _output_block(function: str, literal: str) -> str:
    """Extract an actual legacy sequence or the frozen single-expression Line seam."""
    marker = function.index(literal)
    starts = list(
        re.finditer(
            r"(?m)^[ \t]*(?:debug_write|write_number|Line(?:::<[^>\n]+>)?::new)\s*\(",
            function[:marker],
        )
    )
    if not starts:
        raise ValueError(f"actual anchored producer statement absent: {literal}")
    start = starts[-1].start()
    end = _statement_end(function, start)
    first = function[start:end]
    if first.lstrip().startswith("Line"):
        if not re.search(r"\.emit\s*\(\s*\)\s*;\s*$", first):
            raise ValueError("actual Line producer must end in emit()")
        return first
    while True:
        statement = function[start:end]
        if re.search(r'debug_write\s*\(\s*b"(?:[^"\\]|\\.)*\\n"\s*\)\s*;\s*$', statement):
            return statement
        following = end
        while following < len(function) and function[following].isspace():
            following += 1
        if not re.match(r"(?:debug_write|write_number)\s*\(", function[following:]):
            raise ValueError(f"actual producer sequence changed before newline: {literal}")
        end = _statement_end(function, following)


def _without_comments(source: str) -> str:
    """Mask comments but retain quoted literals and code for lexical route checks."""
    output = list(source)
    cursor = 0
    while cursor < len(source):
        if source[cursor] == '"':
            cursor += 1
            while cursor < len(source):
                if source[cursor] == "\\":
                    cursor += 2
                elif source[cursor] == '"':
                    cursor += 1
                    break
                else:
                    cursor += 1
        elif source[cursor : cursor + 2] == "//":
            end = source.find("\n", cursor)
            end = len(source) if end == -1 else end
            output[cursor:end] = " " * (end - cursor)
            cursor = end
        elif source[cursor : cursor + 2] == "/*":
            depth = 1
            end = cursor + 2
            while end < len(source) and depth:
                if source[end : end + 2] == "/*":
                    depth += 1
                    end += 2
                elif source[end : end + 2] == "*/":
                    depth -= 1
                    end += 2
                else:
                    end += 1
            if depth:
                raise ValueError("unterminated producer comment")
            output[cursor:end] = " " * (end - cursor)
            cursor = end
        else:
            cursor += 1
    return "".join(output)


def audit_producer_routes(root: Path) -> None:
    """Check actual callers and unique qualified literal locations, not reachability."""
    probe = _without_comments((root / _PROBE).read_text())
    network = _without_comments((root / _NETWORK).read_text())
    # Only the frozen external-fault injection body may select native assembly.
    # Audit the rest of each whole source, including attributes before functions
    # omitted by extraction, so the host fixture cannot select a different sink.
    fault = _function(network, "inject_fault")
    if "".join(line.strip() for line in fault.splitlines()) != "".join(
        line.strip() for line in _FAULT_INJECTION.splitlines()
    ):
        raise ValueError("actual external fault injection body changed")
    conditional_sources = [(_PROBE, probe), (_NETWORK, network.replace(fault, "", 1))]
    line = root / _LINE
    if line.exists():
        conditional_sources.append((_LINE, line.read_text()))
    for path, source in conditional_sources:
        if re.search(r"\b(?:cfg|cfg_attr)\b", _rust_code(source)):
            raise ValueError(f"actual producer conditional compilation is forbidden: {path}")
    expected_routes = {
        "main": ("attached=1", "loans returned=2 shutdown=1"),
        "publish": (
            "authority_refusals=4",
            "connected=1",
            "sent=4096 received=2048 identical=1",
            "connections closed=1",
        ),
        "subscribe": (
            "authority_refusals=3",
            "listening=1",
            "accepted=1",
            "timeout=1 resumed=1",
            "sent=2048 received=4096 identical=1",
            "eof=1 connections closed=1 listeners closed=1",
        ),
    }
    for name, expected in expected_routes.items():
        function = _function(probe, name)
        observed = tuple(re.findall(r'\bmarker\s*\(\s*role\s*,\s*b"([^"\\]*)"\s*\)', function))
        if observed != expected:
            raise ValueError(f"actual marker caller route changed: {name}")
        if not re.search(r"\bfail\s*\(", function):
            raise ValueError(f"actual failure helper detached from: {name}")
    if len(re.findall(r"\bmarker\s*\(", probe)) != 13:
        raise ValueError("actual marker routes include an ungraded caller or duplicate helper")
    for name in ("publish", "subscribe"):
        if not re.search(
            rf"\b{name}\s*\(\s*&mut\s+io\s*,\s*&clock\s*,\s*role\s*\)", _function(probe, "main")
        ):
            raise ValueError(f"actual main no longer routes through: {name}")
    if not re.search(
        r"\brecv_response_after_stall\s*\(\s*io\s*,\s*&connection\s*,\s*&mut\s+answer\s*,\s*clock\s*\)",
        _function(probe, "publish"),
    ):
        raise ValueError("actual publisher no longer routes through the timeout reporter")
    for name in ("subscribe", "send_all", "recv_all", "recv_response_after_stall"):
        if not re.search(r"\bwould_block\s*\(\s*&reply\s*\)", _function(probe, name)):
            raise ValueError(f"actual status helper detached from: {name}")
    for name in ("recv_response_after_stall", "would_block"):
        if not re.search(r"\bfail\s*\(", _function(probe, name)):
            raise ValueError(f"actual failure helper detached from: {name}")
    probe_regions = [_function(probe, name) for name in ("marker", "fail", "would_block")]
    probe_regions += [
        _output_block(_function(probe, "main"), 'b"[io-local-network-probe] role="'),
        _output_block(
            _function(probe, "recv_response_after_stall"),
            'b"[io-local-network-probe] role=publisher timeout_retries="',
        ),
    ]
    brace = network.index("{", network.index("impl LocalStack {"))
    constructor = _function(_balanced(network, brace, "{", "}"), "new")
    if not re.search(r"\bLocalStack::new\s*\(\s*\)", _function(network, "main")):
        raise ValueError("actual main no longer routes through the local constructor")
    if not re.search(r"\bfail\s*\(", constructor) or not re.search(
        r"\bfail\s*\(", _function(network, "main")
    ):
        raise ValueError("actual network failure helper detached from qualified callers")
    network_main = _function(network, "main")
    for name, arguments in (
        ("report_authority", r"&destinations"),
        ("report_observed", r"&observed"),
        ("report_socket_options", r"table"),
        ("allocate_incarnation", r""),
        ("report_events", r"&mut\s+local_engine"),
    ):
        if len(re.findall(rf"\b{name}\s*\(\s*{arguments}\s*\)", network_main)) != 1:
            raise ValueError(f"actual local network reporter route changed: {name}")
    network_regions = [
        _function(network, name)
        for name in (
            "fail",
            "report_authority",
            "report_observed",
            "report_socket_options",
            "report_events",
            "allocate_incarnation",
        )
    ]
    network_regions += [_output_block(constructor, 'b"[network-service] loopback interface=')]
    network_regions += [_output_block(network_main, literal) for literal in _LOCAL_MAIN_OUTPUTS]
    # These exact producer regions belong to a declared external interface,
    # supervision/fault profiles, or DNS, absent from sel4-io-local.
    # Removing named regions is not an arbitrary-prefix allowlist.
    excluded_regions = [
        _function(network, name)
        for name in (
            "admit_external_listeners",
            "attach_stack",
            "release_stack",
            "report_dns",
            "report_tcp_bounds",
            "report_interface",
        )
    ]
    excluded_regions += [_output_block(network_main, literal) for literal in _EXCLUDED_MAIN_OUTPUTS]
    qualified_prefixes = tuple(
        re.findall(r'"(\[network-service\][^"\\]*)', "\n".join(network_regions))
    )
    if any(prefix in region for prefix in qualified_prefixes for region in excluded_regions):
        raise ValueError("qualified producer prefix outside the actual tested routes")
    network_regions += excluded_regions
    for source, regions, prefixes in (
        (probe, probe_regions, ("[io-local-network-probe]",)),
        (
            network,
            network_regions,
            ("[network-service]",),
        ),
    ):
        for region in regions:
            if source.count(region) != 1:
                raise ValueError("actual qualified producer region is duplicated")
            source = source.replace(region, "", 1)
        if any(prefix in source for prefix in prefixes):
            raise ValueError("qualified producer prefix outside the actual tested routes")


def fixture_source(root: Path) -> str:
    """Compile product bodies; never derive expected bytes from those bodies."""
    root = root.resolve()
    audit_producer_routes(root)
    probe = (root / _PROBE).read_text()
    network = (root / _NETWORK).read_text()
    probe_main = _function(probe, "main")
    network_main = _function(network, "main")
    local_impl_start = network.index("impl LocalStack {")
    local_impl_brace = network.index("{", local_impl_start)
    local_constructor = _function(_balanced(network, local_impl_brace, "{", "}"), "new")
    line = root / _LINE
    line_include = f'#[path = "{line}"] mod console_line;' if line.exists() else ""
    line_import = "use crate::console_line::Line;" if line.exists() else ""
    replacements = {
        "@LINE@": line_include,
        "@LINE_IMPORT@": line_import,
        "@PROBE_FUNCTIONS@": "\n".join(
            _function(probe, name, required=name != "write_number")
            for name in ("marker", "write_number", "fail", "would_block")
        ),
        "@NETWORK_FUNCTIONS@": "\n".join(
            _function(network, name, required=name != "write_number")
            for name in (
                "write_number",
                "fail",
                "report_authority",
                "report_observed",
                "report_socket_options",
                "report_events",
            )
        ),
        "@OBSERVED_TYPE@": re.search(r"struct Observed\s*\{", network).group(0)[:-1]
        + _balanced(network, network.index("{", network.index("struct Observed")), "{", "}"),
        "@INCARNATION@": _output_block(
            _function(network, "allocate_incarnation"), 'b"[network-service] incarnation="'
        ),
        "@CONGESTION@": _output_block(network_main, 'b"[network-service] tcp congestion control="'),
        "@APPLICATION_BYTES@": _output_block(
            network_main, 'b"[network-service] application bytes sent="'
        ),
        "@APPLICATION_RELEASE@": _output_block(
            network_main, 'b"[network-service] application buffers released'
        ),
        "@APPLICATION_INVALIDATED@": _output_block(
            network_main, 'b"[network-service] application session invalidated'
        ),
        "@APPLICATION_ABORT@": _output_block(
            network_main, 'b"[network-service] application aborted sessions_released="'
        ),
        "@NOTIFICATION@": _output_block(probe_main, 'b"[io-local-network-probe] role="'),
        "@TIMEOUT@": _output_block(
            _function(probe, "recv_response_after_stall"),
            'b"[io-local-network-probe] role=publisher timeout_retries="',
        ),
        "@WAIT@": _output_block(network_main, 'b"[network-service] wait wakes="'),
        "@LOOPBACK@": _output_block(network_main, 'b"[network-service] loopback frames="'),
        "@INTERFACE@": _output_block(
            local_constructor,
            'b"[network-service] loopback interface=',
        ),
    }
    source = _ADAPTER + _VALIDATOR + _PRODUCT + _CASES
    for key, value in replacements.items():
        source = source.replace(key, value)
    return source


def _run(source: str, timeout: int) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="console-producer-exam-") as directory:
        rust = Path(directory) / "exam.rs"
        binary = Path(directory) / "exam"
        rust.write_text(source)
        compiled = subprocess.run(
            ["rustc", "--edition=2024", str(rust), "-o", str(binary)],
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
        if compiled.returncode:
            return compiled
        return subprocess.run(
            [str(binary)], capture_output=True, text=True, timeout=timeout, check=False
        )


def run_exam(root: Path, timeout: int = 30) -> subprocess.CompletedProcess[str]:
    """The fragmented baseline must compile and then fail, never qualify as a pass."""
    return _run(fixture_source(root), timeout)


def check_controls(root: Path) -> int:
    """Frozen operation controls and actual-source lexical route mutations."""
    root = root.resolve()
    audit_producer_routes(root)
    route_mutations = (
        "detached-marker",
        "parallel-qualified-prefix",
        "network-prefix-decoy",
        "authority-prefix-decoy",
        "observed-prefix-decoy",
        "detached-authority",
        "detached-observed",
        "detached-incarnation",
        "detached-options",
        "detached-events",
    )
    for mutation in route_mutations:
        with tempfile.TemporaryDirectory(prefix="producer-route-control-") as directory:
            subject = Path(directory)
            for path in (_PROBE, _NETWORK):
                destination = subject / path
                destination.parent.mkdir(parents=True, exist_ok=True)
                source = (root / path).read_text()
                if path == _PROBE:
                    if mutation == "detached-marker":
                        source = source.replace(
                            'marker(role, b"attached=1")', 'unused_marker(role, b"attached=1")'
                        )
                    elif mutation == "parallel-qualified-prefix":
                        source += '\nfn decoy() { debug_write(b"[io-local-network-probe] role=publisher attached=1\\n"); }\n'
                elif mutation in (
                    "network-prefix-decoy",
                    "authority-prefix-decoy",
                    "observed-prefix-decoy",
                ):
                    literal = {
                        "network-prefix-decoy": "loopback interface=127.0.0.1 external_nic=none",
                        "authority-prefix-decoy": "authority destinations=1 rights=connect,send,recv",
                        "observed-prefix-decoy": "observed requests=2 packets=0",
                    }[mutation]
                    source += (
                        f'\nfn decoy() {{ debug_write(b"[network-service] {literal}\\n"); }}\n'
                    )
                elif mutation in (
                    "detached-authority",
                    "detached-observed",
                    "detached-incarnation",
                    "detached-options",
                    "detached-events",
                ):
                    name, arguments = {
                        "detached-authority": ("report_authority", "&destinations"),
                        "detached-observed": ("report_observed", "&observed"),
                        "detached-incarnation": ("allocate_incarnation", ""),
                        "detached-options": ("report_socket_options", "table"),
                        "detached-events": ("report_events", "&mut local_engine"),
                    }[mutation]
                    source = source.replace(
                        f"{name}({arguments})", f"unused_{name}({arguments})", 1
                    )
                destination.write_text(source)
            try:
                audit_producer_routes(subject)
            except ValueError as error:
                reason = (
                    "actual marker caller route changed"
                    if mutation == "detached-marker"
                    else "actual local network reporter route changed"
                    if mutation.startswith("detached-")
                    else "qualified producer prefix outside"
                )
                if reason not in str(error):
                    raise AssertionError(
                        f"route mutation failed for unrelated reason: {error}"
                    ) from error
            else:
                raise AssertionError(f"route mutation was accepted: {mutation}")
    conditional_mutations = (
        ("marker target attribute", _PROBE, "marker", '#[cfg(target_arch = "x86_64")]'),
        ("marker alternate targets", _PROBE, "marker", "alternate"),
        ("failure test attribute", _PROBE, "fail", "#[cfg_attr(test, inline)]"),
        ("status target macro", _PROBE, "would_block", "macro"),
        ("probe numeric target attribute", _PROBE, "write_number", "#[cfg(not(test))]"),
        ("probe enclosing main attribute", _PROBE, "main", "#[cfg(not(test))]"),
        ("authority target attribute", _NETWORK, "report_authority", "#[cfg(not(test))]"),
        ("observed target attribute", _NETWORK, "report_observed", "#[cfg(not(test))]"),
        ("options target attribute", _NETWORK, "report_socket_options", "#[cfg(not(test))]"),
        ("events target attribute", _NETWORK, "report_events", "#[cfg(not(test))]"),
        ("incarnation target attribute", _NETWORK, "allocate_incarnation", "#[cfg(not(test))]"),
        ("network enclosing main attribute", _NETWORK, "main", "#[cfg(not(test))]"),
        ("network failure target macro", _NETWORK, "fail", "macro"),
        ("network numeric target attribute", _NETWORK, "write_number", "#[cfg(not(test))]"),
        ("fault enclosing attribute", _NETWORK, "inject_fault", "#[cfg(not(test))]"),
        ("fault body replacement", _NETWORK, "inject_fault", "macro"),
        ("Line target attribute", _LINE, "emit", '#[cfg(target_arch = "x86_64")]'),
        ("Line test attribute", _LINE, "emit", "#[cfg_attr(test, inline)]"),
        ("Line target macro", _LINE, "emit", "macro"),
    )
    for description, path, name, conditional in conditional_mutations:
        with tempfile.TemporaryDirectory(prefix="producer-conditional-control-") as directory:
            subject = Path(directory)
            for relative in (_PROBE, _NETWORK, _LINE):
                original = root / relative
                if not original.exists() and relative != path:
                    continue
                source = (
                    original.read_text()
                    if original.exists()
                    else "pub struct Line; impl Line { pub fn emit(self) {} }"
                )
                if relative == path:
                    function = _function(source, name, required=name != "write_number")
                    if not function:
                        source += "\nfn write_number(_: &[u8], _: u64) {}\n"
                        function = _function(source, name)
                    start = source.index(function)
                    visibility = re.search(r"\bpub(?:\([^)]*\))?\s*$", source[:start])
                    if visibility:
                        function = source[visibility.start() : start] + function
                    if conditional == "alternate":
                        replacement = (
                            '#[cfg(target_arch = "x86_64")]\n'
                            + function
                            + '\n#[cfg(target_arch = "aarch64")]\n'
                            + function
                        )
                    elif conditional == "macro":
                        replacement = function.replace(
                            "{", '{ let _ = cfg!(target_arch = "x86_64");', 1
                        )
                    else:
                        replacement = conditional + "\n" + function
                    source = source.replace(function, replacement, 1)
                destination = subject / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_text(source)
            try:
                # Extraction must fail before rustc can erase attributes, pick
                # its host branch, or reject an otherwise malformed mutation.
                fixture_source(subject)
            except ValueError as error:
                reason = (
                    "actual external fault injection body changed"
                    if description == "fault body replacement"
                    else f"actual producer conditional compilation is forbidden: {path}"
                )
                if reason not in str(error):
                    raise AssertionError(
                        f"conditional mutation failed for unrelated reason: {description}: {error}"
                    ) from error
            else:
                raise AssertionError(f"conditional producer mutation was accepted: {description}")
    # Comments and every supported literal spelling must not trigger the token
    # audit; the unchanged native fault assembly remains a legitimate exception.
    with tempfile.TemporaryDirectory(prefix="producer-conditional-positive-") as directory:
        subject = Path(directory)
        for relative in (_PROBE, _NETWORK, _LINE):
            original = root / relative
            destination = subject / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            source = original.read_text() if original.exists() else "pub struct Line;"
            destination.write_text(
                source
                + r'''
// #[cfg(test)] cfg!(target_arch = "x86_64")
/* #[cfg_attr(test, inline)] /* cfg!(test) */ */
const AUDIT_WORDS: &str = "cfg cfg_attr";
const AUDIT_RAW_WORDS: &str = r#"#[cfg(test)] cfg_attr"#;
const AUDIT_BYTES: &[u8] = br#"cfg!(test) cfg_attr"#;
const AUDIT_CHAR: char = 'c';
'''
            )
        audit_producer_routes(subject)
    result = _run(_ADAPTER + _VALIDATOR + _CONTROLS, 30)
    if result.returncode:
        raise AssertionError(f"producer sensitivity controls failed:\n{result.stderr}")
    if result.stdout.strip() != "producer sensitivity controls passed=39":
        raise AssertionError("producer controls did not report their frozen case count")
    return 39 + len(route_mutations) + len(conditional_mutations) + 1


_ADAPTER = r"""
#![allow(dead_code, unused_imports)]
extern crate self as slime_rt;
use std::cell::RefCell;
thread_local! {
    static OPERATIONS: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}
pub fn debug_write(bytes: &[u8]) -> i64 {
    OPERATIONS.with(|operations| operations.borrow_mut().push(bytes.to_vec()));
    0
}
#[derive(Debug)] struct Exit(i64);
pub fn exit(code: i64) -> ! { std::panic::panic_any(Exit(code)); }
fn capture(call: impl FnOnce(), exit_code: Option<i64>) -> Vec<Vec<u8>> {
    OPERATIONS.with(|operations| operations.borrow_mut().clear());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(call));
    match (result, exit_code) {
        (Ok(()), None) => {},
        (Err(payload), Some(expected)) => {
            assert_eq!(payload.downcast_ref::<Exit>().map(|exit| exit.0), Some(expected),
                "producer failed for a reason other than the dedicated exit sentinel");
        },
        _ => panic!("producer did not obey expected exit behavior"),
    }
    OPERATIONS.with(|operations| std::mem::take(&mut *operations.borrow_mut()))
}
"""

_VALIDATOR = r"""
fn validate(operations: &[Vec<u8>], expected: &[&[u8]]) -> Result<(), String> {
    if operations.iter().any(|bytes| bytes.len() > 1024) {
        return Err("oversized record operation".into());
    }
    if operations.iter().any(|bytes| std::str::from_utf8(bytes).is_err()) {
        return Err("non-UTF-8 record operation".into());
    }
    if operations.len() != expected.len() {
        return Err(format!("fragmented record: expected {} operations, observed {}",
            expected.len(), operations.len()));
    }
    if !operations.iter().zip(expected).all(|(actual, expected)| actual == expected) {
        return Err("whole-record bytes differ".into());
    }
    Ok(())
}
fn grade(name: &str, operations: Vec<Vec<u8>>, expected: &[&[u8]], failures: &mut usize) {
    if let Err(error) = validate(&operations, expected) {
        eprintln!("producer case {name}: {error}");
        *failures += 1;
    }
}
"""

_PRODUCT = r"""
@LINE@
mod probe {
    use crate::{debug_write, exit};
    @LINE_IMPORT@
    struct NetworkReply { status_detail: i32, transferred: u64, queue_status: u32, capability_kind: u8 }
    mod net { pub const STATUS_WOULD_BLOCK: i32 = -11; }
    @PROBE_FUNCTIONS@
    pub fn mark(role: &[u8], message: &[u8]) { marker(role, message); }
    pub fn failure(reason: &[u8]) { fail(reason); }
    pub fn notification(role: &[u8], wakes: u64) { @NOTIFICATION@ }
    pub fn timeout(timeouts: u64) { @TIMEOUT@ }
    pub fn status(status_detail: i32, transferred: u64, queue_status: u32, capability_kind: u8) {
        would_block(&NetworkReply { status_detail, transferred, queue_status, capability_kind });
    }
}
mod network {
    use crate::{debug_write, exit};
    @LINE_IMPORT@
    const RIGHT_CONNECT: u16 = 1;
    const RIGHT_SEND: u16 = 2;
    const RIGHT_RECV: u16 = 4;
    const RIGHT_LISTEN: u16 = 8;
    pub struct Destination { rights: u16, socket_limit: u32, listener_limit: u32, dns_record_limit: u32 }
    struct NetworkDestinations<'a> { rows: &'a [Destination] }
    impl NetworkDestinations<'_> {
        fn destination_count(&self) -> usize { self.rows.len() }
        fn destination(&self, index: usize) -> Option<&Destination> { self.rows.get(index) }
    }
    #[derive(Clone, Copy)]
    struct Options { keepalive_ms: u32, nagle: bool, hop_limit: u8, idle_timeout_ms: u32 }
    struct ApplicationEntry<'a> { control_binding: &'a [u8], holder_identity: [u8; 32], options: Options }
    struct NetworkApplications<'a> { rows: &'a [ApplicationEntry<'a>] }
    impl NetworkApplications<'_> {
        fn application_count(&self) -> usize { self.rows.len() }
        fn application(&self, index: usize) -> Option<&ApplicationEntry<'_>> { self.rows.get(index) }
    }
    // Local control stems do not equal the declared publisher/subscriber holder.
    mod boot_contracts { pub mod network_destination {
        pub fn holder_identity(_: &str) -> [u8; 32] { [0; 32] }
    } }
    mod tcp {
        pub enum Terminal { TimeWait, Closed, Reset, Timeout }
        pub enum Event {
            AcceptedClose { unread: usize, unsent: usize, terminal: Terminal, bytes: usize },
            ListenerClosed { children: usize },
        }
        pub struct Engine<'a> { pub events: std::vec::IntoIter<Event>, pub marker: std::marker::PhantomData<&'a ()> }
        impl Engine<'_> { pub fn take_event(&mut self) -> Option<Event> { self.events.next() } }
    }
    @OBSERVED_TYPE@
    @NETWORK_FUNCTIONS@
    struct Wait { value: usize }
    impl Wait { fn wakes(&self) -> usize { self.value } }
    struct Device { values: [u64; 5] }
    impl Device {
        fn egress_count(&self) -> u64 { self.values[0] }
        fn rejected_count(&self) -> u64 { self.values[1] }
        fn reset_count(&self) -> u64 { self.values[2] }
        fn syn_count(&self) -> u64 { self.values[3] }
        fn fin_count(&self) -> u64 { self.values[4] }
    }
    struct Stack { device: Device }
    struct Engine { handles: usize }
    impl Engine {
        fn allocated(&self) -> usize { self.handles }
        fn congestion_control(&self) -> &'static [u8] { b"reno" }
    }
    struct Application { sent: u64, received: u64 }
    pub fn authority(rows: &[[u32; 4]]) {
        assert!(rows.len() <= 64, "bounded destination fixture");
        let rows: Vec<_> = rows.iter().map(|row| Destination { rights: row[0] as u16, socket_limit: row[1], listener_limit: row[2], dns_record_limit: row[3] }).collect();
        report_authority(&NetworkDestinations { rows: &rows });
    }
    pub fn observed(fields: [u32; 6]) {
        report_observed(&Observed { requests: fields[0], packets: fields[1], socket_refusals: fields[2], listener_refusals: fields[3], dns_refusals: fields[4], cross_holder_refusals: fields[5] });
    }
    pub fn options() { options_fields(0, true, 64, 10000); }
    pub fn options_fields(keepalive_ms: u32, nagle: bool, hop_limit: u8, idle_timeout_ms: u32) {
        let options = Options { keepalive_ms, nagle, hop_limit, idle_timeout_ms };
        let rows = [ApplicationEntry { control_binding: b"local-publisher-control", holder_identity: [1; 32], options }, ApplicationEntry { control_binding: b"local-subscriber-control", holder_identity: [2; 32], options }];
        report_socket_options(&NetworkApplications { rows: &rows });
    }
    pub fn events(terminal: u8, unread: usize, unsent: usize, bytes: usize, children: usize) {
        let terminal = match terminal { 0 => tcp::Terminal::TimeWait, 1 => tcp::Terminal::Closed, 2 => tcp::Terminal::Reset, _ => tcp::Terminal::Timeout };
        let events = vec![tcp::Event::AcceptedClose { unread, unsent, terminal, bytes }, tcp::Event::ListenerClosed { children }].into_iter();
        report_events(&mut tcp::Engine { events, marker: std::marker::PhantomData });
    }
    pub fn incarnation(next: u64) { @INCARNATION@ }
    pub fn congestion() { let engine = Engine { handles: 0 }; @CONGESTION@ }
    pub fn application_bytes(sent: u64, received: u64) { let application = Application { sent, received }; @APPLICATION_BYTES@ }
    pub fn application_release() { @APPLICATION_RELEASE@ }
    pub fn application_invalidated() { @APPLICATION_INVALIDATED@ }
    pub fn application_abort(released: u64) { @APPLICATION_ABORT@ }
    pub fn failure(reason: &[u8]) { fail(reason); }
    pub fn interface() { @INTERFACE@ }
    pub fn wait(wakes: usize, coalesced: u64) {
        let wait = Wait { value: wakes };
        @WAIT@
    }
    pub fn loopback(values: [u64; 5], handles: usize, external_frames: u64) {
        let stack = Stack { device: Device { values } };
        let local_engine = Engine { handles };
        @LOOPBACK@
    }
}
"""

_CASES = r"""
fn main() {
    std::panic::set_hook(Box::new(|_| {}));
    let mut failures = 0;
    let mut cases = 0;
    macro_rules! case {
        ($name:expr, $call:expr, $exit:expr, $expected:expr) => {{
            cases += 1;
            grade($name, capture(|| { $call; }, $exit), $expected, &mut failures);
        }};
    }
    // Every marker message in the publisher/subscriber chain is pinned here,
    // rather than discovered from marker() calls in the subject under test.
    for (role, message, expected) in [
        (b"publisher".as_slice(), b"attached=1".as_slice(), b"[io-local-network-probe] role=publisher attached=1\n".as_slice()),
        (b"publisher", b"authority_refusals=4", b"[io-local-network-probe] role=publisher authority_refusals=4\n"),
        (b"publisher", b"connected=1", b"[io-local-network-probe] role=publisher connected=1\n"),
        (b"publisher", b"sent=4096 received=2048 identical=1", b"[io-local-network-probe] role=publisher sent=4096 received=2048 identical=1\n"),
        (b"publisher", b"connections closed=1", b"[io-local-network-probe] role=publisher connections closed=1\n"),
        (b"publisher", b"loans returned=2 shutdown=1", b"[io-local-network-probe] role=publisher loans returned=2 shutdown=1\n"),
        (b"subscriber", b"attached=1", b"[io-local-network-probe] role=subscriber attached=1\n"),
        (b"subscriber", b"authority_refusals=3", b"[io-local-network-probe] role=subscriber authority_refusals=3\n"),
        (b"subscriber", b"listening=1", b"[io-local-network-probe] role=subscriber listening=1\n"),
        (b"subscriber", b"accepted=1", b"[io-local-network-probe] role=subscriber accepted=1\n"),
        (b"subscriber", b"timeout=1 resumed=1", b"[io-local-network-probe] role=subscriber timeout=1 resumed=1\n"),
        (b"subscriber", b"sent=2048 received=4096 identical=1", b"[io-local-network-probe] role=subscriber sent=2048 received=4096 identical=1\n"),
        (b"subscriber", b"eof=1 connections closed=1 listeners closed=1", b"[io-local-network-probe] role=subscriber eof=1 connections closed=1 listeners closed=1\n"),
        (b"subscriber", b"loans returned=2 shutdown=1", b"[io-local-network-probe] role=subscriber loans returned=2 shutdown=1\n"),
    ] { case!("marker chain", probe::mark(role, message), None, &[expected]); }
    case!("probe failure", probe::failure(b"exam failure"), Some(1), &[b"[io-local-network-probe] fail: exam failure\n".as_slice()]);
    case!("network failure", network::failure(b"exam failure"), Some(1), &[b"[network-service] fail: exam failure\n".as_slice()]);
    case!("invalid probe reason", probe::failure(b"valid prefix\xff"), Some(1), &[]);
    case!("invalid network reason", network::failure(b"valid prefix\xff"), Some(1), &[]);
    case!("oversized probe reason", probe::failure(&[b'R'; 1025]), Some(1), &[]);
    case!("oversized network reason", network::failure(&[b'R'; 1025]), Some(1), &[]);
    case!("invalid marker role", probe::mark(b"publisher\xff", b"attached=1"), None, &[]);
    case!("invalid marker message", probe::mark(b"publisher", b"attached=1\xff"), None, &[]);
    case!("oversized marker", probe::mark(b"publisher", &[b'M'; 1025]), None, &[]);
    case!("notification publisher", probe::notification(b"publisher", 7), None, &[b"[io-local-network-probe] role=publisher notification_wakes=7\n".as_slice()]);
    case!("notification subscriber", probe::notification(b"subscriber", 7), None, &[b"[io-local-network-probe] role=subscriber notification_wakes=7\n".as_slice()]);
    case!("notification maximum", probe::notification(b"publisher", u64::MAX), None, &[b"[io-local-network-probe] role=publisher notification_wakes=18446744073709551615\n".as_slice()]);
    case!("timeout zero", probe::timeout(0), None, &[b"[io-local-network-probe] role=publisher timeout_retries=0\n".as_slice()]);
    case!("timeout one", probe::timeout(1), None, &[b"[io-local-network-probe] role=publisher timeout_retries=1\n".as_slice()]);
    case!("would block silent", probe::status(-11, 0, 0, 0), None, &[]);
    case!("unexpected status", probe::status(i32::MIN, 0, 9, 3), Some(1), &[
        b"[io-local-network-probe] status magnitude=2147483648 queue=9 capability_kind=3\n".as_slice(),
        b"[io-local-network-probe] fail: unexpected network status\n".as_slice(),
    ]);
    case!("unexpected transfer", probe::status(-11, 1, 0, 0), Some(1), &[
        b"[io-local-network-probe] status magnitude=11 queue=0 capability_kind=0\n".as_slice(),
        b"[io-local-network-probe] fail: unexpected network status\n".as_slice(),
    ]);
    case!("loopback interface", network::interface(), None, &[b"[network-service] loopback interface=127.0.0.1 external_nic=none\n".as_slice()]);
    case!("network wait zero", network::wait(0, 0), None, &[b"[network-service] wait wakes=0 coalesced=0\n".as_slice()]);
    case!("network wait", network::wait(17, 3), None, &[b"[network-service] wait wakes=17 coalesced=3\n".as_slice()]);
    case!("loopback zero", network::loopback([0; 5], 0, 0), None, &[b"[network-service] loopback frames=0 rejected=0 handles=0 external_frames=0 resets=0 syns=0 fins=0\n".as_slice()]);
    case!("loopback fields", network::loopback([11, 2, 5, 7, 13], 3, 0), None, &[b"[network-service] loopback frames=11 rejected=2 handles=3 external_frames=0 resets=5 syns=7 fins=13\n".as_slice()]);
    case!("loopback external", network::loopback([11, 2, 5, 7, 13], 3, 19), None, &[b"[network-service] loopback frames=11 rejected=2 handles=3 external_frames=19 resets=5 syns=7 fins=13\n".as_slice()]);
    case!("local authority", network::authority(&[[7, 1, 0, 0]]), None, &[
        b"[network-service] authority destinations=1 rights=connect,send,recv\n".as_slice(),
        b"[network-service] declared socket_limit=1 listener_limit=0 dns_record_limit=0\n".as_slice(),
    ]);
    case!("empty authority", network::authority(&[]), None, &[
        b"[network-service] authority destinations=0 rights=\n".as_slice(),
        b"[network-service] declared socket_limit=0 listener_limit=0 dns_record_limit=0\n".as_slice(),
    ]);
    case!("authority field sums", network::authority(&[[9, 2, 3, 5], [6, 7, 11, 13]]), None, &[
        b"[network-service] authority destinations=2 rights=connect,send,recv,listen\n".as_slice(),
        b"[network-service] declared socket_limit=9 listener_limit=14 dns_record_limit=18\n".as_slice(),
    ]);
    case!("local observed", network::observed([2, 0, 0, 0, 0, 0]), None, &[b"[network-service] observed requests=2 packets=0 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=0\n".as_slice()]);
    case!("observed fields", network::observed([3, 5, 7, 11, 13, 17]), None, &[b"[network-service] observed requests=3 packets=5 socket_refusals=7 listener_refusals=11 dns_refusals=13 cross_holder_refusals=17\n".as_slice()]);
    case!("incarnation", network::incarnation(1), None, &[b"[network-service] incarnation=1\n".as_slice()]);
    case!("incarnation maximum", network::incarnation(1073741823), None, &[b"[network-service] incarnation=1073741823\n".as_slice()]);
    case!("congestion", network::congestion(), None, &[b"[network-service] tcp congestion control=reno\n".as_slice()]);
    case!("local options", network::options(), None, &[
        b"[network-service] tcp options holder=unnamed keepalive_ms=0 nagle=1 hop_limit=64 idle_timeout_ms=10000\n".as_slice(),
        b"[network-service] tcp options holder=unnamed keepalive_ms=0 nagle=1 hop_limit=64 idle_timeout_ms=10000\n".as_slice(),
    ]);
    case!("option fields", network::options_fields(3, false, 7, 11), None, &[
        b"[network-service] tcp options holder=unnamed keepalive_ms=3 nagle=0 hop_limit=7 idle_timeout_ms=11\n".as_slice(),
        b"[network-service] tcp options holder=unnamed keepalive_ms=3 nagle=0 hop_limit=7 idle_timeout_ms=11\n".as_slice(),
    ]);
    case!("local events", network::events(1, 0, 0, 4096, 0), None, &[
        b"[network-service] tcp accepted close unread=0 unsent=0 terminal=closed handles=1 bytes=4096\n".as_slice(),
        b"[network-service] tcp listener closed children=0\n".as_slice(),
    ]);
    for (terminal, expected) in [
        (0, b"[network-service] tcp accepted close unread=3 unsent=5 terminal=time-wait handles=1 bytes=7\n".as_slice()),
        (2, b"[network-service] tcp accepted close unread=3 unsent=5 terminal=reset handles=1 bytes=7\n".as_slice()),
        (3, b"[network-service] tcp accepted close unread=3 unsent=5 terminal=timeout handles=1 bytes=7\n".as_slice()),
    ] { case!("event fields", network::events(terminal, 3, 5, 7, 11), None, &[expected, b"[network-service] tcp listener closed children=11\n".as_slice()]); }
    case!("publisher application bytes", network::application_bytes(4096, 2048), None, &[b"[network-service] application bytes sent=4096 received=2048\n".as_slice()]);
    case!("subscriber application bytes", network::application_bytes(2048, 4096), None, &[b"[network-service] application bytes sent=2048 received=4096\n".as_slice()]);
    case!("application release", network::application_release(), None, &[b"[network-service] application buffers released\n".as_slice()]);
    case!("application invalidated", network::application_invalidated(), None, &[b"[network-service] application session invalidated\n".as_slice()]);
    case!("application abort zero", network::application_abort(0), None, &[b"[network-service] application aborted sessions_released=0\n".as_slice()]);
    case!("application abort one", network::application_abort(1), None, &[b"[network-service] application aborted sessions_released=1\n".as_slice()]);
    println!("producer actual-source cases={cases} failed={failures}");
    assert_eq!(failures, 0, "actual producers violated whole-record operation boundary");
}
"""

_CONTROLS = r"""
fn main() {
    let expected = b"[producer-control] complete=1\n";
    let positive = capture(|| { debug_write(expected); }, None);
    assert!(validate(&positive, &[expected.as_slice()]).is_ok());
    let split = capture(|| { debug_write(&expected[..7]); debug_write(&expected[7..]); }, None);
    assert!(validate(&split, &[expected.as_slice()]).unwrap_err().contains("fragmented record"));
    let lost = capture(|| { debug_write(&expected[..expected.len() - 1]); }, None);
    assert!(validate(&lost, &[expected.as_slice()]).unwrap_err().contains("whole-record bytes differ"));
    let oversized = capture(|| { debug_write(&[b'X'; 1025]); }, None);
    assert!(validate(&oversized, &[]).unwrap_err().contains("oversized record"));
    let invalid = capture(|| { debug_write(b"valid prefix\xff"); }, None);
    assert!(validate(&invalid, &[]).unwrap_err().contains("non-UTF-8 record"));
    // Every locally attributable service family has an independent whole-write
    // positive and split-write negative, not an expected string mined from Rust.
    let records: [&[u8]; 17] = [
        b"[network-service] loopback interface=127.0.0.1 external_nic=none\n",
        b"[network-service] wait wakes=7 coalesced=0\n",
        b"[network-service] loopback frames=40 rejected=0 handles=0 external_frames=0 resets=0 syns=2 fins=2\n",
        b"[network-service] authority destinations=1 rights=connect,send,recv\n",
        b"[network-service] declared socket_limit=1 listener_limit=0 dns_record_limit=0\n",
        b"[network-service] incarnation=1\n",
        b"[network-service] tcp options holder=unnamed keepalive_ms=0 nagle=1 hop_limit=64 idle_timeout_ms=10000\n",
        b"[network-service] tcp congestion control=reno\n",
        b"[network-service] tcp accepted close unread=0 unsent=0 terminal=closed handles=1 bytes=4096\n",
        b"[network-service] tcp listener closed children=0\n",
        b"[network-service] application bytes sent=4096 received=2048\n",
        b"[network-service] application bytes sent=2048 received=4096\n",
        b"[network-service] application buffers released\n",
        b"[network-service] observed requests=2 packets=0 socket_refusals=0 listener_refusals=0 dns_refusals=0 cross_holder_refusals=0\n",
        b"[network-service] fail: exam failure\n",
        b"[network-service] application session invalidated\n",
        b"[network-service] application aborted sessions_released=1\n",
    ];
    for record in records {
        let whole = capture(|| { debug_write(record); }, None);
        assert!(validate(&whole, &[record]).is_ok());
        let split = capture(|| { debug_write(&record[..19]); debug_write(&record[19..]); }, None);
        assert!(validate(&split, &[record]).unwrap_err().contains("fragmented record"));
    }
    println!("producer sensitivity controls passed=39");
}
"""
