"""Compile and run the actual root/console output paths with kernel-operation shims."""

from pathlib import Path
import subprocess
import tempfile


def _balanced(source: str, start: int, opening: str, closing: str) -> str:
    """Extract a Rust region, respecting strings and comments rather than regex braces."""
    depth = 0
    quoted = False
    escaped = False
    line_comment = False
    for index in range(start, len(source)):
        char = source[index]
        if line_comment:
            if char == "\n":
                line_comment = False
            continue
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
            continue
        if char == '"':
            quoted = True
        elif source[index : index + 2] == "//":
            line_comment = True
        elif char == opening:
            depth += 1
        elif char == closing:
            depth -= 1
            if depth == 0:
                return source[start : index + 1]
    raise ValueError("unterminated actual-source region")


def _function(source: str, selector: str) -> str:
    if source.count(selector) != 1:
        raise ValueError(f"actual-source selector is not unique: {selector}")
    start = source.index(selector)
    brace = source.index("{", start)
    return source[start:brace] + _balanced(source, brace, "{", "}")


def _dispatch_sources(root: Path) -> dict[str, str]:
    """Keep badge decoding, window routing, and the Write arm in production order."""
    console = (root / "slime-root/src/console.rs").read_text()
    serve = _function(console, "pub unsafe fn serve(")
    first = "        let Some((id, _)) = TaskId::from_badge(message.badge) else {"
    last = "            ipc::ConsoleKind::InputRead => {"
    if serve.count(first) != 1 or serve.count(last) != 1:
        raise ValueError("console dispatch source boundaries drifted")
    start, end = serve.index(first), serve.index(last)
    if start >= end:
        raise ValueError("console dispatch source ordering drifted")
    # The single-iteration loop preserves the extracted badge refusal's continue.
    dispatch = (
        "fn dispatch(message: Message, windows: &WindowTable, context: &ConsoleContext, "
        "buffer: &mut IpcBuffer) { for _ in 0..1 {\n"
        + serve[start:end]
        + "            _ => unreachable!(\"non-write host fixture message\"),\n"
        "        }\n    }\n}\n"
    )
    task = (root / "slime-root/src/task.rs").read_text()
    transfer = (root / "slime-root/src/transfer_window.rs").read_text()
    return {
        "@DISPATCH@": dispatch,
        "@BADGE@": _function(task, "pub const fn from_badge("),
        "@SERVICE_BADGE@": _function(task, "pub const fn service_badge("),
        "@BOUND@": _function(transfer, "pub fn bound("),
        "@THREAD@": _function(transfer, "pub const fn descriptor_thread("),
        "@ABI@": str(root / "boot-contracts/src/generated/component_runtime_abi.rs"),
    }


def fixture_source(root: Path) -> str:
    console = (root / "slime-root/src/console.rs").read_text()
    function = _function(console, "fn write_payload(")
    backing = (root / "slime-root/src/object_allocator/global_backing.rs").read_text()
    marker = backing.index('"SLIME_BACKING snapshot phase={phase} begin"')
    invocation_start = backing.rfind("\n", 0, marker) + 1
    invocation = backing[invocation_start:]
    opening = invocation.index("(")
    root_expression = invocation[:opening] + _balanced(invocation, opening, "(", ")")
    diagnostic = root / "slime-root/src/diagnostic.rs"
    diagnostic_include = f'#[path = "{diagnostic}"] mod diagnostic;' if diagnostic.exists() else ""
    template = _TEMPLATE
    for key, value in {
        **_dispatch_sources(root),
        "@PRINTING@": str(root / "deps/rust-sel4/crates/sel4/src/printing.rs"),
        "@DIAGNOSTIC@": diagnostic_include,
        "@CONSOLE@": function,
        "@ROOT@": root_expression.strip(),
        "@DIRECT_EXACT@": (
            'assert!(diagnostic::write(&vec![b\'Q\'; 1024]).is_ok(), "exact boundary direct write rejected"); assert_eq!(state().0.lock().unwrap().bytes, vec![b\'Q\'; 1024], "exact boundary direct write changed bytes"); state().0.lock().unwrap().bytes.clear();'
            if diagnostic.exists()
            else ""
        ),
        "@DIRECT_OVERSIZE@": (
            'assert!(diagnostic::write(&vec![b\'Q\'; 1025]).is_err(), "direct oversized record accepted"); assert!(diagnostic::write(b"INVALID\\xff").is_err(), "direct malformed record accepted");'
            if diagnostic.exists()
            else ""
        ),
    }.items():
        template = template.replace(key, value)
    return template


