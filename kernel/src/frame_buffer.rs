use alloc::vec::Vec;
use bootloader_api::{info::FrameBuffer, info::FrameBufferInfo};
use core::fmt::{self, Write};
use fontdue::layout::{CoordinateSystem, GlyphPosition, Layout, LayoutSettings, TextStyle};
use fontdue::{Font, FontSettings};
use spin::{Lazy, Mutex};
use volatile::{VolatilePtr, VolatileRef};

const LETTER_SPACING: usize = 1;
const LINE_SPACING: usize = 2;
const BORDER_PADDING: usize = 1;
const FONT_SIZE: f32 = 16.0;

// Color constants
const WHITE: u32 = 0xFFFFFF;
const BLACK: u32 = 0x000000;
const GRAY: u32 = 0x808080;

const RAW_FONT: &[u8] = include_bytes!("../../assets/ter-u16n.otb");
static FONT: Lazy<Font> = Lazy::new(|| {
    Font::from_bytes(RAW_FONT, FontSettings::default())
        .unwrap_or_else(|err| panic!("Unable to initialize font: {}", err))
});

pub struct FrameBufferWriter {
    x_pos: usize,
    y_pos: usize,
    buffer: VolatileRef<'static, [u8]>,
    info: FrameBufferInfo,
    layout: Layout,
    background_color: u32,
    foreground_color: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum FrameBufferError {
    InvalidPosition,
    BufferTooSmall,
}

impl FrameBufferWriter {
    pub fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let buffer_info = framebuffer.info();
        let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
        layout.reset(&LayoutSettings {
            x: BORDER_PADDING as f32,
            y: BORDER_PADDING as f32,
            max_width: Some((buffer_info.width - 2 * BORDER_PADDING) as f32),
            max_height: Some((buffer_info.height - 2 * BORDER_PADDING) as f32),
            line_height: 1.2,
            ..LayoutSettings::default()
        });

        // Create a VolatileRef from the framebuffer buffer
        let buffer_ptr = VolatileRef::from_mut_ref(framebuffer.buffer_mut());

        let mut this = Self {
            x_pos: BORDER_PADDING,
            y_pos: BORDER_PADDING,
            buffer: buffer_ptr,
            info: buffer_info,
            layout,
            background_color: BLACK,
            foreground_color: WHITE,
        };
        this.clear();
        this
    }

    /// Clear the entire screen with the background color
    pub fn clear(&mut self) {
        self.fill_screen(self.background_color);
        self.reset_cursor();
    }

    /// Fill the entire screen with a specific color
    pub fn fill_screen(&mut self, color: u32) {
        let total_pixels = self.info.width * self.info.height;
        let bytes_per_pixel = self.info.bytes_per_pixel;

        for pixel_idx in 0..total_pixels {
            let buffer_offset = pixel_idx * bytes_per_pixel;
            if buffer_offset + bytes_per_pixel <= self.buffer.as_ptr().as_raw_ptr().len() {
                self.write_pixel_at_offset(buffer_offset, color);
            }
        }
    }

    /// Reset cursor to top-left corner (with padding)
    pub fn reset_cursor(&mut self) {
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
        self.layout.reset(&LayoutSettings {
            x: BORDER_PADDING as f32,
            y: BORDER_PADDING as f32,
            max_width: Some((self.info.width - 2 * BORDER_PADDING) as f32),
            max_height: Some((self.info.height - 2 * BORDER_PADDING) as f32),
            line_height: 1.2,
            ..LayoutSettings::default()
        });
    }

    /// Set cursor position (with bounds checking)
    pub fn set_position(&mut self, x: usize, y: usize) -> Result<(), FrameBufferError> {
        if x >= self.info.width || y >= self.info.height {
            return Err(FrameBufferError::InvalidPosition);
        }
        self.x_pos = x;
        self.y_pos = y;
        Ok(())
    }

