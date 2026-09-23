//! Ownership of dynamic mapping tables and the pages that keep them live.
use super::{AllocError, ExtentKind, ObjectAllocator, TaskArenaId};

// A shared mapping is contiguous and at most 64 pages: it crosses at most
// two spans at each table level. Device mappings add one page per record.
const LEVELS: usize = sel4::vspace_levels::NUM_LEVELS - 1;
pub(super) const RECORDS: usize = LEVELS
    * (2 * crate::shared_buffer::MAX_MAPPINGS
        + crate::io_resource::MAX_DMA_MAPPINGS
        + crate::io_resource::MAX_MMIO_MAPPINGS
        + super::MAX_TASK_ARENAS);
const PAGES: usize = crate::shared_buffer::MAX_MAPPING_PAGES
    + crate::io_resource::MAX_DMA_MAPPINGS
    + crate::io_resource::MAX_MMIO_MAPPINGS;

#[derive(Clone, Copy)]
struct MappingTable {
    arena: TaskArenaId,
    vspace: usize,
    level: usize,
    address: usize,
    slot: usize,
    extent: usize,
    mapped: bool,
    retired: bool,
}

#[derive(Clone, Copy)]
struct MappedPage {
    arena: TaskArenaId,
    vspace: usize,
    address: usize,
}

pub(super) struct MappingTables {
    records: [Option<MappingTable>; RECORDS],
    pages: [Option<MappedPage>; PAGES],
}

