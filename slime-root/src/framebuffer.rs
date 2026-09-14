//! Bounded text rendering onto the boot framebuffer (P6.6).
//!
//! A physical Framework 13 exposes no serial port, so the readiness record the
//! QEMU planes read over COM1 has no channel on the real machine. What the
//! firmware does leave behind is a linear framebuffer: GRUB negotiates a mode
//! through EFI GOP, seL4 records the resulting `MULTIBOOT2_TAG_FB` and
//! re-exports it as `SEL4_BOOTINFO_HEADER_X86_FRAMEBUFFER`, and this module
//! turns that record into a small number of legible lines.
//!
//! **Deliberately not a display driver and not a console.** It sets no mode,
//! enumerates no device, and takes no input. It writes pixels into a region the
//! firmware already programmed, which is why it needs no PCI, GOP, or ACPI
//! authority of its own — only BootInfo's device untyped for those frames.
//!
//! The geometry is never trusted. `KernelMultibootGFXDepth` asks for 32 bits
//! per pixel and the pinned q35 target answers with 24: a Multiboot2
//! framebuffer request is a hint the bootloader may satisfy with any mode it
//! has. Every field is therefore range-checked against the record's own stated
//! size before a single byte is written, and an unsupported format disables
//! rendering rather than scribbling over whatever else lives at that address.

use crate::device::{DeviceError, DeviceRegion};
use crate::glyph_font::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph};
use crate::object_allocator::ObjectAllocator;

/// Bytes in a mapping granule, matching the allocator's device frames.
const GRANULE_BYTES: usize = 4096;

/// Scale applied to every glyph, so 8x8 cells stay readable on a high-density
/// panel viewed at arm's length.
const SCALE: usize = 2;

/// Left and top margin in pixels, keeping text clear of an overscanned edge.
const MARGIN: usize = 16;

/// Upper bound on the mode this module will drive, in pixels.
///
/// Not a hardware limit — a bound that keeps `pitch * height` arithmetic and
/// the per-granule walk finite on a record whose fields the firmware supplied.
const MAX_WIDTH: usize = 8192;
const MAX_HEIGHT: usize = 8192;

/// Pixel encodings this renderer can write.
///
/// Both are the packed little-endian RGB layouts a UEFI GOP linear mode
/// reports. Component order is irrelevant here: every pixel written is either
/// full-intensity white or black, which is invariant under channel
/// permutation. A palette or a non-RGB mode is refused instead.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PixelFormat {
    /// Three bytes per pixel.
    Rgb24,
    /// Four bytes per pixel, top byte ignored.
    Rgb32,
}

impl PixelFormat {
    const fn bytes(self) -> usize {
        match self {
            PixelFormat::Rgb24 => 3,
            PixelFormat::Rgb32 => 4,
        }
    }
}

/// Why a framebuffer could not be driven.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FramebufferError {
    /// BootInfo carried no framebuffer record. The bootloader left the machine
    /// in text mode, or the kernel was built without the Multiboot2
    /// framebuffer request tag.
    Absent,
    /// The record was shorter than the fields it must contain.
    Truncated { len: usize },
    /// The record described a mode this renderer does not implement.
    Unsupported {
        width: u32,
        height: u32,
        bpp: u8,
        kind: u8,
    },
    /// The record's own geometry is inconsistent: a pitch narrower than a row,
    /// a zero dimension, or a size that overflows.
    Geometry { width: u32, height: u32, pitch: u32 },
    /// A framebuffer granule could not be retyped or mapped.
    Map(DeviceError),
}

/// The validated geometry of the firmware's framebuffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramebufferInfo {
    paddr: usize,
    width: usize,
    height: usize,
    pitch: usize,
    format: PixelFormat,
}

impl FramebufferInfo {
    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub const fn bits_per_pixel(&self) -> usize {
        self.format.bytes() * 8
    }

    pub const fn physical_address(&self) -> usize {
        self.paddr
    }