    /// Get current cursor position
    pub fn get_position(&self) -> (usize, usize) {
        (self.x_pos, self.y_pos)
    }

    /// Set foreground (text) color
    pub fn set_foreground_color(&mut self, color: u32) {
        self.foreground_color = color;
    }

    /// Set background color
    pub fn set_background_color(&mut self, color: u32) {
        self.background_color = color;
    }

    /// Write a single character at current cursor position
    pub fn write_char(&mut self, ch: char) -> Result<(), FrameBufferError> {
        match ch {
            '\n' => self.newline(),
            '\r' => {
                self.x_pos = BORDER_PADDING;
                Ok(())
            }
            '\t' => {
                // Tab = 4 spaces
                for _ in 0..4 {
                    self.write_char(' ')?;
                }
                Ok(())
            }
            c if c.is_control() => Ok(()), // Skip other control characters
            c => self.render_character(c),
        }
    }

    /// Write a string using fontdue layout for proper positioning
    pub fn write_string(&mut self, text: &str) -> Result<(), FrameBufferError> {
        if text.is_empty() {
            return Ok(());
        }

        // Use fontdue layout for better text positioning
        self.layout.clear();
        self.layout
            .append(&[&*FONT], &TextStyle::new(text, FONT_SIZE, 0));

        // Collect glyphs into an owned Vec to avoid immutably borrowing `self.layout`
        let collected_glyphs: Vec<GlyphPosition> = self.layout.glyphs().to_vec();

        for glyph in &collected_glyphs {
            self.render_glyph_at_position(glyph)?;
        }

        // Update cursor position based on last glyph
        if let Some(last_glyph) = collected_glyphs.last() {
            self.x_pos = (last_glyph.x + last_glyph.width as f32) as usize;
            self.y_pos = last_glyph.y as usize;
        }

        Ok(())
    }

    /// Write a string with automatic line wrapping
    pub fn write_string_wrapped(&mut self, text: &str) -> Result<(), FrameBufferError> {
        let words: Vec<&str> = text.split_whitespace().collect();
        let available_width = self.info.width - self.x_pos - BORDER_PADDING;

        for (i, word) in words.iter().enumerate() {
            let word_width = self.calculate_text_width(word);

            // Check if we need to wrap to next line
            if word_width > available_width {
                self.newline()?;
            }

            self.write_string(word)?;

            // Add space between words (except for last word)
            if i < words.len() - 1 {
                let space_width = self.calculate_text_width(" ");

                // Check if space + next word will fit
                if let Some(next_word) = words.get(i + 1) {
                    let next_word_width = self.calculate_text_width(next_word);
                    let current_x = self.x_pos;

                    if current_x + space_width + next_word_width > self.info.width - BORDER_PADDING
                    {
                        self.newline()?;
                        continue; // Skip the space since we're on a new line
                    }
                }

                self.write_char(' ')?;
            }
        }

        Ok(())
    }

    /// Move to the next line
    pub fn newline(&mut self) -> Result<(), FrameBufferError> {
        self.x_pos = BORDER_PADDING;
        self.y_pos += FONT_SIZE as usize + LINE_SPACING;

        if self.y_pos + FONT_SIZE as usize > self.info.height - BORDER_PADDING {
            self.y_pos = BORDER_PADDING;
        }

        self.layout.reset(&LayoutSettings {
            x: self.x_pos as f32,
            y: self.y_pos as f32,
            max_width: Some((self.info.width - 2 * BORDER_PADDING) as f32),
            max_height: Some((self.info.height - self.y_pos - BORDER_PADDING) as f32),
            line_height: 1.2,
            ..LayoutSettings::default()
        });

        Ok(())
    }

