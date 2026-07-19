//! Limine boot protocol requests.
//!
//! Every `static` here is scanned by the Limine bootloader at boot time.
//! Each request is a tag the kernel declares; Limine fills in the matching
//! response before jumping to `_start`. This is the "kernel declares what
//! authority/resources it needs" model — the same shape as the capability
//! grants the Slime generation manifest will eventually hand to components.
//!
//! All requests must live in the `.requests` linker section, between the
//! `.requests_start` and `.requests_end` markers, or Limine will not find
//! them when base revision >= 3.

use limine::{
    BaseRevision, RequestsEndMarker, RequestsStartMarker,
    request::{FramebufferRequest, HhdmRequest, MemmapRequest, ModulesRequest, RsdpRequest},
};

/// Marks the start of the Limine request block. Must appear before any
/// request and must be in `.requests_start`.
#[used]
#[unsafe(link_section = ".requests_start")]
pub static REQUESTS_START: RequestsStartMarker = RequestsStartMarker::new();

/// Declares which Limine base revision this kernel targets. `new()` picks
/// the highest revision the `limine` crate supports (currently 6). After
/// boot, `is_supported()` tells us whether Limine actually honored it.
#[used]
#[unsafe(link_section = ".requests")]
pub static BASE_REVISION: BaseRevision = BaseRevision::new();

/// Ask Limine to set up a linear framebuffer. Without this, there is no
/// graphical console and we can only talk over the serial port.
#[used]
#[unsafe(link_section = ".requests")]
pub static FRAMEBUFFER: FramebufferRequest = FramebufferRequest::new();

/// Physical memory map as seen by the firmware/bootloader. Needed before
/// we can build our own page tables or a frame allocator.
#[used]
#[unsafe(link_section = ".requests")]
pub static MEMMAP: MemmapRequest = MemmapRequest::new();

/// Higher-Half Direct Map offset. Limine identity-maps all usable physical
/// RAM at this virtual offset; adding it to any physical address yields the
/// virtual address Limine already mapped for us. Essential once we start
/// walking our own page tables or touching memory-map regions directly.
#[used]
#[unsafe(link_section = ".requests")]
pub static HHDM: HhdmRequest = HhdmRequest::new();

/// ACPI RSDP pointer. Needed later for APIC/timer bring-up (Milestone 1).
#[used]
#[unsafe(link_section = ".requests")]
pub static RSDP: RsdpRequest = RsdpRequest::new();

/// Generation manifest and immutable component objects, packaged as one module.
#[used]
#[unsafe(link_section = ".requests")]
pub static MODULES: ModulesRequest = ModulesRequest::new();

/// Marks the end of the request block.
#[used]
#[unsafe(link_section = ".requests_end")]
pub static REQUESTS_END: RequestsEndMarker = RequestsEndMarker::new();

/// Force the linker to include this module's request statics.
///
/// Binaries that do not otherwise reference any Limine request (test
/// harnesses that only talk over serial) must call this from `_start` so
/// the linker pulls in this module's object file. Without it, `--gc-sections`
/// can drop the whole object before Limine gets to scan the ELF, and the
/// bootloader would see no requests at all.
///
/// `#[inline(never)]` makes the call a real relocation against this
/// function's symbol; the volatile read inside prevents the body from
/// being dead-code-eliminated.
#[inline(never)]
pub fn ensure_linked() {
    let _ = BASE_REVISION.is_supported();
}

pub fn generation_module_optional() -> Option<&'static [u8]> {
    let modules = MODULES.response()?.modules();
    modules
        .iter()
        .find(|module| {
            module.cmdline() == "slime-generation-v2" || module.path().ends_with("generation-2.bin")
        })
        .map(|module| module.data())
}

pub fn generation_module() -> &'static [u8] {
    generation_module_optional().expect("generation module missing")
}