    /// Total bytes the mode occupies, which is what the granule walk covers.
    ///
    /// The last row is `width` pixels rather than a full `pitch`: padding past
    /// the final visible pixel need not be backed, and assuming it is would
    /// demand a granule the firmware may not have reserved.
    const fn byte_len(&self) -> usize {
        (self.height - 1) * self.pitch + self.width * self.format.bytes()
    }

    /// Rows of scaled text the mode can display, bounded by the margin.
    const fn text_rows(&self) -> usize {
        let usable = self.height.saturating_sub(MARGIN * 2);
        usable / (GLYPH_HEIGHT * SCALE)
    }

    /// Columns of scaled text the mode can display.
    const fn text_columns(&self) -> usize {
        let usable = self.width.saturating_sub(MARGIN * 2);
        usable / (GLYPH_WIDTH * SCALE)
    }
}

/// seL4's `multiboot2_fb_t`, as re-exported in the extra-BootInfo record.
///
/// Offsets rather than a `#[repr(C)]` struct: this is a borrowed byte slice
/// whose producer is the pinned kernel, and reading named fields out of it by
/// offset keeps the decode explicit and bounds-checked. The trailing
/// colour-layout union is not read, because every pixel written here is black
/// or white.
const FB_ADDR: usize = 0;
const FB_PITCH: usize = 8;
const FB_WIDTH: usize = 12;
const FB_HEIGHT: usize = 16;
const FB_BPP: usize = 20;
const FB_TYPE: usize = 21;
const FB_FIELDS_LEN: usize = 22;

/// `multiboot2_fb_t.type` for a direct-colour RGB mode. Indexed-colour (0) and
/// EGA text (2) are refused: neither can be written as packed RGB.
const FB_TYPE_RGB: u8 = 1;

fn read_u32(content: &[u8], offset: usize) -> u32 {
    let mut bytes = [0u8; 4];
    bytes.copy_from_slice(&content[offset..offset + 4]);
    u32::from_ne_bytes(bytes)
}

fn read_u64(content: &[u8], offset: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&content[offset..offset + 8]);
    u64::from_ne_bytes(bytes)
}

/// Decode and validate the framebuffer record BootInfo carries, if any.
pub fn describe(bootinfo: &sel4::BootInfoPtr) -> Result<FramebufferInfo, FramebufferError> {
    let extra = bootinfo
        .extra()
        .find(|extra| extra.id == sel4::BootInfoExtraId::X86FrameBuffer)
        .ok_or(FramebufferError::Absent)?;
    let content = extra.content();
    if content.len() < FB_FIELDS_LEN {
        return Err(FramebufferError::Truncated { len: content.len() });
    }

    let addr = read_u64(content, FB_ADDR);
    let pitch = read_u32(content, FB_PITCH);
    let width = read_u32(content, FB_WIDTH);
    let height = read_u32(content, FB_HEIGHT);
    let bpp = content[FB_BPP];
    let kind = content[FB_TYPE];

    let format = match (kind, bpp) {
        (FB_TYPE_RGB, 24) => PixelFormat::Rgb24,
        (FB_TYPE_RGB, 32) => PixelFormat::Rgb32,
        _ => {
            return Err(FramebufferError::Unsupported {
                width,
                height,
                bpp,
                kind,
            });
        }
    };

    // A 64-bit framebuffer address that does not fit a pointer cannot be
    // mapped at all, and the granule walk below would wrap while computing its
    // end.
    let Ok(paddr) = usize::try_from(addr) else {
        return Err(FramebufferError::Geometry {
            width,
            height,
            pitch,
        });
    };
    let (width, height, pitch) = (width as usize, height as usize, pitch as usize);
    // `pitch < row bytes` would make row `n + 1` start inside row `n`, so a
    // bounded write per row would still overlap. Zero dimensions leave nothing
    // to draw, and the bounds keep `height * pitch` finite.
    if width == 0
        || height == 0
        || width > MAX_WIDTH
        || height > MAX_HEIGHT
        || pitch < width * format.bytes()
        || paddr == 0
        || !paddr.is_multiple_of(GRANULE_BYTES)
    {
        return Err(FramebufferError::Geometry {
            width: width as u32,
            height: height as u32,
            pitch: pitch as u32,
        });
    }

    Ok(FramebufferInfo {
        paddr,
        width,
        height,
        pitch,
        format,
    })
}

