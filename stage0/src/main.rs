#![no_main]
#![no_std]
// Stage-0 must never panic: a panic here bricks the boot path before any
// rollback machinery exists. Every fallible step must return a BootError.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

extern crate alloc;

use alloc::vec::Vec;
use boot_contracts::handoff::{
    HandoffFramebuffer, HandoffMemoryEntry, KernelHandoffV1, MAX_MEMORY_ENTRIES, MEMORY_RESERVED,
    MEMORY_USABLE,
};
use boot_contracts::kernel_image::{KernelImage, LOAD_BASE};
use boot_contracts::trace;
use slime_stage0::{
    BootError, Slot, admit_generation_closure, decode_directory, select_bootstate_for_directory,
    select_generation, verify_generation, verify_release,
};
use uefi::boot::{self, AllocateType, MemoryType, PAGE_SIZE};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType, RegularFile};
use uefi::{CString16, Status};

const BOOT_STORE_PATH: &str = "\\boot\\boot-store.bin";
const HEALTH_CONFIRM_PATH: &str = "\\boot\\health-confirm.bin";
const KERNEL_STACK_BYTES: usize = 256 * 1024;

mod arch;

use arch::target::{DIRECT_MAP_BASE, PageTables, enable_nxe, jump};

#[derive(Clone, Copy)]
pub(crate) struct LoadedSegment {
    virtual_address: u64,
    physical_address: u64,
    page_count: usize,
    flags: u32,
}

#[uefi::entry]
fn main() -> Status {
    match boot() {
        Ok(()) => Status::SUCCESS,
        Err(error) => {
            uefi::println!("[stage0] boot failed: {:?}", error);
            Status::LOAD_ERROR
        }
    }
}

