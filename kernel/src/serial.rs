use core::fmt::{self, Write};

use spin::{LazyLock, Mutex};

const COM1: u16 = 0x3F8;
const INTERRUPT_ENABLE: u16 = COM1 + 1;
const FIFO_CONTROL: u16 = COM1 + 2;
const LINE_CONTROL: u16 = COM1 + 3;
const MODEM_CONTROL: u16 = COM1 + 4;
const LINE_STATUS: u16 = COM1 + 5;
const TRANSMITTER_EMPTY: u8 = 1 << 5;
const DLAB: u8 = 1 << 7;
const SERIAL_SPINS: usize = 100_000;

static SERIAL1: LazyLock<Mutex<SerialPort>> = LazyLock::new(|| {
    let mut serial = SerialPort;
    serial.init();
    Mutex::new(serial)
});

struct SerialPort;

impl SerialPort {
    fn init(&mut self) {
        // 38400 baud, 8 data bits, no parity, one stop bit. Every wait below is
        // bounded so a Framework with no legacy COM1 UART cannot hang bring-up.
        unsafe {
            outb(INTERRUPT_ENABLE, 0x00);
            outb(LINE_CONTROL, DLAB);
            outb(COM1, 0x03);
            outb(INTERRUPT_ENABLE, 0x00);
            outb(LINE_CONTROL, 0x03);
            outb(FIFO_CONTROL, 0xC7);
            outb(MODEM_CONTROL, 0x0B);
        }
    }

    fn write_byte(&mut self, byte: u8) {
        if self.wait_transmitter_empty() {
            unsafe { outb(COM1, byte) };
        }
    }

    fn wait_transmitter_empty(&self) -> bool {
        for _ in 0..SERIAL_SPINS {
            if unsafe { inb(LINE_STATUS) } & TRANSMITTER_EMPTY != 0 {
                return true;
            }
        }
        false
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
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

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    let _ = SERIAL1.lock().write_fmt(args);
}

pub fn write_bytes(bytes: &[u8]) {
    let mut serial = SERIAL1.lock();
    for byte in bytes {
        serial.write_byte(*byte);
    }
}

/// Prints to the host through the serial interface.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::serial::_print(format_args!($($arg)*))
    };
}

/// Prints to the host through the serial interface, appending a newline.
#[macro_export]
macro_rules! serial_println {
    () => ($crate::serial_print!("\n"));
    ($fmt:expr) => ($crate::serial_print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::serial_print!(
        concat!($fmt, "\n"), $($arg)*));
}