/// One granule of the framebuffer, mapped at a reused scratch address.
///
/// The whole mode is 1.4 MiB at 800x600x24 and far larger on a laptop panel,
/// which no single granule window covers and which the root has no reason to
/// map simultaneously: text is written once, top to bottom. So the window
/// advances a granule at a time, exactly as `DeviceRegion::unmap` is documented
/// to allow, and a device untyped's monotonic retype is satisfied because the
/// walk only ever moves forward.
struct Window {
    region: DeviceRegion,
    /// Framebuffer-relative byte offset this granule begins at.
    offset: usize,
    /// Root CSlot the granule's frame capability occupies, so the walk can
    /// return it. A `Granule` capability does not carry its own index.
    slot: usize,
    /// Whether the frame is currently mapped at the scratch address.
    ///
    /// A released window keeps its capability — it anchors the device untyped
    /// — but its virtual address is gone, so a later render of the *same*
    /// granule must map it again rather than reuse a base of zero.
    mapped: bool,
}

/// A framebuffer the root can draw bounded text into.
pub struct Framebuffer {
    info: FramebufferInfo,
    vspace: sel4::cap::VSpace,
    base: usize,
    window: Option<Window>,
    /// Next text row to write, so successive records stack down the panel.
    row: usize,
}

impl Framebuffer {
    /// Claim the framebuffer described by BootInfo.
    ///
    /// `base` must be a granule-aligned root-image address already released by
    /// `ScratchPage::claim`, exactly as a device register window is.
    pub fn claim(info: FramebufferInfo, vspace: sel4::cap::VSpace, base: usize) -> Self {
        Self {
            info,
            vspace,
            base,
            window: None,
            row: 0,
        }
    }

    pub const fn info(&self) -> &FramebufferInfo {
        &self.info
    }

    /// Map the granule containing framebuffer offset `offset`, reusing the
    /// current mapping when it already covers it.
    ///
    /// Exactly one granule capability is kept alive at all times. This is not
    /// frugality — it is the device untyped's own invariant, stated where the
    /// allocator walks one: *if every capability derived from a device untyped
    /// disappears, seL4 resets that untyped's free index to zero*, and the
    /// next retype recreates the prefix instead of continuing toward the
    /// requested page. Releasing the current granule before taking the next
    /// one therefore restarts the walk and renders garbage — observed as a
    /// panel that drew two pixel rows and stopped.
    ///
    /// So the *previous* granule is released only after the next one exists.
    /// That caps the cost at two live frames rather than one per granule,
    /// which is what the provenance table — sized for the live DMA-capable
    /// population, not for a display — can actually hold: a 1280x800x32 mode
    /// otherwise raised the root's baseline live slots from 54 to 119, and a
    /// panel-sized mode would exhaust the table and make the *graph* fail to
    /// launch for a reason that has nothing to do with it.
    ///
    /// Returns the mapped virtual address of that granule.
    fn window_for(
        &mut self,
        allocator: &mut ObjectAllocator,
        offset: usize,
    ) -> Result<usize, FramebufferError> {
        let granule = offset - offset % GRANULE_BYTES;
        if let Some(window) = self.window.as_ref() {
            if window.offset == granule && window.mapped {
                return Ok(window.region.mapped_base());
            }
            if window.offset == granule {
                // Same granule, capability retained, but `release` gave the
                // virtual address back. Re-map the frame this root still
                // holds rather than retyping another.
                let window = self.window.take().expect("checked above");
                let region = window
                    .region
                    .remap(self.vspace, self.base)
                    .map_err(FramebufferError::Map)?;
                let mapped = region.mapped_base();
                self.window = Some(Window {
                    region,
                    mapped: true,
                    ..window
                });
                return Ok(mapped);
            }
        }
        // Free the virtual window first: one claimed root-image page is what
        // scans the whole mode, so the address has to be available before the
        // next frame can take it. The capability stays, anchoring the untyped.
        let previous = match self.window.take() {
            Some(mut window) => {
                if window.mapped {
                    window.region.unmap().map_err(FramebufferError::Map)?;
                    window.mapped = false;
                }
                Some(window)
            }
            None => None,
        };
        let (region, slot) =
            DeviceRegion::map_tracked(allocator, self.vspace, self.base, self.info.paddr + granule)
                .map_err(FramebufferError::Map)?;
        // The new frame now anchors the untyped, so the old one can go.
        if let Some(window) = previous {
            let slot = window.slot;
            window
                .region
                .discard(allocator, slot)
                .map_err(FramebufferError::Map)?;
        }
        let mapped = region.mapped_base();
        self.window = Some(Window {
            region,
            offset: granule,
            slot,
            mapped: true,
        });
        Ok(mapped)
    }