    /// Render a single character
    fn render_character(&mut self, ch: char) -> Result<(), FrameBufferError> {
        let (metrics, bitmap) = FONT.rasterize(ch, FONT_SIZE);

        let render_x = self.x_pos;
        let render_y = self.y_pos + metrics.ymin.max(0) as usize;

        self.render_bitmap_at(render_x, render_y, &bitmap, metrics.width, metrics.height)?;

        self.x_pos += metrics.advance_width as usize + LETTER_SPACING;

        if self.x_pos > self.info.width - BORDER_PADDING {
            self.newline()?;
        }

        Ok(())
    }

    /// Render a glyph at its layout-determined position
    fn render_glyph_at_position(&mut self, glyph: &GlyphPosition) -> Result<(), FrameBufferError> {
        let ch = glyph.parent;
        let (metrics, bitmap) = FONT.rasterize(ch, FONT_SIZE);

        let render_x = glyph.x as usize;
        let render_y = glyph.y as usize;

        self.render_bitmap_at(render_x, render_y, &bitmap, metrics.width, metrics.height)
    }

    /// Render a bitmap at the specified position
    fn render_bitmap_at(
        &mut self,
        x: usize,
        y: usize,
        bitmap: &[u8],
        width: usize,
        height: usize,
    ) -> Result<(), FrameBufferError> {
        let expected_size = width * height;
        if bitmap.len() < expected_size {
            return Err(FrameBufferError::BufferTooSmall);
        }

        for pixel_y in 0..height {
            let row_start = pixel_y * width;
            let row_end = row_start + width;

            if row_end > bitmap.len() {
                break;
            }

            let row = &bitmap[row_start..row_end];

            for (pixel_x, &alpha) in row.iter().enumerate() {
                let screen_x = x + pixel_x;
                let screen_y = y + pixel_y;

                if screen_x >= self.info.width || screen_y >= self.info.height {
                    continue;
                }

                if alpha > 0 {
                    let blended_color = if alpha == 255 {
                        self.foreground_color
                    } else {
                        self.blend_colors(self.background_color, self.foreground_color, alpha)
                    };

                    self.write_pixel_at(screen_x, screen_y, blended_color)?;
                }
            }
        }
        Ok(())
    }

    /// Write a pixel at the given coordinates
    fn write_pixel_at(&mut self, x: usize, y: usize, color: u32) -> Result<(), FrameBufferError> {
        if x >= self.info.width || y >= self.info.height {
            return Err(FrameBufferError::InvalidPosition);
        }

        let pixel_offset = (y * self.info.stride + x) * self.info.bytes_per_pixel;
        self.write_pixel_at_offset(pixel_offset, color);
        Ok(())
    }

    /// Write a pixel at the given buffer offset using volatile writes
    fn write_pixel_at_offset(&mut self, offset: usize, color: u32) {
        let buffer_len = self.buffer.as_ptr().as_raw_ptr().len();
        if offset + self.info.bytes_per_pixel > buffer_len {
            return;
        }

        let (b4, b3, b2, b1) = {
            (
                color as u8,
                (color >> 8) as u8,
                (color >> 16) as u8,
                (color >> 24) as u8,
            )
        };

        // Helper function to write bytes volatilely
        fn write_bytes(buffer: VolatilePtr<[u8]>, offset: usize, bytes: &[u8]) {
            for (i, &byte) in bytes.iter().enumerate() {
                buffer.index(offset + i).write(byte);
            }
        }

        let buffer_ptr = self.buffer.as_mut_ptr();

        match self.info.pixel_format {
            bootloader_api::info::PixelFormat::Rgb => match self.info.bytes_per_pixel {
                3 => write_bytes(buffer_ptr, offset, &[b3, b2, b1]),
                4 => write_bytes(buffer_ptr, offset, &[b4, b3, b2, b1]),
                _ => {
                    // Fallback: write as many bytes as needed
                    let bytes = color.to_le_bytes();
                    let write_len = self.info.bytes_per_pixel.min(4);
                    for (i, &b) in bytes.iter().enumerate().take(write_len) {
                        buffer_ptr.index(offset + i).write(b);
                    }
                }
            },
            bootloader_api::info::PixelFormat::Bgr => match self.info.bytes_per_pixel {
                3 => write_bytes(buffer_ptr, offset, &[b1, b2, b3]),
                4 => write_bytes(buffer_ptr, offset, &[b2, b3, b4, b1]),
                _ => {
                    // Fallback
                    let bytes = color.to_le_bytes();
                    let write_len = self.info.bytes_per_pixel.min(4);
                    for (i, &b) in bytes.iter().enumerate().take(write_len) {
                        buffer_ptr.index(offset + i).write(b);
                    }
                }
            },
            _ => unreachable!(),
        }
    }

