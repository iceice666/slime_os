//! Interrupt-driven i8042 keyboard input for early Framework bring-up.
//!
//! The driver owns only device mechanism: bounded controller setup, scan-code
//! decoding, and a fixed-size queue. Console focus and key-binding policy stay
//! in userspace once the input service exists.

use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use crate::acpi::MadtInfo;
use crate::serial_println;
use crate::time::apic::RouteError;

const STATUS_PORT: u16 = 0x64;
const DATA_PORT: u16 = 0x60;
const STATUS_OUTPUT_FULL: u8 = 1 << 0;
const STATUS_INPUT_FULL: u8 = 1 << 1;
const CONTROLLER_SPINS: usize = 100_000;
const QUEUE_CAPACITY: usize = 128;

static KEYBOARD_PRESENT: AtomicBool = AtomicBool::new(false);
static QUEUE: Mutex<KeyQueue> = Mutex::new(KeyQueue::new());
static DECODER: Mutex<ScanDecoder> = Mutex::new(ScanDecoder::new());
static SCRIPT: Mutex<ScriptInput> = Mutex::new(ScriptInput::new());
const MAX_INIT_STAGES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputPath {
    I8042,
    UsbHid,
    FirmwareOther,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputStage {
    FirmwareHint,
    PortsDisabled,
    OutputDrained,
    ConfigRead,
    ControllerSelfTest,
    FirstPortEnabled,
    KeyboardResetAck,
    KeyboardSelfTest,
    ScanningEnabled,
    InterruptRouted,
    Online,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputStageRecord {
    pub stage: InputStage,
    pub error: Option<InputError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputInitReport {
    pub path: InputPath,
    pub stages: [Option<InputStageRecord>; MAX_INIT_STAGES],
    pub len: usize,
}

impl InputInitReport {
    const fn new(path: InputPath) -> Self {
        Self {
            path,
            stages: [None; MAX_INIT_STAGES],
            len: 0,
        }
    }

    fn push(&mut self, stage: InputStage, error: Option<InputError>) {
        if let Some(slot) = self.stages.get_mut(self.len) {
            *slot = Some(InputStageRecord { stage, error });
            self.len += 1;
        }
    }

    pub fn result(&self) -> Result<(), InputError> {
        self.stages[..self.len]
            .iter()
            .flatten()
            .find_map(|record| record.error)
            .map_or(Ok(()), Err)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    ControllerTimeout,
    ControllerSelfTestFailed(u8),
    KeyboardResetFailed(u8),
    RouteMissingIoApic,
    RouteGsiOutOfRange,
    RouteMapFailed,
    ControllerNotImplemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Escape,
    Backspace,
    Tab,
    Enter,
    LeftControl,
    LeftShift,
    RightShift,
    LeftAlt,
    Space,
    Up,
    Down,
    Left,
    Right,
    Character(char),
    Unknown(u16),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: KeyCode,
    pub pressed: bool,
}

pub fn init(madt: &MadtInfo, i8042_present: bool) -> Result<(), InputError> {
    init_with_report(madt, i8042_present, false).result()
}

pub fn init_with_report(
    madt: &MadtInfo,
    i8042_present: bool,
    usb_controller_present: bool,
) -> InputInitReport {
    let mut report = InputInitReport::new(if i8042_present {
        InputPath::I8042
    } else if usb_controller_present {
        InputPath::UsbHid
    } else {
        InputPath::FirmwareOther
    });
    report.push(InputStage::FirmwareHint, None);
    if !i8042_present {
        report.push(
            InputStage::Failed,
            Some(InputError::ControllerNotImplemented),
        );
        return report;
    }

    unsafe { outb(STATUS_PORT, 0xad) };
    unsafe { outb(STATUS_PORT, 0xa7) };
    report.push(InputStage::PortsDisabled, None);
    drain_output();
    report.push(InputStage::OutputDrained, None);

    let mut config = match read_controller_config() {
        Ok(config) => config,
        Err(error) => {
            report.push(InputStage::ConfigRead, Some(error));
            return report;
        }
    };
    config &= !0x43;
    if let Err(error) = write_controller_config(config) {
        report.push(InputStage::ConfigRead, Some(error));
        return report;
    }
    report.push(InputStage::ConfigRead, None);

    if let Err(error) = command(0xaa) {
        report.push(InputStage::ControllerSelfTest, Some(error));
        return report;
    }
    let self_test = match read_data() {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::ControllerSelfTest, Some(error));
            return report;
        }
    };
    if self_test != 0x55 {
        report.push(
            InputStage::ControllerSelfTest,
            Some(InputError::ControllerSelfTestFailed(self_test)),
        );
        return report;
    }
    report.push(InputStage::ControllerSelfTest, None);

    let port_result = (|| {
        command(0xae)?;
        config = read_controller_config()?;
        config |= 0x41;
        config &= !0x10;
        write_controller_config(config)
    })();
    if let Err(error) = port_result {
        report.push(InputStage::FirstPortEnabled, Some(error));
        return report;
    }
    report.push(InputStage::FirstPortEnabled, None);

    let reset_ack = (|| {
        write_data(0xff)?;
        read_data()
    })();
    let ack = match reset_ack {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::KeyboardResetAck, Some(error));
            return report;
        }
    };
    if ack != 0xfa {
        report.push(
            InputStage::KeyboardResetAck,
            Some(InputError::KeyboardResetFailed(ack)),
        );
        return report;
    }
    report.push(InputStage::KeyboardResetAck, None);

    let reset = match read_data() {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::KeyboardSelfTest, Some(error));
            return report;
        }
    };
    if reset != 0xaa {
        report.push(
            InputStage::KeyboardSelfTest,
            Some(InputError::KeyboardResetFailed(reset)),
        );
        return report;
    }
    report.push(InputStage::KeyboardSelfTest, None);

    let enable_ack = (|| {
        write_data(0xf4)?;
        read_data()
    })();
    let enable = match enable_ack {
        Ok(value) => value,
        Err(error) => {
            report.push(InputStage::ScanningEnabled, Some(error));
            return report;
        }
    };
    if enable != 0xfa {
        report.push(
            InputStage::ScanningEnabled,
            Some(InputError::KeyboardResetFailed(enable)),
        );
        return report;
    }
    report.push(InputStage::ScanningEnabled, None);

    if let Err(error) =
        crate::time::apic::route_external_irq(madt, 1, crate::interrupts::KEYBOARD_VECTOR)
    {
        let error = match error {
            RouteError::MissingIoApic => InputError::RouteMissingIoApic,
            RouteError::GsiOutOfRange => InputError::RouteGsiOutOfRange,
            RouteError::Map(_) => InputError::RouteMapFailed,
        };
        report.push(InputStage::InterruptRouted, Some(error));
        return report;
    }
    report.push(InputStage::InterruptRouted, None);
    KEYBOARD_PRESENT.store(true, Ordering::Release);
    report.push(InputStage::Online, None);
    serial_println!("[input] i8042 keyboard online");
    report
}