    /// Write one pixel, ignoring coordinates outside the validated mode.
    fn put_pixel(
        &mut self,
        allocator: &mut ObjectAllocator,
        x: usize,
        y: usize,
        on: bool,
    ) -> Result<(), FramebufferError> {
        if x >= self.info.width || y >= self.info.height {
            return Ok(());
        }
        let bytes = self.info.format.bytes();
        let offset = y * self.info.pitch + x * bytes;
        // A pixel must not straddle two granules, or the second half would land
        // in whatever the window holds next, so such a pixel is skipped rather
        // than torn.
        //
        // Only reachable at 24 bpp: 3 bytes does not divide 4096, so a granule
        // boundary can fall mid-pixel. At 32 bpp — which is what the Framework
        // panel reports — every pixel is 4-byte aligned within the granule and
        // this never triggers. The cost when it does is one dropped pixel of
        // glyph or background, against a stray write into an unrelated page.
        if offset % GRANULE_BYTES + bytes > GRANULE_BYTES {
            return Ok(());
        }
        let mapped = self.window_for(allocator, offset)?;
        let address = mapped + offset % GRANULE_BYTES;
        let value: u32 = if on { 0x00ff_ffff } else { 0 };
        // SAFETY: `mapped` names a granule this root retyped from the device
        // untyped covering the firmware's framebuffer and mapped read-write at
        // `base`; `offset` is inside the validated mode, the straddle check
        // above keeps all `bytes` inside that granule, and the writes are
        // volatile because they are an effect on a display rather than stores
        // the compiler may elide.
        unsafe {
            match self.info.format {
                PixelFormat::Rgb24 => {
                    let pointer = address as *mut u8;
                    pointer.write_volatile((value & 0xff) as u8);
                    pointer.add(1).write_volatile(((value >> 8) & 0xff) as u8);
                    pointer.add(2).write_volatile(((value >> 16) & 0xff) as u8);
                }
                PixelFormat::Rgb32 => (address as *mut u32).write_volatile(value),
            }
        }
        Ok(())
    }

