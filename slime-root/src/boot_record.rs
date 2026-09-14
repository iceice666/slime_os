//! The bounded readiness record P6.6 observes on a physical panel.
//!
//! On every emulated plane the ordered marker chain arrives over COM1 and a
//! gate reads it from a transcript. A Framework 13 has no serial port, so the
//! same facts have to be legible on the display: the target profile the
//! generation was qualified for, the generation identity prefix, the mode the
//! firmware programmed, and a terminal line that distinguishes "reached the
//! end deliberately" from "stopped somewhere".
//!
//! Rendering is the *additional* channel, not a replacement. The serial
//! markers are still emitted, unchanged, so one root build produces evidence
//! on both channels and the QEMU planes keep reading exactly what they read
//! before.

use crate::framebuffer::{Framebuffer, FramebufferError};
use crate::object_allocator::ObjectAllocator;

/// Longest record line rendered, in characters.
///
/// Bounds the formatting buffers below. The renderer additionally truncates to
/// what the mode can display, so this is a source bound rather than a claim
/// about the panel.
const MAX_LINE: usize = 64;

/// A fixed-capacity line builder.
///
/// The root has no allocator for text and `core::fmt` into a stack buffer would
/// still need a sink; this is that sink, with truncation instead of failure so
/// a long field cannot suppress a whole evidence line.
struct Line {
    bytes: [u8; MAX_LINE],
    len: usize,
}

impl Line {
    const fn new() -> Self {
        Self {
            bytes: [b' '; MAX_LINE],
            len: 0,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.len < MAX_LINE {
            self.bytes[self.len] = byte;
            self.len += 1;
        }
    }

    fn text(&mut self, value: &str) {
        for byte in value.as_bytes().iter().copied() {
            self.push(byte);
        }
    }

    /// Append `value` in decimal.
    fn decimal(&mut self, value: u64) {
        // 20 digits covers u64::MAX.
        let mut digits = [0u8; 20];
        let mut count = 0;
        let mut remaining = value;
        loop {
            digits[count] = b'0' + (remaining % 10) as u8;
            count += 1;
            remaining /= 10;
            if remaining == 0 {
                break;
            }
        }
        while count > 0 {
            count -= 1;
            self.push(digits[count]);
        }
    }

    /// Append `value` as lowercase hexadecimal, no prefix.
    fn hex_byte(&mut self, value: u8) {
        const DIGITS: &[u8; 16] = b"0123456789ABCDEF";
        self.push(DIGITS[usize::from(value >> 4)]);
        self.push(DIGITS[usize::from(value & 0xf)]);
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// Render the readiness record, then leave it on screen.
///
/// Every field is one the serial chain already carries, so the panel and the
/// transcript cannot disagree about what booted.
pub fn render_ready(
    framebuffer: &mut Framebuffer,
    allocator: &mut ObjectAllocator,
    target_profile: &str,
    generation_number: u64,
    generation_identity: &[u8],
) -> Result<(), FramebufferError> {
    let mut line = Line::new();
    line.text("SLIME OS");
    framebuffer.write_line(allocator, line.as_slice())?;

    let mut line = Line::new();
    line.text("TARGET ");
    line.text(target_profile);
    framebuffer.write_line(allocator, line.as_slice())?;

    let mut line = Line::new();
    line.text("GENERATION ");
    line.decimal(generation_number);
    line.text(" ID ");
    // The same eight-byte prefix the `SLIME_GRAPH healthy` marker prints, so
    // the panel and the transcript name one generation in one notation.
    for byte in generation_identity.iter().copied().take(8) {
        line.hex_byte(byte);
    }
    framebuffer.write_line(allocator, line.as_slice())?;

    let mut line = Line::new();
    let info = *framebuffer.info();
    line.text("DISPLAY ");
    line.decimal(info.width() as u64);
    line.text("X");
    line.decimal(info.height() as u64);
    line.text("X");
    line.decimal(info.bits_per_pixel() as u64);
    framebuffer.write_line(allocator, line.as_slice())?;

    let mut line = Line::new();
    line.text("ROOT READY");
    framebuffer.write_line(allocator, line.as_slice())?;

    Ok(())
}

/// Render one boot-stage marker as the next panel line.
///
/// The panel is the whole evidence channel on a machine with no serial port,
/// and a record rendered only at readiness says nothing about a boot that
/// stops earlier — the screen simply keeps whatever the bootloader left, which
/// is indistinguishable from a hang in the bootloader itself. Each stage
/// therefore reports as it passes, so the last line on the panel names the
/// furthest point reached.
///
/// Downward by construction: `write_line` only ever advances, which is also
/// what keeps the framebuffer's granule walk monotonic.
pub fn render_stage(
    framebuffer: &mut Framebuffer,
    allocator: &mut ObjectAllocator,
    label: &str,
) -> Result<(), FramebufferError> {
    let mut line = Line::new();
    line.text(label);
    framebuffer.write_line(allocator, line.as_slice())
}

/// Render the terminal line and release the scratch window.
///
/// The graph stays resident: P6.5 proved a product graph that reaches `slisp>`
/// and keeps serving, and P6.6 must boot those exact bytes rather than a
/// variant that exits. So the terminal state is "the record is complete and
/// the machine is idle", which is also the state an operator can safely cut
/// power from — no storage is mounted and the medium is read-only.
///
/// Powering off here would be worse than useless: it would clear the panel
/// that carries the only evidence this boot produced, and a cleared display is
/// indistinguishable from one that never rendered.
///
/// Non-interactive and bounded — it takes no input, waits for nothing, and
/// writes a fixed number of pixels.
pub fn render_idle(
    framebuffer: &mut Framebuffer,
    allocator: &mut ObjectAllocator,
) -> Result<(), FramebufferError> {
    let mut line = Line::new();
    line.text("IDLE - SAFE TO POWER OFF");
    framebuffer.write_line(allocator, line.as_slice())?;
    // The record is shorter than the panel, and the rows past it still hold
    // whatever the bootloader drew. Cleared here because this is the last
    // line: the granule walk only moves forward, so nothing may return to
    // them afterwards.
    framebuffer.clear_tail(allocator)?;
    framebuffer.release()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(line: &Line) -> &str {
        core::str::from_utf8(line.as_slice()).expect("ASCII")
    }

    #[test]
    fn decimal_writes_digits_in_order() {
        let mut line = Line::new();
        line.decimal(0);
        line.push(b' ');
        line.decimal(800);
        line.push(b' ');
        line.decimal(u64::from(u32::MAX));
        assert_eq!(rendered(&line), "0 800 4294967295");
    }

    #[test]
    fn hex_byte_writes_two_uppercase_digits() {
        let mut line = Line::new();
        line.hex_byte(0x0f);
        line.hex_byte(0xa5);
        assert_eq!(rendered(&line), "0FA5");
    }

    #[test]
    fn a_line_truncates_instead_of_overflowing() {
        // A long field must cost its own tail, never a panic or a write past
        // the buffer: the record is evidence, and a panic here would replace it
        // with a fault.
        let mut line = Line::new();
        for _ in 0..MAX_LINE * 2 {
            line.text("AB");
        }
        assert_eq!(line.as_slice().len(), MAX_LINE);
    }

    #[test]
    fn text_and_numbers_compose_in_field_order() {
        let mut line = Line::new();
        line.text("DISPLAY ");
        line.decimal(800);
        line.text("X");
        line.decimal(600);
        line.text("X");
        line.decimal(24);
        assert_eq!(rendered(&line), "DISPLAY 800X600X24");
    }
}