pub fn present() -> bool {
    KEYBOARD_PRESENT.load(Ordering::Acquire)
}

pub fn pop_event() -> Option<KeyEvent> {
    without_interrupts(|| QUEUE.lock().pop())
}

pub fn install_script(input: &'static [u8]) {
    *SCRIPT.lock() = ScriptInput { input, cursor: 0 };
}

pub fn pump_script() {
    let next = {
        let mut script = SCRIPT.lock();
        let Some(byte) = script.input.get(script.cursor).copied() else {
            return;
        };
        script.cursor += 1;
        byte
    };
    let code = match next {
        0x1b => KeyCode::Escape,
        b' ' => KeyCode::Space,
        b'\n' => KeyCode::Enter,
        byte if byte.is_ascii() => KeyCode::Character(byte as char),
        _ => return,
    };
    without_interrupts(|| {
        let mut queue = QUEUE.lock();
        if queue.len < QUEUE_CAPACITY {
            queue.push(KeyEvent {
                code,
                pressed: true,
            });
        }
    });
}

struct ScriptInput {
    input: &'static [u8],
    cursor: usize,
}

impl ScriptInput {
    const fn new() -> Self {
        Self {
            input: &[],
            cursor: 0,
        }
    }
}

pub(crate) fn on_interrupt() {
    let status = unsafe { inb(STATUS_PORT) };
    if status & STATUS_OUTPUT_FULL != 0 {
        let scancode = unsafe { inb(DATA_PORT) };
        if let Some(event) = DECODER.lock().feed(scancode) {
            QUEUE.lock().push(event);
        }
    }
}