    /// Draw `text` as the next line, truncated to what the mode can show.
    ///
    /// Every pixel of the line's cell band is written — background as well as
    /// foreground. Writing only the set bits leaves whatever the firmware and
    /// the bootloader already drew showing through the gaps, which on the
    /// first physical boot that reached this point rendered the record
    /// illegible: GRUB's own console text sat underneath, and a generation
    /// digest read back as `41?1?19329F39BF5FA1F` instead of
    /// `0F1929F39BF5FA1F`. An evidence channel that can be misread is not one.
    ///
    /// Emitted strictly in ascending framebuffer offset: every pixel of screen
    /// row `y` before any pixel of row `y + 1`, and left to right within a
    /// row. This is a correctness requirement rather than a style choice. A
    /// device untyped's retype is monotonic, so `allocate_device_frame` can
    /// only ever walk forward; drawing glyph-by-glyph would return to a lower
    /// offset at each new character and the allocator would — correctly —
    /// refuse with `DeviceFramePassed`. Painting the background in the same
    /// left-to-right pass preserves that, where a separate clearing pass over
    /// the band would have to walk the region twice.
    ///
    /// Lines past the bottom of the panel are dropped rather than scrolled: the
    /// record this renders is bounded, and scrolling would mean re-reading
    /// pixels the root has no reason to keep.
    pub fn write_line(
        &mut self,
        allocator: &mut ObjectAllocator,
        text: &[u8],
    ) -> Result<(), FramebufferError> {
        if self.row >= self.info.text_rows() {
            return Ok(());
        }
        let top = MARGIN + self.row * GLYPH_HEIGHT * SCALE;
        // The first line also clears the rows above it, which is the top
        // margin: nothing else ever writes them, so a bootloader's output
        // would survive there.
        if self.row == 0 {
            self.fill_rows(allocator, 0, MARGIN)?;
        }
        let columns = self.info.text_columns();
        let text_width = columns * GLYPH_WIDTH * SCALE;
        for cell_row in 0..GLYPH_HEIGHT {
            for dy in 0..SCALE {
                let y = top + cell_row * SCALE + dy;
                // Whole screen rows, left edge to right edge. Clearing only
                // the text band leaves the side margins holding whatever was
                // underneath, which is exactly what the second complete
                // physical boot showed in the screen corners.
                for x in 0..self.info.width {
                    let on = x
                        .checked_sub(MARGIN)
                        .filter(|offset| *offset < text_width)
                        .and_then(|offset| {
                            let column = offset / (GLYPH_WIDTH * SCALE);
                            let cell_column = (offset % (GLYPH_WIDTH * SCALE)) / SCALE;
                            text.get(column)
                                .copied()
                                .and_then(glyph)
                                .map(|bitmap| bitmap[cell_row] & (0x80 >> cell_column) != 0)
                        })
                        .unwrap_or(false);
                    self.put_pixel(allocator, x, y, on)?;
                }
            }
        }
        self.row += 1;
        Ok(())
    }

    /// Paint every pixel of screen rows `top..bottom` black.
    ///
    /// Ascending by construction, so it composes with the record's own walk:
    /// the framebuffer's device untyped is retyped monotonically and a
    /// backwards step would be refused.
    fn fill_rows(
        &mut self,
        allocator: &mut ObjectAllocator,
        top: usize,
        bottom: usize,
    ) -> Result<(), FramebufferError> {
        for y in top..bottom.min(self.info.height) {
            for x in 0..self.info.width {
                self.put_pixel(allocator, x, y, false)?;
            }
        }
        Ok(())
    }

    /// Clear everything below the last line written.
    ///
    /// The record is shorter than the panel, so the rows past it are never
    /// otherwise touched and keep the bootloader's output. Called once, after
    /// the final line, because the walk cannot return to them afterwards.
    pub fn clear_tail(&mut self, allocator: &mut ObjectAllocator) -> Result<(), FramebufferError> {
        let below = MARGIN + self.row * GLYPH_HEIGHT * SCALE;
        self.fill_rows(allocator, below, self.info.height)
    }

    /// Release the virtual window, leaving the drawn pixels on screen.
    ///
    /// Pixels already written stay visible: the display controller scans the
    /// physical memory regardless of whether this root still holds a mapping.
    /// So the root gives back the root-image page it borrowed.
    ///
    /// The last granule's *capability* is deliberately kept. It is one frame
    /// and one CSlot, and it anchors the framebuffer's device untyped: were it
    /// deleted, seL4 would reset that untyped's free index and a later render
    /// — a `fatal!` after readiness, say — would restart the granule walk and
    /// draw garbage over the record. `unmap` rather than `discard` for exactly
    /// that reason.
    pub fn release(&mut self) -> Result<(), FramebufferError> {
        if let Some(window) = self.window.as_mut() {
            window.region.unmap().map_err(FramebufferError::Map)?;
        }
        Ok(())
    }
}

