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


def fixture_source(root: Path) -> str:
    console = (root / "slime-root/src/console.rs").read_text()
    start = console.index("fn write_payload(")
    brace = console.index("{", start)
    function = console[start:brace] + _balanced(console, brace, "{", "}")
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
        "@PRINTING@": str(root / "deps/rust-sel4/crates/sel4/src/printing.rs"),
        "@DIAGNOSTIC@": diagnostic_include,
        "@CONSOLE@": function,
        "@ROOT@": root_expression.strip(),
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
            "@DIRECT_OVERSIZE@",
            "assert!(diagnostic::write(&vec![b'Q'; 1025]).is_err()); assert!(diagnostic::write(b\"INVALID\\xff\").is_err());",
        )
    )
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
            elif result.returncode == 0:
                raise AssertionError(f"{name} mutation was accepted")
    return 7


# The frozen grader closure contains the full host adapter, not a mutable resource.
_TEMPLATE = r"""
// Host kernel-operation adapters; production formatting and record code are included below.
extern crate self as sel4;
use std::sync::{Mutex, Condvar, OnceLock};
use std::cell::RefCell;
pub type Word = usize;
pub struct IpcBuffer;
pub struct ScratchPage;
#[derive(Clone, Copy)] pub struct Window;
struct Frame(Vec<u8>);
impl Frame { fn bytes(&self) -> &[u8] { &self.0 } }
#[derive(Debug)] struct StagingError;
thread_local! { static INPUT: RefCell<Result<Vec<u8>, ()>> = const { RefCell::new(Err(())) }; static PRODUCER: RefCell<u8> = const { RefCell::new(0) }; }
mod transfer_window {
    use super::*;
    pub fn read_staged_array_with(window: Option<Window>, _: Word, _: &[Word], _: &ScratchPage, _: &mut IpcBuffer) -> Result<Frame, StagingError> {
        if window.is_none() { return Err(StagingError); }
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
fn root_record(phase: &str) { @ROOT@; }
fn console_record(bytes: &[u8], window: bool, words: &[Word]) {
    INPUT.with(|v| *v.borrow_mut() = Ok(bytes.to_vec()));
    write_payload(window.then_some(Window), words, &ScratchPage, &mut IpcBuffer);
}
fn main() {
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
    write_payload(Some(Window), &[0, 0], &ScratchPage, &mut IpcBuffer);
    assert_eq!(state().0.lock().unwrap().bytes, b"SLIME_ROOT console staging refused: StagingError\n");
    state().0.lock().unwrap().bytes.clear();
    for malformed in [false, true] {
    for console_first in [false, true] {
    for _round in 0..8 {
    { let mut s = state().0.lock().unwrap(); *s = State::default(); }
    let a = std::thread::spawn(move || { PRODUCER.with(|v| *v.borrow_mut() = 1); if console_first { console_record(if malformed { b"UNACCEPTED_RECORD\xff" } else { b"CONSOLE_RECORD complete\n" }, true, &[0, 0]); } else { root_record("exam"); } });
    let (lock, cv) = state();
    { let mut s = lock.lock().unwrap(); while !s.paused { s = cv.wait(s).unwrap(); } }
    let b = std::thread::spawn(move || { PRODUCER.with(|v| *v.borrow_mut() = 2); if console_first { root_record("exam"); } else { console_record(if malformed { b"UNACCEPTED_RECORD\xff" } else { b"CONSOLE_RECORD complete\n" }, true, &[0, 0]); } });
    { let mut s = lock.lock().unwrap(); while !s.contender { s = cv.wait(s).unwrap(); } s.release = true; cv.notify_all(); }
    a.join().unwrap(); b.join().unwrap();
    let output = lock.lock().unwrap().bytes.clone();
    let root = "SLIME_BACKING snapshot phase=exam begin\n";
    let console = if malformed { "SLIME_ROOT console refused non-utf8 bytes=18\n" } else { "CONSOLE_RECORD complete\n" };
    let expected = format!("{root}{console}").into_bytes();
    let reverse = format!("{console}{root}").into_bytes();
    assert!(output == expected || output == reverse, "accepted records spliced under forced mid-byte contention: {:?}", String::from_utf8_lossy(&output));
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
    println!("console actual-source deterministic contention passed");
}
"""