fn boot() -> Result<(), BootError> {
    uefi::helpers::init().map_err(|_| BootError::Truncated)?;
    uefi::println!("[stage0] immutable selector");

    let store = read_file(BOOT_STORE_PATH)?;
    let directory = decode_directory(&store)?;
    let slot_a: &[u8; 512] = store
        .get(..512)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(BootError::Truncated)?;
    let slot_b: &[u8; 512] = store
        .get(512..1024)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(BootError::Truncated)?;
    let mut selected_state = select_bootstate_for_directory(slot_a, slot_b, &directory)?;
    let selection_state = selected_state.state;
    let running_pending =
        selection_state.pending.is_some() && selection_state.remaining_attempts > 0;
    if running_pending {
        let before = selected_state.state;
        selected_state.state = selected_state
            .state
            .consume_pending_attempt()
            .map_err(|_| BootError::NoValidBootState)?;
        let target = match selected_state.slot {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        };
        // Commit before touching untrusted pending-generation bytes. Any
        // selection, image-admission, or release failure then consumes one
        // bounded attempt and eventually returns to known-good.
        persist_bootstate(target, selected_state.state)?;
        emit_trace(&trace::Record {
            action: trace::Action::ConsumeAttempt,
            commit: trace::Commit::AfterAttemptCommit,
            selected_slot: slot_index(selected_state.slot),
            target_slot: Some(slot_index(target)),
            sequence_before: before.sequence,
            sequence_after: selected_state.state.sequence,
            attempts_before: before.remaining_attempts,
            attempts_after: selected_state.state.remaining_attempts,
            known_good: selected_state.state.known_good,
            pending: selected_state.state.pending,
            generation_root: selected_state.state.generation_root,
            state_root: selected_state.state.state_root,
        });
        selected_state.slot = target;
    }
    let selected = select_generation(&directory, &selection_state)?;
    let generation = verify_generation(selected.bytes, &selected.identity)?;
    let kernel = admit_generation_closure(&generation)?;
    let confirmation_pending =
        selection_state.pending.is_some() && health_confirmation_matches(selection_state.pending);
    if confirmation_pending {
        verify_pending_for_promotion(&directory, &selection_state)?;
    }
    let release_sequence = verify_release(
        &selected,
        &generation,
        &selection_state,
        confirmation_pending
            || (selection_state.pending.is_some() && selection_state.remaining_attempts > 0),
    )?;

    if !running_pending {
        let state = selected_state.state;
        emit_trace(&trace::Record {
            action: if state.pending.is_some() {
                trace::Action::BootExhaustedKnownGood
            } else {
                trace::Action::BootKnownGood
            },
            commit: trace::Commit::None,
            selected_slot: slot_index(selected_state.slot),
            target_slot: None,
            sequence_before: state.sequence,
            sequence_after: state.sequence,
            attempts_before: state.remaining_attempts,
            attempts_after: state.remaining_attempts,
            known_good: state.known_good,
            pending: state.pending,
            generation_root: state.generation_root,
            state_root: state.state_root,
        });
    }
    if confirmation_pending {
        let before = selected_state.state;
        let pending = before.pending.ok_or(BootError::NoValidBootState)?;
        selected_state.state = selected_state
            .state
            .promote_pending(pending, release_sequence)
            .map_err(|_| BootError::NoValidBootState)?;
        let target = match selected_state.slot {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        };
        consume_health_confirmation()?;
        persist_bootstate(target, selected_state.state)?;
        emit_trace(&trace::Record {
            action: trace::Action::Promotion,
            commit: trace::Commit::HealthPromotion,
            selected_slot: slot_index(selected_state.slot),
            target_slot: Some(slot_index(target)),
            sequence_before: before.sequence,
            sequence_after: selected_state.state.sequence,
            attempts_before: before.remaining_attempts,
            attempts_after: selected_state.state.remaining_attempts,
            known_good: selected_state.state.known_good,
            pending: selected_state.state.pending,
            generation_root: selected_state.state.generation_root,
            state_root: selected_state.state.state_root,
        });
        selected_state.slot = target;
    }
    let generation_copy = allocate_bytes(selected.bytes)?;
    let framebuffer = framebuffer_info()?;
    let (segments, entry) = load_kernel(&kernel)?;
    let stack = allocate_zeroed(KERNEL_STACK_BYTES, MemoryType::LOADER_DATA)?;
    let mut tables = PageTables::new()?;
    let framebuffer_end = framebuffer
        .address
        .checked_add(framebuffer.pitch.saturating_mul(framebuffer.height))
        .ok_or(BootError::AddressOverflow)?;
    let direct_map_end =
        core::cmp::max(max_physical_address()?, framebuffer_end).next_multiple_of(1 << 30);
    tables.map_identity(direct_map_end)?;
    tables.map_segments(&segments)?;
    // Map the boot stack at its dedicated guarded virtual window (must run after
    // the identity/direct maps so it lands in a fresh, huge-page-free slot) and
    // hand the kernel that VA as its initial RSP.
    let stack_top = tables.map_stack(stack as u64, KERNEL_STACK_BYTES)?;
    enable_nxe()?;

    let memory = allocate_zeroed(
        core::mem::size_of::<HandoffMemoryEntry>() * MAX_MEMORY_ENTRIES,
        MemoryType::LOADER_DATA,
    )? as *mut HandoffMemoryEntry;
    let handoff = allocate_zeroed(
        core::mem::size_of::<KernelHandoffV1>(),
        MemoryType::LOADER_DATA,
    )? as *mut KernelHandoffV1;
    unsafe {
        handoff.write(KernelHandoffV1 {
            magic: boot_contracts::handoff::HANDOFF_MAGIC,
            version: boot_contracts::handoff::HANDOFF_VERSION,
            size: core::mem::size_of::<KernelHandoffV1>() as u32,
            direct_map_offset: DIRECT_MAP_BASE,
            memory_map_ptr: core::ptr::null(),
            memory_map_len: 0,
            reserved0: 0,
            framebuffer,
            rsdp_address: rsdp_address(),
            generation_ptr: generation_copy,
            generation_len: selected.bytes.len() as u64,
            generation_identity: selected.identity,
            bootstate_sequence: selected_state.state.sequence,
            known_good_identity: selected_state.state.known_good,
            pending_identity: selected_state.state.pending.unwrap_or([0; 32]),
            remaining_attempts: selected_state.state.remaining_attempts,
            bootstate_slot: match selected_state.slot {
                Slot::A => 0,
                Slot::B => 1,
            },
            running_pending: u8::from(running_pending && !confirmation_pending),
            reserved1: [0; 2],
            generation_root: selected_state.state.generation_root,
            state_root: selected_state.state.state_root,
            accepted_release_sequence: selected_state.state.accepted_release_sequence,
            running_release_sequence: release_sequence,
        });
    }

    uefi::println!("[stage0] generation and kernel verified");
    let final_map = unsafe { boot::exit_boot_services(Some(MemoryType::LOADER_DATA)) };
    let mut count = 0usize;
    for descriptor in final_map.entries() {
        let kind = if descriptor.ty == MemoryType::CONVENTIONAL {
            MEMORY_USABLE
        } else {
            MEMORY_RESERVED
        };
        push_memory_entry(
            memory,
            &mut count,
            HandoffMemoryEntry {
                base: descriptor.phys_start,
                length: descriptor.page_count * PAGE_SIZE as u64,
                kind,
            },
        )?;
    }
    unsafe {
        (*handoff).memory_map_ptr = memory;
        (*handoff).memory_map_len = count as u32;
        tables.activate();
        jump(entry, handoff, stack_top)
    }
}