/// The panel a render site should reach, when a mode was negotiated.
///
/// A global because `fatal!` expands at 126 sites across the root, most of
/// them holding no display handle, and because the readiness record is written
/// from the service loop, which otherwise has no reason to know about
/// displays. On a machine whose only channel is the panel, a fatal that prints
/// to serial alone is a silent stop — exactly the condition that made the
/// first physical boot attempt uninterpretable.
///
/// The allocator pointer is stored alongside for the fatal path only; see
/// [`report_fatal`] for why that is separate from [`with_panel`].
///
/// Written once during the display phase and read from the root's own startup
/// and fatal paths, both single-threaded.
static mut PANEL: Option<(Framebuffer, *mut ObjectAllocator)> = None;

/// Install `framebuffer` as the root's panel.
///
/// # Safety
///
/// `allocator` must name the root's own allocator storage, which lives for the
/// whole boot, and this must be called once from root startup before any other
/// thread exists.
pub unsafe fn install_panel(framebuffer: Framebuffer, allocator: *mut ObjectAllocator) {
    unsafe { *core::ptr::addr_of_mut!(PANEL) = Some((framebuffer, allocator)) };
}

/// Run `body` against the installed panel with an allocator the caller owns.
///
/// This is the ordinary render path. The allocator is a parameter rather than
/// taken from the global precisely because every startup render site already
/// holds it exclusively: re-deriving it here would produce two live exclusive
/// references to one allocator.
///
/// Returns `false` when no mode was negotiated, which is every emulated plane
/// except the media gate.
///
/// # Safety
///
/// Must not be called reentrantly or after components are running. Both hold
/// for the root's single-threaded startup path.
pub unsafe fn with_panel(
    allocator: &mut ObjectAllocator,
    body: impl FnOnce(&mut Framebuffer, &mut ObjectAllocator),
) -> bool {
    if let Some((panel, _)) = unsafe { (&mut *core::ptr::addr_of_mut!(PANEL)).as_mut() } {
        body(panel, allocator);
        return true;
    }
    false
}