    /// Simple alpha blending between two colors
    fn blend_colors(&self, bg: u32, fg: u32, alpha: u8) -> u32 {
        let alpha = alpha as u32;
        let inv_alpha = 255 - alpha;

        let bg_r = (bg >> 16) & 0xFF;
        let bg_g = (bg >> 8) & 0xFF;
        let bg_b = bg & 0xFF;

        let fg_r = (fg >> 16) & 0xFF;
        let fg_g = (fg >> 8) & 0xFF;
        let fg_b = fg & 0xFF;

        let r = (fg_r * alpha + bg_r * inv_alpha) / 255;
        let g = (fg_g * alpha + bg_g * inv_alpha) / 255;
        let b = (fg_b * alpha + bg_b * inv_alpha) / 255;

        (r << 16) | (g << 8) | b
    }

    /// Calculate text dimensions using fontdue's Layout system for accuracy
    pub fn calculate_text_dimensions(&self, text: &str) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }

        // Create a temporary layout for calculation
        let mut temp_layout = Layout::new(CoordinateSystem::PositiveYDown);
        temp_layout.reset(&LayoutSettings {
            x: 0.0,
            y: 0.0,
            max_width: None, // No wrapping for measurement
            max_height: None,
            ..LayoutSettings::default()
        });

        // Add the text to layout
        temp_layout.append(&[&*FONT], &TextStyle::new(text, FONT_SIZE, 0));

        // Calculate dimensions from positioned glyphs
        let glyphs = temp_layout.glyphs();

        if glyphs.is_empty() {
            return (0.0, 0.0);
        }

        // Find the bounding box of all glyphs
        let mut min_x = f32::MAX;
        let mut max_x = f32::MIN;
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;

        for glyph in glyphs {
            let left = glyph.x;
            let right = glyph.x + glyph.width as f32;
            let top = glyph.y;
            let bottom = glyph.y + glyph.height as f32;

            min_x = min_x.min(left);
            max_x = max_x.max(right);
            min_y = min_y.min(top);
            max_y = max_y.max(bottom);
        }

        let width = if max_x > min_x { max_x - min_x } else { 0.0 };
        let height = if max_y > min_y { max_y - min_y } else { 0.0 };

        (width, height)
    }

    /// Check if text will fit within available width
    pub fn will_text_fit(&self, text: &str, available_width: usize) -> bool {
        let text_width = self.calculate_text_width(text);
        text_width <= available_width
    }

    pub fn calculate_text_width(&self, text: &str) -> usize {
        self.calculate_text_dimensions(text).0 as usize
    }
}

// Implement Write trait for easy formatting
impl Write for FrameBufferWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write_string(s).map_err(|_| fmt::Error)
    }

    fn write_char(&mut self, c: char) -> fmt::Result {
        self.write_char(c).map_err(|_| fmt::Error)
    }
}

pub static WRITER: Mutex<Option<FrameBufferWriter>> = Mutex::new(None);

pub fn init_framebuffer(framebuffer: &'static mut FrameBuffer) {
    let writer = FrameBufferWriter::new(framebuffer);
    *WRITER.lock() = Some(writer);
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    let mut writer_guard = WRITER.lock();
    if let Some(writer) = writer_guard.as_mut() {
        writer.write_fmt(args).unwrap();
    }
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::frame_buffer::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}