fn push_memory_entry(
    memory: *mut HandoffMemoryEntry,
    count: &mut usize,
    entry: HandoffMemoryEntry,
) -> Result<(), BootError> {
    if *count >= MAX_MEMORY_ENTRIES {
        return Err(BootError::TooManyMemoryEntries);
    }
    unsafe { memory.add(*count).write(entry) };
    *count += 1;
    Ok(())
}

fn open_regular(path: &str, mode: FileMode) -> Result<RegularFile, BootError> {
    let mut fs =
        boot::get_image_file_system(boot::image_handle()).map_err(|_| BootError::Truncated)?;
    let mut root = fs.open_volume().map_err(|_| BootError::Truncated)?;
    let path = CString16::try_from(path).map_err(|_| BootError::Truncated)?;
    let file = root
        .open(&path, mode, FileAttribute::empty())
        .map_err(|_| BootError::Truncated)?;
    match file.into_type().map_err(|_| BootError::Truncated)? {
        FileType::Regular(file) => Ok(file),
        _ => Err(BootError::Truncated),
    }
}

fn read_file(path: &str) -> Result<Vec<u8>, BootError> {
    let mut file = open_regular(path, FileMode::Read)?;
    read_regular(&mut file)
}

fn verify_pending_for_promotion(
    directory: &slime_stage0::BootDirectory<'_>,
    state: &boot_contracts::bootstate::BootState,
) -> Result<(), BootError> {
    let pending = state.pending.ok_or(BootError::MissingGeneration)?;
    for index in 0..directory.count() {
        let entry = directory.entry(index)?;
        if entry.identity == pending {
            let generation = verify_generation(entry.bytes, &entry.identity)?;
            admit_generation_closure(&generation)?;
            verify_release(&entry, &generation, state, true)?;
            return Ok(());
        }
    }
    Err(BootError::MissingGeneration)
}

fn consume_health_confirmation() -> Result<(), BootError> {
    open_regular(HEALTH_CONFIRM_PATH, FileMode::ReadWrite)?
        .delete()
        .map_err(|_| BootError::Truncated)
}

fn health_confirmation_matches(pending: Option<[u8; 32]>) -> bool {
    let Some(pending) = pending else {
        return false;
    };
    let Ok(bytes) = read_file(HEALTH_CONFIRM_PATH) else {
        return false;
    };
    bytes.as_slice() == pending
}

