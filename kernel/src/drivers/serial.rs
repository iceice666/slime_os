//! Serial diagnostic console.
//!
//! The transport is architecture-specific (`arch::target::uart`); this module
//! owns only the neutral console policy: one lock, CRLF line endings, and the
//! `serial_print!`/`serial_println!` formatting entry points.

use core::fmt::{self, Write};

use spin::{LazyLock, Mutex};

use crate::arch::target::uart;

static SERIAL1: LazyLock<Mutex<SerialPort>> = LazyLock::new(|| {
    uart::init();
    Mutex::new(SerialPort)
});

struct SerialPort;

impl SerialPort {
    fn write_byte(&mut self, byte: u8) {
        uart::write_byte(byte);
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