struct KeyQueue {
    events: [Option<KeyEvent>; QUEUE_CAPACITY],
    head: usize,
    len: usize,
}

impl KeyQueue {
    const fn new() -> Self {
        Self {
            events: [None; QUEUE_CAPACITY],
            head: 0,
            len: 0,
        }
    }

    fn push(&mut self, event: KeyEvent) {
        if self.len == QUEUE_CAPACITY {
            self.head = (self.head + 1) % QUEUE_CAPACITY;
            self.len -= 1;
        }
        let tail = (self.head + self.len) % QUEUE_CAPACITY;
        self.events[tail] = Some(event);
        self.len += 1;
    }

    fn pop(&mut self) -> Option<KeyEvent> {
        if self.len == 0 {
            return None;
        }
        let event = self.events[self.head].take();
        self.head = (self.head + 1) % QUEUE_CAPACITY;
        self.len -= 1;
        event
    }
}

struct ScanDecoder {
    extended: bool,
    left_shift: bool,
    right_shift: bool,
}

impl ScanDecoder {
    const fn new() -> Self {
        Self {
            extended: false,
            left_shift: false,
            right_shift: false,
        }
    }

    fn feed(&mut self, byte: u8) -> Option<KeyEvent> {
        if byte == 0xe0 {
            self.extended = true;
            return None;
        }
        if byte == 0xe1 {
            self.extended = false;
            return None;
        }

        let released = byte & 0x80 != 0;
        let code = byte & 0x7f;
        let extended = core::mem::take(&mut self.extended);
        if !extended {
            match code {
                0x2a => self.left_shift = !released,
                0x36 => self.right_shift = !released,
                _ => {}
            }
        }
        let shift = self.left_shift || self.right_shift;
        Some(KeyEvent {
            code: decode_key(code, extended, shift),
            pressed: !released,
        })
    }
}

fn decode_key(code: u8, extended: bool, shift: bool) -> KeyCode {
    if extended {
        return match code {
            0x1c => KeyCode::Enter,
            0x1d => KeyCode::LeftControl,
            0x35 => KeyCode::Character('/'),
            0x38 => KeyCode::LeftAlt,
            0x48 => KeyCode::Up,
            0x4b => KeyCode::Left,
            0x4d => KeyCode::Right,
            0x50 => KeyCode::Down,
            _ => KeyCode::Unknown(0xe000 | code as u16),
        };
    }

    match code {
        0x01 => KeyCode::Escape,
        0x0e => KeyCode::Backspace,
        0x0f => KeyCode::Tab,
        0x1c => KeyCode::Enter,
        0x1d => KeyCode::LeftControl,
        0x2a => KeyCode::LeftShift,
        0x36 => KeyCode::RightShift,
        0x38 => KeyCode::LeftAlt,
        0x39 => KeyCode::Space,
        _ => decode_character(code, shift)
            .map(KeyCode::Character)
            .unwrap_or(KeyCode::Unknown(code as u16)),
    }
}

