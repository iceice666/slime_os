//! x86-64 boot mechanism for the `x86_64-qemu-virtio` profile.
//!
//! Stage-0's verified-generation selection, BootState handling, release
//! authorization, and rollback flow are architecture-neutral and stay in
//! `main.rs` and `lib.rs`. What is profile-specific lives here: 4-level page
//! tables for the kernel's virtual layout, the NX enable, and the entry-state
//! setup that transfers to the kernel.
//!
//! Every fallible step returns a [`BootError`]: stage-0 must never panic, since
//! a panic here bricks the boot path before any rollback machinery exists. The
//! crate denies `unwrap`, `expect`, `panic`, and slice indexing for that reason.

use alloc::vec::Vec;

use boot_contracts::handoff::KernelHandoffV1;
use boot_contracts::kernel_image::{SEGMENT_EXEC, SEGMENT_WRITE};
use slime_stage0::BootError;
use uefi::boot::{MemoryType, PAGE_SIZE};

use crate::{LoadedSegment, allocate_zeroed};

/// Present: the entry maps something.
const PAGE_PRESENT: u64 = 1;
/// Writable.
const PAGE_WRITE: u64 = 1 << 1;
/// Maps a large page directly rather than pointing at a lower-level table.
const PAGE_HUGE: u64 = 1 << 7;
/// No-execute. Requires EFER.NXE, enabled by [`enable_nxe`].
const PAGE_NX: u64 = 1 << 63;
/// Base of the kernel's direct map of physical memory.
pub const DIRECT_MAP_BASE: u64 = 0xffff_8000_0000_0000;

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

/// One page-table's worth of entries.
#[repr(align(4096))]
struct Page([u64; 512]);

/// The kernel's 4-level page-table hierarchy, built by stage-0.
pub struct PageTables {
    pml4: u64,
    pages: Vec<u64>,
}

impl PageTables {
    pub fn new() -> Result<Self, BootError> {
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

    pub fn map_identity(&mut self, bytes: u64) -> Result<(), BootError> {
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

    pub fn map_segments(&mut self, segments: &[LoadedSegment]) -> Result<(), BootError> {
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
    pub fn map_stack(&mut self, stack_phys: u64, bytes: usize) -> Result<u64, BootError> {
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

    pub unsafe fn activate(&self) {
        unsafe {
            core::arch::asm!(
                "mov cr3, {}",
                in(reg) self.pml4,
                options(nostack, preserves_flags)
            );
        }
    }
}

pub fn enable_nxe() -> Result<(), BootError> {
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

pub unsafe fn jump(entry: u64, handoff: *const KernelHandoffV1, stack_top: u64) -> ! {
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