fn persist_bootstate(
    slot: Slot,
    state: boot_contracts::bootstate::BootState,
) -> Result<(), BootError> {
    let offset = match slot {
        Slot::A => 0,
        Slot::B => boot_contracts::bootstate::SLOT_BYTES as u64,
    };
    let encoded = state.encode().map_err(|_| BootError::NoValidBootState)?;
    let mut file = open_regular(BOOT_STORE_PATH, FileMode::ReadWrite)?;
    file.set_position(offset)
        .map_err(|_| BootError::Truncated)?;
    file.write(&encoded).map_err(|_| BootError::Truncated)?;
    file.flush().map_err(|_| BootError::Truncated)?;
    Ok(())
}

const fn slot_index(slot: Slot) -> u8 {
    match slot {
        Slot::A => 0,
        Slot::B => 1,
    }
}

fn emit_trace(record: &trace::Record) {
    uefi::println!("{}", record.render().as_str());
}

fn read_regular(file: &mut RegularFile) -> Result<Vec<u8>, BootError> {
    let info = file
        .get_boxed_info::<uefi::proto::media::file::FileInfo>()
        .map_err(|_| BootError::Truncated)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(info.file_size() as usize)
        .map_err(|_| BootError::Truncated)?;
    bytes.resize(info.file_size() as usize, 0);
    let mut offset = 0;
    while offset < bytes.len() {
        let read = file
            .read(&mut bytes[offset..])
            .map_err(|_| BootError::Truncated)?;
        if read == 0 {
            return Err(BootError::Truncated);
        }
        offset += read;
    }
    Ok(bytes)
}

fn allocate_bytes(bytes: &[u8]) -> Result<*const u8, BootError> {
    let pointer = allocate_zeroed(bytes.len(), MemoryType::LOADER_DATA)?;
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len()) };
    Ok(pointer)
}

fn allocate_zeroed(bytes: usize, memory_type: MemoryType) -> Result<*mut u8, BootError> {
    let pages = bytes.div_ceil(PAGE_SIZE);
    let address = boot::allocate_pages(AllocateType::AnyPages, memory_type, pages)
        .map_err(|_| BootError::AddressOverflow)?;
    let pointer = address.as_ptr();
    unsafe { core::ptr::write_bytes(pointer, 0, pages * PAGE_SIZE) };
    Ok(pointer)
}

fn load_kernel(image: &KernelImage<'_>) -> Result<(Vec<LoadedSegment>, u64), BootError> {
    let mut loaded = Vec::new();
    for index in 0..image.segment_count() {
        let segment = image
            .segment(index)
            .map_err(|_| BootError::BadKernelImage)?;
        let page_count = (segment.mem_len as usize).div_ceil(PAGE_SIZE);
        let address =
            boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, page_count)
                .map_err(|_| BootError::AddressOverflow)?;
        let pointer = address.as_ptr();
        unsafe {
            core::ptr::write_bytes(pointer, 0, page_count * PAGE_SIZE);
            core::ptr::copy_nonoverlapping(segment.bytes.as_ptr(), pointer, segment.bytes.len());
        }
        loaded.push(LoadedSegment {
            virtual_address: LOAD_BASE + segment.vaddr_offset,
            physical_address: pointer as u64,
            page_count,
            flags: segment.flags,
        });
    }

    for index in 0..image.relocation_count() {
        let relocation = image
            .relocation(index)
            .map_err(|_| BootError::BadKernelImage)?;
        let target = loaded
            .iter()
            .find_map(|segment| {
                let offset = relocation
                    .target_offset
                    .checked_sub(segment.virtual_address - LOAD_BASE)?;
                (offset + 8 <= segment.page_count as u64 * PAGE_SIZE as u64)
                    .then_some(segment.physical_address + offset)
            })
            .ok_or(BootError::BadKernelImage)?;
        let addend = relocation.addend as u64;
        let value = LOAD_BASE
            .checked_add(addend.wrapping_sub(image.preferred_base))
            .ok_or(BootError::AddressOverflow)?;
        unsafe { (target as *mut u64).write_unaligned(value) };
    }

    Ok((loaded, LOAD_BASE + image.entry_offset))
}

