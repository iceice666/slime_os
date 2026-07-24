//! Framebuffer text console.
//!
//! Reads the boot handoff framebuffer and rasterizes text into it using
//! `noto-sans-mono-bitmap`.

use core::{fmt, ptr};
use font_constants::BACKUP_CHAR;
use noto_sans_mono_bitmap::{
    FontWeight, RasterHeight, RasterizedChar, get_raster, get_raster_width,
};
use spin::Mutex;

/// Additional vertical space between lines.
const LINE_SPACING: usize = 2;
/// Additional horizontal space between characters.
const LETTER_SPACING: usize = 0;
/// Padding from the border so the font does not touch the edges.
const BORDER_PADDING: usize = 1;

/// Constants for the usage of the [`noto_sans_mono_bitmap`] crate.
mod font_constants {
    use super::*;

    /// Height of each char raster. The font size is ~0.84% of this. Thus,
    /// this is the line height that enables multiple characters to be
    /// side-by-side and appear optically in one line in a natural way.
    pub const CHAR_RASTER_HEIGHT: RasterHeight = RasterHeight::Size32;

    /// The width of each single symbol of the mono space font.
    pub const CHAR_RASTER_WIDTH: usize = get_raster_width(FontWeight::Regular, CHAR_RASTER_HEIGHT);

    /// Backup character if a desired symbol is not available by the font.
    /// The '' character requires the feature "unicode-specials".
    pub const BACKUP_CHAR: char = '\u{FFFD}';

    pub const FONT_WEIGHT: FontWeight = FontWeight::Regular;
}

/// Returns the raster of the given char or the raster of [`font_constants::BACKUP_CHAR`].
fn get_char_raster(c: char) -> RasterizedChar {
    fn get(c: char) -> Option<RasterizedChar> {
        get_raster(
            c,
            font_constants::FONT_WEIGHT,
            font_constants::CHAR_RASTER_HEIGHT,
        )
    }
    get(c).unwrap_or_else(|| get(BACKUP_CHAR).expect("Should get raster of backup char."))
}

/// Byte order of a framebuffer pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// Bytes in memory: R, G, B, (X).
    Rgb,
    /// Bytes in memory: B, G, R, (X).
    Bgr,
    /// Grayscale, one byte per pixel packed into a u8 cell.
    U8,
}

/// Decoded geometry of a framebuffer, independent of the bootloader.
#[derive(Debug, Clone, Copy)]
pub struct FrameBufferInfo {
    pub width: usize,
    pub height: usize,
    /// Pixels per row (= `pitch / bytes_per_pixel`).
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub pixel_format: PixelFormat,
}

impl FrameBufferInfo {
    fn from_boot(fb: crate::boot::Framebuffer) -> Self {
        let bytes_per_pixel = (fb.bpp / 8) as usize;
        let pixel_format = if fb.memory_model == 1 {
            if fb.red_mask_shift == 0 {
                PixelFormat::Rgb
            } else {
                PixelFormat::Bgr
            }
        } else {
            PixelFormat::Rgb
        };
        Self {
            width: fb.width as usize,
            height: fb.height as usize,
            stride: (fb.pitch as usize) / bytes_per_pixel,
            bytes_per_pixel,
            pixel_format,
        }
    }
}

/// Allows logging text to a pixel-based framebuffer.
pub struct FrameBufferWriter {
    framebuffer: &'static mut [u8],
    info: FrameBufferInfo,
    x_pos: usize,
    y_pos: usize,
}

impl FrameBufferWriter {
    /// Creates a new logger that uses the given framebuffer.
    pub fn new(framebuffer: &'static mut [u8], info: FrameBufferInfo) -> Self {
        let mut logger = Self {
            framebuffer,
            info,
            x_pos: 0,
            y_pos: 0,
        };
        logger.clear();
        logger
    }

    fn newline(&mut self) {
        self.y_pos += font_constants::CHAR_RASTER_HEIGHT.val() + LINE_SPACING;
        self.carriage_return()
    }

    fn carriage_return(&mut self) {
        self.x_pos = BORDER_PADDING;
    }

    /// Erases all text on the screen. Resets `self.x_pos` and `self.y_pos`.
    pub fn clear(&mut self) {
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
        self.framebuffer.fill(0);
    }