def run_exam(root: Path, timeout: int = 30) -> subprocess.CompletedProcess[str]:
    """A nonzero baseline result is expected; callers must never turn it into a pass."""
    root = root.resolve()
    with tempfile.TemporaryDirectory(prefix="console-record-exam-") as directory:
        source = Path(directory) / "exam.rs"
        binary = Path(directory) / "exam"
        source.write_text(fixture_source(root))
        compiled = subprocess.run(
            ["rustc", "--edition=2024", str(source), "-o", str(binary)],
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


# Synthetic subjects grade fixture sensitivity only; never substitute for product evidence.
_CONTROL_MODULE = r"""
mod diagnostic {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::fmt::Write;
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    static SERVING: AtomicUsize = AtomicUsize::new(0);
    pub fn write(bytes: &[u8]) -> Result<(), ()> {
        if bytes.len() > 1024 || std::str::from_utf8(bytes).is_err() { return Err(()); }
        let ticket = NEXT.fetch_add(1, Ordering::Relaxed);
        while SERVING.load(Ordering::Acquire) != ticket { crate::r#yield(); }
        for byte in bytes { crate::debug_put_char(*byte); }
        SERVING.store(ticket.wrapping_add(1), Ordering::Release);
        Ok(())
    }
    pub fn print(args: std::fmt::Arguments) {
        struct Buffer { bytes: [u8; 1024], len: usize }
        impl Write for Buffer {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                if text.len() > 1024 - self.len { return Err(std::fmt::Error); }
                self.bytes[self.len..self.len + text.len()].copy_from_slice(text.as_bytes());
                self.len += text.len(); Ok(())
            }
        }
        let mut buffer = Buffer { bytes: [0; 1024], len: 0 };
        if buffer.write_fmt(args).is_ok() { let _ = write(&buffer.bytes[..buffer.len]); }
    }
}
"""


def check_fixture_controls(root: Path) -> int:
    """Positive and malformed/refusal/serialization sensitivity controls."""
    root = root.resolve()
    # These deliberately synthetic subjects remain independent of repaired product APIs.
    source = (
        _TEMPLATE.replace("@PRINTING@", str(root / "deps/rust-sel4/crates/sel4/src/printing.rs"))
        .replace("@DIAGNOSTIC@", "")
        .replace("@ROOT@", 'control_println!("SLIME_BACKING snapshot phase={phase} begin")')
        .replace(
            "@DIRECT_EXACT@",
            'assert!(diagnostic::write(&vec![b\'Q\'; 1024]).is_ok(), "exact boundary direct write rejected"); assert_eq!(state().0.lock().unwrap().bytes, vec![b\'Q\'; 1024], "exact boundary direct write changed bytes"); state().0.lock().unwrap().bytes.clear();',
        )
        .replace(
            "@DIRECT_OVERSIZE@",
            "assert!(diagnostic::write(&vec![b'Q'; 1025]).is_err(), \"direct oversized record accepted\"); assert!(diagnostic::write(b\"INVALID\\xff\").is_err(), \"direct malformed record accepted\");",
        )
    )
    for key, value in _dispatch_sources(root).items():
        source = source.replace(key, value)
    source = source.replace(
        "@CONSOLE@",
        r"""
fn write_payload(window: Option<Window>, words: &[Word], scratch: &ScratchPage, buffer: &mut IpcBuffer) {
    if words.len() < 2 { return; }
    let frame = match transfer_window::read_staged_array_with(window, words[1], words, scratch, buffer) {
        Ok(frame) => frame,
        Err(error) => { control_println!("SLIME_ROOT console staging refused: {error:?}"); return; }
    };
    if let Ok(text) = std::str::from_utf8(frame.bytes()) { control_print!("{text}"); }
    else { control_println!("SLIME_ROOT console refused non-utf8 bytes={}", frame.bytes().len()); }
}
""",
    )
    macros = r"""
macro_rules! control_print { ($($arg:tt)*) => { diagnostic::print(format_args!($($arg)*)) }; }
macro_rules! control_println { ($($arg:tt)*) => { diagnostic::print(format_args!("{}\n", format_args!($($arg)*))) }; }
"""
    source = source.replace("fn write_payload(", macros + "\nfn write_payload(")
    for name in (
        "positive",
        "unserialized",
        "busywait",
        "invalid-bytes",
        "overflow-prefix",
        "missing-window-prefix",
        "short-message-prefix",
        "invalid-badge-routing",
        "cross-holder-routing",
        "wrong-thread-routing",
        "undersized-write",
        "undersized-formatter",
        "staging-refusal-bypass",
    ):
        module = _CONTROL_MODULE
        subject = source
        if name == "unserialized":
            module = module.replace(
                "while SERVING.load(Ordering::Acquire) != ticket { crate::r#yield(); }", ""
            )
        elif name == "busywait":
            module = module.replace("crate::r#yield();", "std::hint::spin_loop();")
        elif name == "invalid-bytes":
            module = module.replace(" || std::str::from_utf8(bytes).is_err()", "")
        elif name == "overflow-prefix":
            module = module.replace(
                "if buffer.write_fmt(args).is_ok()",
                'if buffer.write_fmt(args).is_err() { let _ = write(b"SLIME_BACKING snapshot phase="); } else',
            )
        elif name == "missing-window-prefix":
            subject = subject.replace(
                "if window.is_none() { return Err(StagingError); }",
                "if window.is_none() { crate::debug_put_char(b'M'); return Err(StagingError); }",
            )
        elif name == "short-message-prefix":
            subject = subject.replace(
                "if words.len() < 2 {", "if words.len() < 2 { crate::debug_put_char(b'M');"
            )
        elif name == "invalid-badge-routing":
            subject = subject.replace(
                "let Some((id, _)) = TaskId::from_badge(message.badge) else {\n"
                "            continue;\n        };",
                "let (id, _) = TaskId::from_badge(message.badge)"
                ".unwrap_or((TaskId(0), Arrival::Request));",
            )
        elif name == "cross-holder-routing":
            subject = subject.replace(
                "windows.bound(id, thread)", "windows.bound(TaskId(0), thread)"
            )
        elif name == "wrong-thread-routing":
            subject = subject.replace("windows.bound(id, thread)", "windows.bound(id, 0)")
        elif name == "undersized-write":
            module = module.replace("if bytes.len() > 1024", "if bytes.len() > 1023")
        elif name == "undersized-formatter":
            module = module.replace("if text.len() > 1024 - self.len", "if text.len() > 1023 - self.len")
        elif name == "staging-refusal-bypass":
            module = module.replace(
                "pub fn print(args: std::fmt::Arguments) {",
                'pub fn print(args: std::fmt::Arguments) {\n'
                '        let text = args.to_string();\n'
                '        if text.starts_with("SLIME_ROOT console staging refused:") {\n'
                '            for byte in text.bytes() { crate::debug_put_char(byte); }\n'
                '            return;\n'
                '        }',
            )
        with tempfile.TemporaryDirectory(prefix="console-exam-control-") as directory:
            rust = Path(directory) / "control.rs"
            binary = Path(directory) / "control"
            rust.write_text(module + subject)
            subprocess.run(
                ["rustc", "--edition=2024", str(rust), "-o", str(binary)],
                capture_output=True,
                text=True,
                check=True,
                timeout=30,
            )
            try:
                result = subprocess.run(
                    [str(binary)], capture_output=True, text=True, timeout=2, check=False
                )
            except subprocess.TimeoutExpired:
                if name != "busywait":
                    raise
                continue
            if name == "positive":
                if result.returncode:
                    raise AssertionError(result.stderr)
            elif name == "unserialized":
                if result.returncode == 0 or "accepted records spliced" not in result.stderr:
                    raise AssertionError("unserialized subject not refused for splicing")
            elif name == "busywait":
                raise AssertionError("busywait subject did not exhaust timeout")
            elif name in ("invalid-badge-routing", "cross-holder-routing", "wrong-thread-routing"):
                reason = (
                    "invalid badge leaked accepted bytes"
                    if name == "invalid-badge-routing"
                    else "unbound/guessed holder or thread leaked accepted bytes"
                )
                if result.returncode == 0 or reason not in result.stderr:
                    raise AssertionError(f"{name} mutation not refused for authorization")
            else:
                reason = {
                    "invalid-bytes": "direct malformed record accepted",
                    "overflow-prefix": "oversize formatting emitted a partial record",
                    "missing-window-prefix": "unbound/guessed holder or thread leaked accepted bytes",
                    "short-message-prefix": "short message leaked payload",
                    "undersized-write": "exact boundary direct write rejected",
                    "undersized-formatter": "exact boundary formatted root changed bytes",
                    "staging-refusal-bypass": "staging refusal records spliced",
                }[name]
                if result.returncode == 0 or reason not in result.stderr:
                    raise AssertionError(f"{name} mutation not refused for {reason}: {result.stderr}")
    return 13


# The frozen grader closure contains the full host adapter, not a mutable resource.
_TEMPLATE = r"""
// Host kernel-operation adapters; production formatting and record code are included below.
extern crate self as sel4;
use std::sync::{Mutex, Condvar, OnceLock};
use std::cell::RefCell;
pub type Word = u64;
pub type Badge = u64;
pub struct IpcBuffer;
pub struct ScratchPage;
extern crate self as boot_contracts;
#[path = "@ABI@"] pub mod component_runtime_abi;
#[derive(Clone, Copy, Debug, PartialEq)] pub struct TaskId(pub u32);
#[derive(Clone, Copy)] pub enum Arrival { Request, Fault }
impl TaskId { @BADGE@ @SERVICE_BADGE@ }
#[derive(Clone, Copy)] pub struct Window { task: TaskId, thread: usize }
struct WindowTable { entries: [Option<(Window, bool)>; 2] }
impl WindowTable { @BOUND@ }
@THREAD@
struct ConsoleContext { scratch: ScratchPage }
mod ipc { pub enum ConsoleKind { Write, Other } }
struct Message { badge: Badge, mrs: [Word; 4], len: usize, kind: ipc::ConsoleKind }
fn fixture_windows() -> WindowTable {
    WindowTable { entries: [
        Some((Window { task: TaskId(0), thread: 0 }, true)),
        Some((Window { task: TaskId(1), thread: 1 }, true)),
    ] }
}
fn staged_descriptor(thread: usize) -> Word {
    (component_runtime_abi::DESCRIPTOR_FORM_WINDOW << component_runtime_abi::DESCRIPTOR_FORM_SHIFT)
        | ((thread as u64) << component_runtime_abi::DESCRIPTOR_THREAD_SHIFT)
}
struct Frame(Vec<u8>);
impl Frame { fn bytes(&self) -> &[u8] { &self.0 } }
#[derive(Debug)] struct StagingError;
thread_local! { static INPUT: RefCell<Result<Vec<u8>, ()>> = const { RefCell::new(Err(())) }; static PRODUCER: RefCell<u8> = const { RefCell::new(0) }; }
mod transfer_window {
    use super::*;
    pub fn read_staged_array_with(window: Option<Window>, transfer: Word, _: &[Word], _: &ScratchPage, _: &mut IpcBuffer) -> Result<Frame, StagingError> {
        let form = (transfer >> component_runtime_abi::DESCRIPTOR_FORM_SHIFT) & 255;
        if form != component_runtime_abi::DESCRIPTOR_FORM_INLINE {
            if window.is_none() { return Err(StagingError); }
            // A map reads the selected root-held frame, never a caller-guessed address.
            let selected = window.unwrap();
            if selected.task != TaskId(0) || selected.thread != 0 {
                return Ok(Frame(b"SECOND_HOLDER_WINDOW\n".to_vec()));
            }
        }
        INPUT.with(|v| v.borrow().clone().and_then(|bytes| if bytes.len() <= 1024 { Ok(bytes) } else { Err(()) }).map(Frame).map_err(|_| StagingError))
    }
}
#[derive(Default)] struct State { bytes: Vec<u8>, paused: bool, release: bool, contender: bool, yields: usize }
static STATE: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();
fn state() -> &'static (Mutex<State>, Condvar) { STATE.get_or_init(|| (Mutex::new(State::default()), Condvar::new())) }
pub fn debug_put_char(byte: u8) {
    let producer = PRODUCER.with(|v| *v.borrow());
    let (lock, cv) = state(); let mut s = lock.lock().unwrap();
    s.bytes.push(byte); assert!(s.bytes.len() <= 32768, "byte budget exceeded");
    if producer == 1 && !s.paused { s.paused = true; cv.notify_all(); while !s.release { s = cv.wait(s).unwrap(); } }
    if producer == 2 { s.contender = true; cv.notify_all(); }
}
pub fn r#yield() {
    let (lock, cv) = state(); let mut s = lock.lock().unwrap();
    s.yields += 1; assert!(s.yields <= 100000, "yield budget exceeded");
    if PRODUCER.with(|v| *v.borrow()) == 2 {
        s.contender = true; cv.notify_all();
        while !s.release { s = cv.wait(s).unwrap(); }
    }
    drop(s); std::thread::yield_now();
}
#[path = "@PRINTING@"] mod printing;
pub mod _private { pub mod printing { pub use crate::printing::debug_print_helper; } }
pub mod sys { #[allow(non_snake_case)] pub fn seL4_DebugPutChar(c: u8) { crate::debug_put_char(c); } }
@DIAGNOSTIC@
@CONSOLE@
@DISPATCH@
fn root_record(phase: &str) { @ROOT@; }
fn console_message(bytes: &[u8], badge: Badge, words: &[Word]) {
    console_message_input(Some(bytes), badge, words);
}
fn console_message_input(bytes: Option<&[u8]>, badge: Badge, words: &[Word]) {
    INPUT.with(|v| *v.borrow_mut() = bytes.map(|b| b.to_vec()).ok_or(()));
    let mut mrs = [0; 4]; mrs[..words.len()].copy_from_slice(words);
    dispatch(Message { badge, mrs, len: words.len(), kind: ipc::ConsoleKind::Write },
        &fixture_windows(), &ConsoleContext { scratch: ScratchPage }, &mut IpcBuffer);
}
fn console_record(bytes: &[u8], window: bool, words: &[Word]) {
    let mut staged = words.to_vec();
    if staged.len() >= 2 { staged[1] = staged_descriptor(0); }
    console_message(bytes, TaskId(if window { 0 } else { 2 }).service_badge(), &staged);
}
fn raw_window() -> Window { Window { task: TaskId(0), thread: 0 } }
fn contention_console(case: usize) {
    match case {
        0 => console_record(b"CONSOLE_RECORD complete\n", true, &[0, 0]),
        1 => console_record(b"UNACCEPTED_RECORD\xff", true, &[0, 0]),
        2 => console_record(b"MUST_NOT_APPEAR", false, &[0, 0]),
        3 => console_message_input(None, TaskId(0).service_badge(), &[0, staged_descriptor(0)]),
        4 => console_record(&vec![b'X'; 1025], true, &[0, 0]),
        _ => unreachable!(),
    }
}
fn main() {
    // Authorization checks run through the actual serve prefix and Write arm.
    for badge in [0, 1] {
        for descriptor in [0, staged_descriptor(0)] {
            console_message(b"MUST_NOT_APPEAR", badge, &[0, descriptor]);
            assert!(state().0.lock().unwrap().bytes.is_empty(), "invalid badge leaked accepted bytes");
        }
    }
    for (badge, descriptor) in [
        (TaskId(2).service_badge(), staged_descriptor(0)), // unbound holder
        (TaskId(1).service_badge(), staged_descriptor(0)), // another holder's thread
        (TaskId(0).service_badge(), staged_descriptor(1)), // another task's thread
        (TaskId(0).service_badge(), staged_descriptor(usize::MAX)),
    ] {
        console_message(b"MUST_NOT_APPEAR", badge, &[TaskId(0).service_badge(), descriptor, 0xdeadbeef]);
        assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console staging refused: StagingError\n", "unbound/guessed holder or thread leaked accepted bytes");
        state().0.lock().unwrap().bytes.clear();
    }
    console_message(b"MUST_NOT_APPEAR", TaskId(1).service_badge(), &[0, staged_descriptor(1)]);
    assert_eq!(state().0.lock().unwrap().bytes, b"SECOND_HOLDER_WINDOW\n", "dispatch did not map the authenticated holder's frame");
    state().0.lock().unwrap().bytes.clear();
    // Inline has no mapping: endpoint possession, not window ownership, authorizes it.
    console_message(b"INLINE_VALID\n", TaskId(2).service_badge(), &[0, 0]);
    assert_eq!(state().0.lock().unwrap().bytes, b"INLINE_VALID\n");
    state().0.lock().unwrap().bytes.clear();
    // Raw missing-window coverage remains independent of the message dispatcher.
    INPUT.with(|v| *v.borrow_mut() = Ok(b"MUST_NOT_APPEAR".to_vec()));
    write_payload(None, &[0, staged_descriptor(0)], &ScratchPage, &mut IpcBuffer);
    assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console staging refused: StagingError\n");
    state().0.lock().unwrap().bytes.clear();
    // These controls exercise the actual console validation before contention.
    console_record(b"UNACCEPTED_RECORD\xff", true, &[0, 0]);
    assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console refused non-utf8 bytes=18\n");
    state().0.lock().unwrap().bytes.clear();
    console_record(b"MUST_NOT_APPEAR", false, &[0, 0]);
    assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console staging refused: StagingError\n");
    state().0.lock().unwrap().bytes.clear();
    console_record(b"MUST_NOT_APPEAR", true, &[0]);
    assert!(state().0.lock().unwrap().bytes.is_empty(), "short message leaked payload");
    INPUT.with(|v| *v.borrow_mut() = Err(()));
    write_payload(Some(raw_window()), &[0, staged_descriptor(0)], &ScratchPage, &mut IpcBuffer);
    assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console staging refused: StagingError\n");
    state().0.lock().unwrap().bytes.clear();
    // A valid atomic record may fill the whole byte limit without a newline.
    @DIRECT_EXACT@
    let prefix = "SLIME_BACKING snapshot phase=";
    let suffix = " begin\n";
    let phase = "R".repeat(1024 - prefix.len() - suffix.len());
    let exact_root = format!("{prefix}{phase}{suffix}").into_bytes();
    assert_eq!(exact_root.len(), 1024);
    root_record(&phase);
    assert_eq!(state().0.lock().unwrap().bytes, exact_root, "exact boundary formatted root changed bytes");
    state().0.lock().unwrap().bytes.clear();
    console_record(&vec![b'Q'; 1024], true, &[0, 0]);
    assert_eq!(state().0.lock().unwrap().bytes, vec![b'Q'; 1024], "exact boundary staged console changed bytes");
    state().0.lock().unwrap().bytes.clear();
    for case in 0..5 {
    for console_first in [false, true] {
    for _round in 0..8 {
    { let mut s = state().0.lock().unwrap(); *s = State::default(); }
    let a = std::thread::spawn(move || { PRODUCER.with(|v| *v.borrow_mut() = 1); if console_first { contention_console(case); } else { root_record("exam"); } });
    let (lock, cv) = state();
    { let mut s = lock.lock().unwrap(); while !s.paused { s = cv.wait(s).unwrap(); } }
    let b = std::thread::spawn(move || { PRODUCER.with(|v| *v.borrow_mut() = 2); if console_first { root_record("exam"); } else { contention_console(case); } });
    { let mut s = lock.lock().unwrap(); while !s.contender { s = cv.wait(s).unwrap(); } s.release = true; cv.notify_all(); }
    a.join().unwrap(); b.join().unwrap();
    let output = lock.lock().unwrap().bytes.clone();
    let root = "SLIME_BACKING snapshot phase=exam begin\n";
    let console = match case {
        0 => "CONSOLE_RECORD complete\n",
        1 => "SLIME_ROOT console refused non-utf8 bytes=18\n",
        _ => "SLIME_ROOT console staging refused: StagingError\n",
    };
    let expected = format!("{root}{console}").into_bytes();
    let reverse = format!("{console}{root}").into_bytes();
    let reason = if case >= 2 { "staging refusal records spliced" } else { "accepted records spliced" };
    assert!(output == expected || output == reverse, "{reason} under forced first-byte contention case={case} console_first={console_first}: {:?}", String::from_utf8_lossy(&output));
    }
    }
    }
    // Oversize rejection must occur in actual production code, not the staging shim.
    let lock = &state().0;
    let before = lock.lock().unwrap().bytes.len();
    @DIRECT_OVERSIZE@
    assert_eq!(lock.lock().unwrap().bytes.len(), before, "direct oversize refusal emitted bytes");
    console_record(&vec![b'X'; 1025], true, &[0, 0]);
    let after = lock.lock().unwrap().bytes.clone();
    assert_eq!(&after[before..], b"SLIME_ROOT console staging refused: StagingError\n", "oversize staging leaked payload");
    let before = after.len(); root_record(&"Z".repeat(1025));
    assert_eq!(lock.lock().unwrap().bytes.len(), before, "oversize formatting emitted a partial record");
    println!("console actual-source deterministic contention passed: 80 forced pairs (5 routes x 2 owner orders x 8 rounds), exact-1024 direct/console/root acceptance, 1025 refusals");
}
"""