fn decode_character(code: u8, shift: bool) -> Option<char> {
    let pair = match code {
        0x02 => ('1', '!'),
        0x03 => ('2', '@'),
        0x04 => ('3', '#'),
        0x05 => ('4', '$'),
        0x06 => ('5', '%'),
        0x07 => ('6', '^'),
        0x08 => ('7', '&'),
        0x09 => ('8', '*'),
        0x0a => ('9', '('),
        0x0b => ('0', ')'),
        0x0c => ('-', '_'),
        0x0d => ('=', '+'),
        0x10 => ('q', 'Q'),
        0x11 => ('w', 'W'),
        0x12 => ('e', 'E'),
        0x13 => ('r', 'R'),
        0x14 => ('t', 'T'),
        0x15 => ('y', 'Y'),
        0x16 => ('u', 'U'),
        0x17 => ('i', 'I'),
        0x18 => ('o', 'O'),
        0x19 => ('p', 'P'),
        0x1a => ('[', '{'),
        0x1b => (']', '}'),
        0x1e => ('a', 'A'),
        0x1f => ('s', 'S'),
        0x20 => ('d', 'D'),
        0x21 => ('f', 'F'),
        0x22 => ('g', 'G'),
        0x23 => ('h', 'H'),
        0x24 => ('j', 'J'),
        0x25 => ('k', 'K'),
        0x26 => ('l', 'L'),
        0x27 => (';', ':'),
        0x28 => ('\'', '"'),
        0x29 => ('`', '~'),
        0x2b => ('\\', '|'),
        0x2c => ('z', 'Z'),
        0x2d => ('x', 'X'),
        0x2e => ('c', 'C'),
        0x2f => ('v', 'V'),
        0x30 => ('b', 'B'),
        0x31 => ('n', 'N'),
        0x32 => ('m', 'M'),
        0x33 => (',', '<'),
        0x34 => ('.', '>'),
        0x35 => ('/', '?'),
        _ => return None,
    };
    Some(if shift { pair.1 } else { pair.0 })
}

fn read_controller_config() -> Result<u8, InputError> {
    command(0x20)?;
    read_data()
}

fn write_controller_config(config: u8) -> Result<(), InputError> {
    command(0x60)?;
    write_data(config)
}

fn command(value: u8) -> Result<(), InputError> {
    wait_input_empty()?;
    unsafe { outb(STATUS_PORT, value) };
    Ok(())
}

fn write_data(value: u8) -> Result<(), InputError> {
    wait_input_empty()?;
    unsafe { outb(DATA_PORT, value) };
    Ok(())
}

fn read_data() -> Result<u8, InputError> {
    for _ in 0..CONTROLLER_SPINS {
        if unsafe { inb(STATUS_PORT) } & STATUS_OUTPUT_FULL != 0 {
            return Ok(unsafe { inb(DATA_PORT) });
        }
    }
    Err(InputError::ControllerTimeout)
}

fn wait_input_empty() -> Result<(), InputError> {
    for _ in 0..CONTROLLER_SPINS {
        if unsafe { inb(STATUS_PORT) } & STATUS_INPUT_FULL == 0 {
            return Ok(());
        }
    }
    Err(InputError::ControllerTimeout)
}

fn drain_output() {
    for _ in 0..64 {
        if unsafe { inb(STATUS_PORT) } & STATUS_OUTPUT_FULL == 0 {
            return;
        }
        let _ = unsafe { inb(DATA_PORT) };
    }
}

fn without_interrupts<T>(f: impl FnOnce() -> T) -> T {
    let flags: u64;
    unsafe {
        core::arch::asm!("pushfq", "pop {}", out(reg) flags, options(nomem, preserves_flags));
        core::arch::asm!("cli", options(nomem, nostack, preserves_flags));
    }
    let result = f();
    if flags & (1 << 9) != 0 {
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
    result
}

unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") value,
            options(nomem, nostack, preserves_flags),
        );
    }
}

unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!(
            "in al, dx",
            out("al") value,
            in("dx") port,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}