    fn width(&self) -> usize {
        self.info.width
    }

    fn height(&self) -> usize {
        self.info.height
    }

    /// Writes a single char to the framebuffer. Takes care of special control characters, such as
    /// newlines and carriage returns.
    fn write_char(&mut self, c: char) {
        match c {
            '\n' => self.newline(),
            '\r' => self.carriage_return(),
            c => {
                let new_xpos = self.x_pos + font_constants::CHAR_RASTER_WIDTH;
                if new_xpos >= self.width() {
                    self.newline();
                }
                let new_ypos =
                    self.y_pos + font_constants::CHAR_RASTER_HEIGHT.val() + BORDER_PADDING;
                if new_ypos >= self.height() {
                    self.clear();
                }
                self.write_rendered_char(get_char_raster(c));
            }
        }
    }

    /// Prints a rendered char into the framebuffer.
    /// Updates `self.x_pos`.
    fn write_rendered_char(&mut self, rendered_char: RasterizedChar) {
        for (y, row) in rendered_char.raster().iter().enumerate() {
            for (x, byte) in row.iter().enumerate() {
                self.write_pixel(self.x_pos + x, self.y_pos + y, *byte);
            }
        }
        self.x_pos += rendered_char.width() + LETTER_SPACING;
    }

    fn write_pixel(&mut self, x: usize, y: usize, intensity: u8) {
        let pixel_offset = y * self.info.stride + x;
        let color = match self.info.pixel_format {
            PixelFormat::Rgb => [intensity, intensity, intensity / 2, 0],
            PixelFormat::Bgr => [intensity / 2, intensity, intensity, 0],
            PixelFormat::U8 => [if intensity > 200 { 0xf } else { 0 }, 0, 0, 0],
        };
        let bytes_per_pixel = self.info.bytes_per_pixel;
        let byte_offset = pixel_offset * bytes_per_pixel;
        self.framebuffer[byte_offset..(byte_offset + bytes_per_pixel)]
            .copy_from_slice(&color[..bytes_per_pixel]);
        let _ = unsafe { ptr::read_volatile(&self.framebuffer[byte_offset]) };
    }
}

unsafe impl Send for FrameBufferWriter {}
unsafe impl Sync for FrameBufferWriter {}

impl fmt::Write for FrameBufferWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}

pub static WRITER: Mutex<Option<FrameBufferWriter>> = Mutex::new(None);

/// Initialize the framebuffer console from the boot handoff.
pub fn init_framebuffer() {
    let fb = crate::boot::framebuffer();
    let info = FrameBufferInfo::from_boot(fb);
    let len = (fb.pitch as usize) * (fb.height as usize);
    let framebuffer = unsafe { core::slice::from_raw_parts_mut(fb.address as *mut u8, len) };
    let writer = FrameBufferWriter::new(framebuffer, info);
    *WRITER.lock() = Some(writer);
}

/// Write userspace console bytes to the visible framebuffer without allocating.
/// Invalid UTF-8 is rendered one byte at a time using the fallback glyph path.
pub fn write_bytes(bytes: &[u8]) {
    use core::fmt::Write;
    let mut writer = WRITER.lock();
    let Some(writer) = writer.as_mut() else {
        return;
    };
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match core::str::from_utf8(remaining) {
            Ok(text) => {
                let _ = writer.write_str(text);
                break;
            }
            Err(error) => {
                let valid = error.valid_up_to();
                if valid != 0 {
                    // SAFETY: `valid_up_to` is guaranteed to end at a UTF-8 boundary.
                    let text = unsafe { core::str::from_utf8_unchecked(&remaining[..valid]) };
                    let _ = writer.write_str(text);
                    remaining = &remaining[valid..];
                }
                if remaining.is_empty() {
                    break;
                }
                writer.write_char(char::from(remaining[0]));
                remaining = &remaining[1..];
            }
        }
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    if let Some(writer) = WRITER.lock().as_mut() {
        let _ = writer.write_fmt(args);
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::frame_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($fmt:expr) => ($crate::print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => ($crate::print!(
        concat!($fmt, "\n"), $($arg)*));
}
