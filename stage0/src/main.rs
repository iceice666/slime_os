#![no_main]
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use boot_contracts::handoff::{
    HandoffFramebuffer, HandoffMemoryEntry, KernelHandoffV1, MAX_MEMORY_ENTRIES, MEMORY_RESERVED,
    MEMORY_USABLE,
};
use boot_contracts::kernel_image::{KernelImage, LOAD_BASE, SEGMENT_EXEC, SEGMENT_WRITE};
use boot_contracts::trace;
use slime_stage0::{
    BootError, Slot, decode_directory, select_bootstate_for_directory, select_generation,
    verify_generation, verify_kernel, verify_release,
};
use uefi::boot::{self, AllocateType, MemoryType, PAGE_SIZE};
use uefi::mem::memory_map::MemoryMap;
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType, RegularFile};
use uefi::{CString16, Status};

const BOOT_STORE_PATH: &str = "\\boot\\boot-store.bin";
const HEALTH_CONFIRM_PATH: &str = "\\boot\\health-confirm.bin";
const PAGE_PRESENT: u64 = 1;
const PAGE_WRITE: u64 = 1 << 1;
const PAGE_HUGE: u64 = 1 << 7;
const PAGE_NX: u64 = 1 << 63;
const DIRECT_MAP_BASE: u64 = 0xffff_8000_0000_0000;

/// Top (exclusive) of the kernel boot stack's dedicated virtual window.
///
/// The stack is mapped with 4 KiB pages into an otherwise-unused higher-half
/// PML4 slot (510 — the kernel uses 256/384/386/388/448/511), with the page
/// directly below its base left unmapped as a guard. Overflow past the base
/// faults deterministically at the guard instead of silently corrupting
/// whatever physical RAM happens to sit below the stack (previously the kernel
/// PML4 itself). Placing the stack at its own virtual address means the CPU
/// reaches it through RSP's VA, so the guard hole catches the overflow even
/// though the underlying frames stay aliased in the identity and direct maps.
const KERNEL_STACK_TOP_VA: u64 = 0xffff_ff00_0010_0000;

#[repr(align(4096))]
struct Page([u64; 512]);
const KERNEL_STACK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy)]
struct LoadedSegment {
    virtual_address: u64,
    physical_address: u64,
    page_count: usize,
    flags: u32,
}