/// Render a fatal label on the panel, then return so the caller can suspend.
///
/// Separate from [`with_panel`] because a failing caller cannot hand over an
/// allocator it may already hold, and the granule walk needs one. This path
/// therefore re-derives the allocator from the pointer installed above, which
/// can alias a caller's live exclusive reference.
///
/// That is a deliberate, bounded trade rather than an oversight. It is
/// reachable only from `fatal!`, which suspends the root on the next
/// statement, so nothing observes the allocator afterwards; the alternative on
/// a machine with no serial port is no evidence at all, which is the failure
/// mode this module exists to remove. The allocator is used only to retype and
/// map device granules, and a failure to do so leaves the label unrendered
/// rather than faulting.
///
/// # Safety
///
/// Must be called only from a terminal path that suspends immediately
/// afterwards and never returns to a caller holding the allocator.
pub unsafe fn report_fatal(message: &str) {
    if let Some((panel, allocator)) = unsafe { (&mut *core::ptr::addr_of_mut!(PANEL)).as_mut() } {
        // SAFETY: `install_panel`'s contract — the pointer names the root's own
        // allocator, which outlives the boot — plus this function's own: the
        // caller suspends next, so this reference is the last live one.
        if let Some(allocator) = unsafe { allocator.as_mut() } {
            let _ = panel.write_line(allocator, message.as_bytes());
            // A fatal also ends the record, so the rows past it would keep the
            // bootloader's output and make the report ambiguous.
            let _ = panel.clear_tail(allocator);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(width: usize, height: usize, pitch: usize, format: PixelFormat) -> FramebufferInfo {
        FramebufferInfo {
            paddr: 0x8000_0000,
            width,
            height,
            pitch,
            format,
        }
    }

    #[test]
    fn byte_len_excludes_padding_past_the_last_visible_pixel() {
        // 800x600x24 with the pitch QEMU's std VGA reports. The final row
        // contributes its visible pixels only, so the walk never demands a
        // granule past the mode.
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        assert_eq!(framebuffer.byte_len(), 599 * 2400 + 800 * 3);
        assert!(framebuffer.byte_len() <= 600 * 2400);
    }

    #[test]
    fn text_grid_accounts_for_both_margins_and_scale() {
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        assert_eq!(framebuffer.text_columns(), (800 - 32) / 16);
        assert_eq!(framebuffer.text_rows(), (600 - 32) / 16);
    }

    #[test]
    fn a_mode_smaller_than_its_margins_yields_no_text_cells() {
        // Rendering must refuse rather than underflow when the panel cannot
        // hold even one cell inside the margins.
        let framebuffer = info(8, 8, 24, PixelFormat::Rgb24);
        assert_eq!(framebuffer.text_columns(), 0);
        assert_eq!(framebuffer.text_rows(), 0);
    }

    #[test]
    fn bits_per_pixel_reports_the_decoded_format() {
        assert_eq!(info(8, 8, 24, PixelFormat::Rgb24).bits_per_pixel(), 24);
        assert_eq!(info(8, 8, 32, PixelFormat::Rgb32).bits_per_pixel(), 32);
    }

    /// The `(offset, on)` pairs `write_line` writes, in the order it writes
    /// them.
    ///
    /// Mirrors the loop in `write_line` rather than calling it, because the
    /// real one needs an allocator and a mapped device frame. What is asserted
    /// is the *order* and the *coverage*, which are the parts a refactor can
    /// silently break.
    fn visit_order(
        framebuffer: &FramebufferInfo,
        row: usize,
        text: &[u8],
    ) -> alloc::vec::Vec<(usize, bool)> {
        let mut writes = alloc::vec::Vec::new();
        let bytes = framebuffer.format.bytes();
        let columns = framebuffer.text_columns();
        let text_width = columns * GLYPH_WIDTH * SCALE;
        let top = MARGIN + row * GLYPH_HEIGHT * SCALE;
        let mut emit =
            |writes: &mut alloc::vec::Vec<(usize, bool)>, x: usize, y: usize, on: bool| {
                if x < framebuffer.width && y < framebuffer.height {
                    writes.push((y * framebuffer.pitch + x * bytes, on));
                }
            };
        if row == 0 {
            for y in 0..MARGIN.min(framebuffer.height) {
                for x in 0..framebuffer.width {
                    emit(&mut writes, x, y, false);
                }
            }
        }
        for cell_row in 0..GLYPH_HEIGHT {
            for dy in 0..SCALE {
                let y = top + cell_row * SCALE + dy;
                for x in 0..framebuffer.width {
                    let on = x
                        .checked_sub(MARGIN)
                        .filter(|offset| *offset < text_width)
                        .and_then(|offset| {
                            let column = offset / (GLYPH_WIDTH * SCALE);
                            let cell_column = (offset % (GLYPH_WIDTH * SCALE)) / SCALE;
                            text.get(column)
                                .copied()
                                .and_then(glyph)
                                .map(|bitmap| bitmap[cell_row] & (0x80 >> cell_column) != 0)
                        })
                        .unwrap_or(false);
                    emit(&mut writes, x, y, on);
                }
            }
        }
        writes
    }

    #[test]
    fn a_rendered_line_only_ever_walks_forward() {
        // The load-bearing invariant of the whole renderer. A device untyped's
        // retype is monotonic, so `allocate_device_frame` can only walk
        // forward; an earlier draft emitted pixels glyph-by-glyph, returned to
        // a lower offset at each new character, and was refused with
        // `DeviceFramePassed`. Scanline-major order is what keeps offsets
        // ascending, and this test fails if that is ever reordered.
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        let writes = visit_order(&framebuffer, 0, b"SLIME OS - ROOT RUNNING");
        assert!(writes.len() > 1);
        assert!(
            writes.windows(2).all(|pair| pair[0].0 <= pair[1].0),
            "rendering must not revisit a lower framebuffer offset"
        );
    }

    #[test]
    fn successive_lines_start_below_the_previous_one() {
        // The same invariant across `write_line` calls: stage markers and the
        // readiness record are separate calls, and the walk must still only
        // move forward between them.
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        let first = visit_order(&framebuffer, 0, b"TIMER TICKING");
        let second = visit_order(&framebuffer, 1, b"GENERATION ADMITTED");
        let last = first.iter().map(|write| write.0).max();
        let next = second.iter().map(|write| write.0).min();
        assert!(last < next);
    }

    #[test]
    fn a_line_writes_every_pixel_of_its_rows_and_the_top_margin() {
        // Two physical boots taught this in stages. The first complete one
        // rendered only set bits, so GRUB's console text showed through the
        // gaps and a generation digest read back wrong. Fixing that to paint
        // the cell band left the margins untouched, and the next boot showed
        // bootloader text surviving in the screen corners. A line therefore
        // clears whole screen rows, and the first line also clears the rows
        // above it.
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        let writes = visit_order(&framebuffer, 0, b"OK");
        let band = framebuffer.width * GLYPH_HEIGHT * SCALE;
        let margin = framebuffer.width * MARGIN;
        assert_eq!(writes.len(), band + margin);
        assert!(
            writes.iter().any(|write| write.1),
            "the glyphs themselves must still be drawn"
        );
        // The leftmost column of the band is margin, so it must be cleared
        // rather than skipped.
        let bytes = framebuffer.format.bytes();
        let left_edge = MARGIN * framebuffer.pitch;
        assert!(
            writes.contains(&(left_edge, false)),
            "the left margin of a text row must be cleared"
        );
        let right_edge = left_edge + (framebuffer.width - 1) * bytes;
        assert!(
            writes.contains(&(right_edge, false)),
            "the right margin of a text row must be cleared"
        );
    }

    #[test]
    fn a_line_erases_whatever_occupied_its_band() {
        // The defect the first complete physical boot exposed, reproduced
        // deterministically. A real panel arrives holding the bootloader's own
        // console output; writing only the set bits leaves it showing through
        // the gaps, and the record becomes unreadable — on the observed boot a
        // generation digest photographed as `41?1?19329F39BF5FA1F` instead of
        // `0F1929F39BF5FA1F`.
        //
        // A QEMU boot cannot show this: its framebuffer starts black, so the
        // background writes are invisible either way. Replaying the write list
        // into a buffer pre-filled with noise does show it, and costs no
        // emulator.
        let framebuffer = info(800, 600, 2400, PixelFormat::Rgb24);
        let bytes = framebuffer.format.bytes();
        let mut panel = alloc::vec![0xa5u8; framebuffer.pitch * framebuffer.height];
        for (offset, on) in visit_order(&framebuffer, 0, b"OK") {
            let value: u32 = if on { 0x00ff_ffff } else { 0 };
            for index in 0..bytes {
                panel[offset + index] = ((value >> (index * 8)) & 0xff) as u8;
            }
        }
        // Every pixel of the band is now either glyph or black — no noise
        // survives anywhere inside it.
        let top = MARGIN;
        let width = framebuffer.text_columns() * GLYPH_WIDTH * SCALE;
        for y in top..top + GLYPH_HEIGHT * SCALE {
            for x in MARGIN..MARGIN + width {
                let offset = y * framebuffer.pitch + x * bytes;
                let pixel = &panel[offset..offset + bytes];
                assert!(
                    pixel.iter().all(|byte| *byte == 0) || pixel.iter().all(|byte| *byte == 0xff),
                    "stale pixel at ({x}, {y}): {pixel:?}"
                );
            }
        }
        // And the glyphs are really there, so the band was not merely cleared.
        assert!(
            panel[..framebuffer.pitch * (top + GLYPH_HEIGHT * SCALE)]
                .iter()
                .any(|byte| *byte == 0xff)
        );
    }
}