fn max_physical_address() -> Result<u64, BootError> {
    let map = boot::memory_map(MemoryType::LOADER_DATA).map_err(|_| BootError::AddressOverflow)?;
    map.entries()
        .filter_map(|entry| {
            entry
                .phys_start
                .checked_add(entry.page_count * PAGE_SIZE as u64)
        })
        .max()
        .map(|end| end.next_multiple_of(1 << 30))
        .ok_or(BootError::AddressOverflow)
}

/// The handoff encoding for "this machine has no framebuffer": a zero address
/// and zero geometry.
///
/// A headless machine is a legitimate configuration, not a boot failure — the
/// serial console is the diagnostic channel that must always exist, and the
/// `aarch64-qemu-virt` profile is booted headless by its own gate. The kernel's
/// framebuffer console checks for this and stays uninitialized; the direct-map
/// sizing math already treats a zero-length framebuffer as contributing no
/// range.
const ABSENT_FRAMEBUFFER: HandoffFramebuffer = HandoffFramebuffer {
    address: 0,
    width: 0,
    height: 0,
    pitch: 0,
    bpp: 0,
    memory_model: 0,
    red_mask_size: 0,
    red_mask_shift: 0,
    green_mask_size: 0,
    green_mask_shift: 0,
    blue_mask_size: 0,
    blue_mask_shift: 0,
    reserved: [0; 5],
};

/// Describe the firmware's framebuffer, or report its absence.
///
/// Absence is distinguished from a framebuffer that exists in an unsupported
/// pixel format: the latter still fails closed with
/// [`BootError::UnsupportedFramebuffer`], because a present device we cannot
/// describe correctly must not be handed over as if it were describable.
fn framebuffer_info() -> Result<HandoffFramebuffer, BootError> {
    let Ok(handle) = boot::get_handle_for_protocol::<GraphicsOutput>() else {
        return Ok(ABSENT_FRAMEBUFFER);
    };
    let Ok(mut gop) = boot::open_protocol_exclusive::<GraphicsOutput>(handle) else {
        return Ok(ABSENT_FRAMEBUFFER);
    };
    let info = gop.current_mode_info();
    let (
        red_mask_size,
        red_mask_shift,
        green_mask_size,
        green_mask_shift,
        blue_mask_size,
        blue_mask_shift,
    ) = match info.pixel_format() {
        PixelFormat::Rgb => (8, 0, 8, 8, 8, 16),
        PixelFormat::Bgr => (8, 16, 8, 8, 8, 0),
        _ => return Err(BootError::UnsupportedFramebuffer),
    };
    let (width, height) = info.resolution();
    let stride = info.stride();
    let address = gop.frame_buffer().as_mut_ptr() as u64;
    Ok(HandoffFramebuffer {
        address,
        width: width as u64,
        height: height as u64,
        pitch: (stride * 4) as u64,
        bpp: 32,
        memory_model: 1,
        red_mask_size,
        red_mask_shift,
        green_mask_size,
        green_mask_shift,
        blue_mask_size,
        blue_mask_shift,
        reserved: [0; 5],
    })
}

fn rsdp_address() -> u64 {
    uefi::system::with_config_table(|tables| {
        tables
            .iter()
            .find(|table| table.guid == uefi::table::cfg::ConfigTableEntry::ACPI2_GUID)
            .or_else(|| {
                tables
                    .iter()
                    .find(|table| table.guid == uefi::table::cfg::ConfigTableEntry::ACPI_GUID)
            })
            .map_or(0, |table| table.address as u64)
    })
}