struct PageTables {
    pml4: u64,
    pages: Vec<u64>,
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
    let slot_a: &[u8; 512] = store[..512].try_into().unwrap();
    let slot_b: &[u8; 512] = store[512..1024].try_into().unwrap();
    let mut selected_state = select_bootstate_for_directory(slot_a, slot_b, &directory)?;
    let selection_state = selected_state.state;
    let running_pending =
        selected_state.state.pending.is_some() && selected_state.state.remaining_attempts > 0;
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
    } else {
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
    let selected = select_generation(&directory, &selection_state)?;
    let confirmation_pending =
        selection_state.pending.is_some() && health_confirmation_matches(selection_state.pending);
    if confirmation_pending {
        verify_pending_for_promotion(&directory, &selection_state)?;
    }
    let generation = verify_generation(selected.bytes, &selected.identity)?;
    let release_sequence = verify_release(
        &selected,
        &generation,
        &selection_state,
        confirmation_pending
            || (selection_state.pending.is_some() && selection_state.remaining_attempts > 0),
    )?;
    if confirmation_pending {
        let before = selected_state.state;
        selected_state.state = selected_state
            .state
            .promote_pending(before.pending.unwrap(), release_sequence)
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
    let kernel = verify_kernel(&generation)?;

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

impl PageTables {
    fn new() -> Result<Self, BootError> {
        let pml4 = allocate_zeroed(PAGE_SIZE, MemoryType::LOADER_DATA)? as u64;
        Ok(Self {
            pml4,
            pages: alloc::vec![pml4],
        })
    }

    fn table(&mut self, parent: u64, index: usize) -> Result<u64, BootError> {
        let entries = unsafe { &mut *(parent as *mut Page) };
        let entry = entries.0[index];
        if entry & PAGE_PRESENT != 0 {
            if entry & PAGE_HUGE != 0 {
                return Err(BootError::PageTableExhausted);
            }
            return Ok(entry & 0x000f_ffff_ffff_f000);
        }
        let child = allocate_zeroed(PAGE_SIZE, MemoryType::LOADER_DATA)? as u64;
        self.pages.push(child);
        entries.0[index] = child | PAGE_PRESENT | PAGE_WRITE;
        Ok(child)
    }

    fn map_4k(
        &mut self,
        virtual_address: u64,
        physical_address: u64,
        flags: u64,
    ) -> Result<(), BootError> {
        let pml4_index = ((virtual_address >> 39) & 0x1ff) as usize;
        let pdpt_index = ((virtual_address >> 30) & 0x1ff) as usize;
        let pd_index = ((virtual_address >> 21) & 0x1ff) as usize;
        let pt_index = ((virtual_address >> 12) & 0x1ff) as usize;
        let pdpt = self.table(self.pml4, pml4_index)?;
        let pd = self.table(pdpt, pdpt_index)?;
        let pt = self.table(pd, pd_index)?;
        let entries = unsafe { &mut *(pt as *mut Page) };
        entries.0[pt_index] = physical_address | PAGE_PRESENT | flags;
        Ok(())
    }

    fn map_identity(&mut self, bytes: u64) -> Result<(), BootError> {
        let direct_pml4 = ((DIRECT_MAP_BASE >> 39) & 0x1ff) as usize;
        let pml4_count = bytes.div_ceil(512 << 30) as usize;
        for pml4_offset in 0..pml4_count {
            let identity_pdpt = self.table(self.pml4, pml4_offset)?;
            let direct_pdpt = self.table(self.pml4, direct_pml4 + pml4_offset)?;
            let base_gb = pml4_offset as u64 * 512;
            let gb_count = core::cmp::min(512, bytes.div_ceil(1 << 30).saturating_sub(base_gb));
            for gb in 0..gb_count {
                let identity_pd = self.table(identity_pdpt, gb as usize)?;
                let direct_pd = self.table(direct_pdpt, gb as usize)?;
                for mb in 0..512u64 {
                    let physical = (base_gb + gb) * (1 << 30) + mb * (1 << 21);
                    let entry = physical | PAGE_PRESENT | PAGE_WRITE | PAGE_HUGE;
                    unsafe {
                        (&mut *(identity_pd as *mut Page)).0[mb as usize] = entry;
                        (&mut *(direct_pd as *mut Page)).0[mb as usize] = entry;
                    }
                }
            }
        }
        Ok(())
    }

    fn map_segments(&mut self, segments: &[LoadedSegment]) -> Result<(), BootError> {
        for segment in segments {
            let flags = (if segment.flags & SEGMENT_WRITE != 0 {
                PAGE_WRITE
            } else {
                0
            }) | (if segment.flags & SEGMENT_EXEC == 0 {
                PAGE_NX
            } else {
                0
            });
            for page in 0..segment.page_count as u64 {
                self.map_4k(
                    segment.virtual_address + page * PAGE_SIZE as u64,
                    segment.physical_address + page * PAGE_SIZE as u64,
                    flags,
                )?;
            }
        }
        Ok(())
    }

    /// Map `bytes` of kernel boot stack, whose frames start at physical
    /// `stack_phys`, into the dedicated virtual window ending at
    /// [`KERNEL_STACK_TOP_VA`], leaving one unmapped guard page below the base.
    /// Returns the top-of-stack virtual address to load into RSP.
    fn map_stack(&mut self, stack_phys: u64, bytes: usize) -> Result<u64, BootError> {
        let page = PAGE_SIZE as u64;
        let bytes = bytes as u64;
        let base_va = KERNEL_STACK_TOP_VA
            .checked_sub(bytes)
            .ok_or(BootError::AddressOverflow)?;
        // The guard page sits at base_va - PAGE_SIZE and is intentionally never
        // mapped, so a downward overflow past the stack base faults.
        let pages = bytes / page;
        for index in 0..pages {
            self.map_4k(
                base_va + index * page,
                stack_phys + index * page,
                PAGE_WRITE | PAGE_NX,
            )?;
        }
        Ok(KERNEL_STACK_TOP_VA)
    }

    unsafe fn activate(&self) {
        unsafe {
            core::arch::asm!(
                "mov cr3, {}",
                in(reg) self.pml4,
                options(nostack, preserves_flags)
            );
        }
    }
}

fn enable_nxe() -> Result<(), BootError> {
    let extended = core::arch::x86_64::__cpuid(0x8000_0000);
    if extended.eax < 0x8000_0001 {
        return Err(BootError::PageTableExhausted);
    }
    let features = core::arch::x86_64::__cpuid(0x8000_0001);
    if features.edx & (1 << 20) == 0 {
        return Err(BootError::PageTableExhausted);
    }
    const EFER: u32 = 0xc000_0080;
    const EFER_NXE: u64 = 1 << 11;
    let low: u32;
    let high: u32;
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") EFER,
            out("eax") low,
            out("edx") high,
            options(nostack, preserves_flags),
        );
        let value = ((high as u64) << 32) | low as u64 | EFER_NXE;
        core::arch::asm!(
            "wrmsr",
            in("ecx") EFER,
            in("eax") value as u32,
            in("edx") (value >> 32) as u32,
            options(nostack, preserves_flags),
        );
    }
    Ok(())
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

fn framebuffer_info() -> Result<HandoffFramebuffer, BootError> {
    let handle = boot::get_handle_for_protocol::<GraphicsOutput>()
        .map_err(|_| BootError::MissingFramebuffer)?;
    let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| BootError::MissingFramebuffer)?;
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

unsafe fn jump(entry: u64, handoff: *const KernelHandoffV1, stack_top: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "mov rsp, {stack}",
            "and rsp, -16",
            "push {entry}",
            "ret",
            entry = in(reg) entry,
            stack = in(reg) stack_top,
            in("rdi") handoff,
            options(noreturn)
        )
    }
}
