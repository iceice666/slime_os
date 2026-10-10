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

from console_record_exam import _balanced


_PROBE = "components/testkit/io-local-network-probe/src/main.rs"
_NETWORK = "components/services/network-service/src/main.rs"


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
    network_regions = [
        _function(network, "fail"),
        _output_block(constructor, 'b"[network-service] loopback interface='),
        _output_block(_function(network, "main"), 'b"[network-service] wait wakes="'),
        _output_block(_function(network, "main"), 'b"[network-service] loopback frames="'),
    ]
    for source, regions, prefixes in (
        (probe, probe_regions, ("[io-local-network-probe]",)),
        (
            network,
            network_regions,
            (
                "[network-service] loopback interface=",
                "[network-service] loopback frames=",
                "[network-service] wait wakes=",
                "[network-service] fail:",
            ),
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
    line = root / "components/lib/src/console_line.rs"
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
            for name in ("write_number", "fail")
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
    """Five operation controls and three actual-source lexical route mutations."""
    root = root.resolve()
    audit_producer_routes(root)
    for mutation in ("detached-marker", "parallel-qualified-prefix", "network-prefix-decoy"):
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
                elif mutation == "network-prefix-decoy":
                    source += '\nfn decoy() { debug_write(b"[network-service] loopback interface=127.0.0.1 external_nic=none\\n"); }\n'
                destination.write_text(source)
            try:
                audit_producer_routes(subject)
            except ValueError as error:
                reason = (
                    "actual marker caller route changed"
                    if mutation == "detached-marker"
                    else "qualified producer prefix outside"
                )
                if reason not in str(error):
                    raise AssertionError(
                        f"route mutation failed for unrelated reason: {error}"
                    ) from error
            else:
                raise AssertionError(f"route mutation was accepted: {mutation}")
    result = _run(_ADAPTER + _VALIDATOR + _CONTROLS, 30)
    if result.returncode:
        raise AssertionError(f"producer sensitivity controls failed:\n{result.stderr}")
    if result.stdout.strip() != "producer sensitivity controls passed=5":
        raise AssertionError("producer controls did not report their frozen case count")
    return 8


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
    impl Engine { fn allocated(&self) -> usize { self.handles } }
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
    println!("producer sensitivity controls passed=5");
}
"""