impl MappingTables {
    pub const fn new() -> Self {
        Self {
            records: [None; RECORDS],
            pages: [None; PAGES],
        }
    }
    pub fn forget(&mut self, arena: TaskArenaId) {
        for entry in &mut self.records {
            if entry.is_some_and(|entry| entry.arena == arena) {
                *entry = None;
            }
        }
        for entry in &mut self.pages {
            if entry.is_some_and(|entry| entry.arena == arena) {
                *entry = None;
            }
        }
    }
    fn used(&self, table: MappingTable) -> bool {
        let mask = !((1usize << sel4::vspace_levels::span_bits(table.level)) - 1);
        self.pages.iter().flatten().any(|page| {
            page.arena == table.arena
                && page.vspace == table.vspace
                && page.address & mask == table.address
        })
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;

    #[test]
    fn table_liveness_is_scoped_to_vspace_lifetime_and_span() {
        std::thread::Builder::new()
            .stack_size(4 * 1024 * 1024)
            .spawn(|| {
                let mut tables = MappingTables::new();
                let arena = TaskArenaId::from_raw(0, 1);
                let successor = TaskArenaId::from_raw(0, 2);
                let table = MappingTable {
                    arena,
                    vspace: 30,
                    level: LEVELS,
                    address: 0,
                    slot: 20,
                    extent: 0,
                    mapped: true,
                    retired: false,
                };
                tables.records[0] = Some(table);
                tables.pages[0] = Some(MappedPage {
                    arena,
                    vspace: 30,
                    address: 4096,
                });
                tables.pages[1] = Some(MappedPage {
                    arena,
                    vspace: 30,
                    address: 8192,
                });
                assert!(tables.used(table));
                tables.pages[0] = None;
                assert!(
                    tables.used(table),
                    "a second device or buffer page still needs the table"
                );
                tables.pages[1] = Some(MappedPage {
                    arena,
                    vspace: 31,
                    address: 8192,
                });
                assert!(!tables.used(table));
                tables.pages[1] = Some(MappedPage {
                    arena: successor,
                    vspace: 30,
                    address: 8192,
                });
                assert!(!tables.used(table));
                tables.pages[1] = Some(MappedPage {
                    arena,
                    vspace: 30,
                    address: 1 << sel4::vspace_levels::span_bits(LEVELS),
                });
                assert!(!tables.used(table));
                tables.pages[2] = Some(MappedPage {
                    arena: successor,
                    vspace: 30,
                    address: 0,
                });
                tables.forget(arena);
                assert!(tables.records[0].is_none());
                assert!(tables.pages[1].is_none());
                assert!(
                    tables.pages[2].is_some(),
                    "retiring an arena must not erase its successor"
                );
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn table_cleanup_failure_retains_ownership_until_retry() {
        std::thread::Builder::new()
            .stack_size(32 * 1024 * 1024)
            .spawn(|| {
                let mut allocator = ObjectAllocator::empty();
                let arena = allocator.arena_owning_slots_for_test(1);
                let mut extent =
                    super::super::ExtentRecord::new(sel4::cap::Untyped::from_bits(10), 12);
                extent.assign(arena.index(), arena.serial, ExtentKind::MappingTables);
                extent.objects = 1;
                extent.bytes = 4096;
                allocator.extents[0] = Some(extent);
                allocator.allocations[0] = super::super::AllocationRecord {
                    owner: arena.index() as u16,
                    extent: 0,
                    serial: arena.serial,
                    allocation: super::super::ArenaAllocation::new(20, 12, false, false),
                    next_state: super::super::PRIVATE_STATE_NONE,
                };
                allocator.mapping_tables.records[0] = Some(MappingTable {
                    arena,
                    vspace: 30,
                    level: 1,
                    address: 0,
                    slot: 20,
                    extent: 0,
                    mapped: true,
                    retired: false,
                });
                allocator.live_objects = 1;
                allocator.live_bytes = 4096;
                assert!(
                    allocator
                        .collect_mapping_tables_with(
                            arena,
                            |_| Err(AllocError::NoKernelUntyped),
                            |_| panic!("revoke before delete"),
                        )
                        .is_err()
                );
                assert!(!allocator.mapping_tables.records[0].unwrap().retired);
                assert!(
                    allocator
                        .collect_mapping_tables_with(
                            arena,
                            |_| Ok(()),
                            |_| Err(AllocError::NoKernelUntyped),
                        )
                        .is_err()
                );
                assert!(allocator.mapping_tables.records[0].unwrap().retired);
                assert!(allocator.extents[0].unwrap().active);
                assert_eq!(allocator.live_objects, 1);
                allocator
                    .collect_mapping_tables_with(
                        arena,
                        |_| panic!("completed delete repeated"),
                        |_| Ok(()),
                    )
                    .unwrap();
                assert!(allocator.mapping_tables.records[0].is_none());
                assert!(!allocator.extents[0].unwrap().active);
                assert_eq!(allocator.live_objects, 0);
                assert_eq!(allocator.live_bytes, 0);
                assert_eq!(allocator.arena_slot_count(arena), Ok(0));
            })
            .unwrap()
            .join()
            .unwrap();
    }
}

impl ObjectAllocator {
    fn vspace_arena(&self, vspace: usize) -> Result<TaskArenaId, AllocError> {
        let record = self
            .allocations
            .iter()
            .find(|record| record.owner != u16::MAX && record.allocation.slot() == vspace)
            .ok_or(AllocError::NoKernelUntyped)?;
        let arena = TaskArenaId::from_raw(record.owner, record.serial);
        self.private_arena(arena)?;
        Ok(arena)
    }

    pub fn mapping_page_tracked(&self, vspace: usize, address: usize) -> bool {
        self.mapping_tables
            .pages
            .iter()
            .flatten()
            .any(|page| page.vspace == vspace && page.address == address)
    }

    /// Reserve page ownership before installing any table or leaf mapping.
    /// Failed installation must call `release_mapping_page` to unwind it.
    pub fn prepare_mapping_page(
        &mut self,
        vspace: usize,
        address: usize,
    ) -> Result<(), AllocError> {
        let arena = self.vspace_arena(vspace)?;
        if self
            .mapping_tables
            .pages
            .iter()
            .flatten()
            .any(|page| page.arena == arena && page.vspace == vspace && page.address == address)
        {
            return Err(AllocError::ArenaTooSmall {
                size_bits: 12,
                required: 4096,
            });
        }
        self.collect_mapping_tables(arena)?;
        let slot = self
            .mapping_tables
            .pages
            .iter()
            .position(Option::is_none)
            .ok_or(AllocError::ArenaTableFull { limit: PAGES })?;
        self.mapping_tables.pages[slot] = Some(MappedPage {
            arena,
            vspace,
            address,
        });
        Ok(())
    }

    /// Called only after the frame/device mapping was removed (or never
    /// installed). Kernel failures retain table records for the next retry.
    pub fn release_mapping_page(
        &mut self,
        vspace: usize,
        address: usize,
    ) -> Result<(), AllocError> {
        let arena = self.vspace_arena(vspace)?;
        for page in &mut self.mapping_tables.pages {
            if page.is_some_and(|page| {
                page.arena == arena && page.vspace == vspace && page.address == address
            }) {
                *page = None;
            }
        }
        // The leaf is already gone. A table-GC failure must not masquerade as
        // failed leaf unmap: seal rollback restores only earlier pages. Retain
        // the table's ownership and retry at prepare or arena reclamation.
        let _ = self.collect_mapping_tables(arena);
        Ok(())
    }

    fn collect_mapping_tables(&mut self, arena: TaskArenaId) -> Result<(), AllocError> {
        self.collect_mapping_tables_with(
            arena,
            |slot| {
                sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                    .delete()
                    .map_err(|error| AllocError::ArenaCleanup { slot, error })
            },
            |slot| {
                sel4::init_thread::slot::CNODE
                    .cap()
                    .absolute_cptr(sel4::CPtr::from_bits(slot as _))
                    .revoke()
                    .map_err(|error| AllocError::ArenaCleanup { slot, error })
            },
        )
    }

    fn collect_mapping_tables_with(
        &mut self,
        arena: TaskArenaId,
        mut delete: impl FnMut(usize) -> Result<(), AllocError>,
        mut revoke: impl FnMut(usize) -> Result<(), AllocError>,
    ) -> Result<(), AllocError> {
        // Child tables are removed before a parent can be finalized.
        for level in (1..sel4::vspace_levels::NUM_LEVELS).rev() {
            for index in 0..RECORDS {
                let Some(table) = self.mapping_tables.records[index] else {
                    continue;
                };
                if table.arena != arena || table.level != level || self.mapping_tables.used(table) {
                    continue;
                }
                if !table.retired {
                    delete(table.slot)?;
                    self.mapping_tables.records[index].as_mut().unwrap().retired = true;
                }
                // The table has its own backing extent. Resetting this parent
                // cannot revoke any sibling mapping or loader-owned table.
                let extent = self.extents[table.extent].ok_or(AllocError::NoKernelUntyped)?;
                revoke(extent.parent.bits() as usize)?;
                self.live_objects -= extent.objects;
                self.live_bytes -= extent.bytes;
                let position = self
                    .allocations
                    .iter()
                    .position(|record| {
                        record.belongs_to(arena) && record.allocation.slot() == table.slot
                    })
                    .ok_or(AllocError::NoKernelUntyped)?;
                self.allocations[position] = super::AllocationRecord::EMPTY;
                self.allocation_search_start = self.allocation_search_start.min(position);
                self.arena_mut(arena)?.slot_len -= 1;
                self.slots.release(table.slot);
                let extent = self.extents[table.extent].as_mut().unwrap();
                extent.active = false;
                extent.revoked = false;
                extent.owner = u16::MAX;
                extent.serial = 0;
                extent.watermark = 0;
                extent.objects = 0;
                extent.bytes = 0;
                self.mapping_tables.records[index] = None;
            }
        }
        Ok(())
    }

    pub fn ensure_owned_mapping_table(
        &mut self,
        vspace: sel4::cap::VSpace,
        level: usize,
        address: usize,
    ) -> Result<bool, AllocError> {
        let ty = sel4::TranslationTableObjectType::from_level(level)
            .ok_or(AllocError::NoKernelUntyped)?;
        let arena = self.vspace_arena(vspace.bits() as usize)?;
        if self.mapping_tables.records.iter().flatten().any(|entry| {
            entry.arena == arena
                && entry.vspace == vspace.bits() as usize
                && entry.level == level
                && entry.address == address
                && entry.mapped
                && !entry.retired
        }) {
            return Ok(false);
        }
        let reusable = self.mapping_tables.records.iter().position(|entry| {
            entry.is_some_and(|entry| {
                entry.arena == arena && entry.level == level && !entry.mapped && !entry.retired
            })
        });
        let index = reusable
            .or_else(|| self.mapping_tables.records.iter().position(Option::is_none))
            .ok_or(AllocError::ArenaTableFull { limit: RECORDS })?;
        let slot = if let Some(entry) = self.mapping_tables.records[index] {
            entry.slot
        } else {
            self.ensure_allocation_descriptors(1)?;
            self.allocation_position()?;
            let bits = ty.blueprint().physical_size_bits();
            let extent = if let Ok((extent, _)) =
                self.extent_for_allocation(arena, ExtentKind::MappingTables, bits)
            {
                extent
            } else {
                self.provision_extent(arena, bits, ExtentKind::MappingTables)?
            };
            let slot = self
                .allocate_in_kind(arena, ty.blueprint(), ExtentKind::MappingTables)?
                .index();
            self.mapping_tables.records[index] = Some(MappingTable {
                arena,
                vspace: vspace.bits() as usize,
                level,
                address,
                slot,
                extent,
                mapped: false,
                retired: false,
            });
            slot
        };
        match sel4::cap::UnspecifiedIntermediateTranslationTable::from_bits(slot as _)
            .generic_intermediate_translation_table_map(
                ty,
                vspace,
                address,
                sel4::VmAttributes::default(),
            ) {
            Ok(()) => {
                let entry = self.mapping_tables.records[index].as_mut().unwrap();
                entry.address = address;
                entry.mapped = true;
                Ok(true)
            }
            Err(sel4::Error::DeleteFirst) => Ok(false),
            Err(error) => Err(AllocError::Retype {
                size_bits: ty.blueprint().physical_size_bits(),
                error,
            }),
        }
    }
}
