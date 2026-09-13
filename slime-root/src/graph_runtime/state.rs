use super::*;

/// Rights recorded on a shared buffer's own slot.
///
/// The region's real authority lives in the `BufferHandle` the table issued and
/// the quota it charged; this slot's rights only say the task holds it at all,
/// so the buffer plane's own checks stay the single place rights are decided.
pub(super) const RIGHT_BUFFER_ALL: u64 = u64::MAX;

/// Largest component ELF the loader will copy through [`ElfScratch`]. Generous
/// against the five components this profile declares (the largest is ~44 KiB)
/// while keeping the buffer a bounded, statically sized object like every other
/// table in this crate.
pub(super) const MAX_COMPONENT_ELF_BYTES: usize = 512 * 1024;

/// An 8-byte-aligned staging buffer for one component ELF at a time.
#[repr(align(8))]
pub(super) struct ElfScratch {
    bytes: [u8; MAX_COMPONENT_ELF_BYTES],
}

/// The staging buffer, `const`-initialized in `.bss`.
///
/// A static rather than a local: at 512 KiB it would overflow the root task's
/// 256 KiB stack, which is exactly the failure B3 recorded — a 10 KiB stack
/// temporary silently corrupting adjacent memory instead of faulting. The same
/// reasoning that made `SHARED_BUFFER_TABLE` a plain `const`-initialized static
/// applies here, and more so.
pub(super) static mut ELF_SCRATCH: ElfScratch = ElfScratch {
    bytes: [0; MAX_COMPONENT_ELF_BYTES],
};

const _: () = assert!(MAX_COMPONENT_ELF_BYTES >= 64 * 1024);

/// Generation-global native object catalogues. Objects live for the generation;
/// child-local derived capabilities are charged to and reclaimed with each task.
pub(super) static mut PEER_ENDPOINTS: peer_endpoint::PeerEndpointTable =
    peer_endpoint::PeerEndpointTable::new();
pub(super) static mut NOTIFICATIONS: notification::NotificationTable =
    notification::NotificationTable::new();
pub(super) static mut CLOCK_SERVICE: clock::ClockService = clock::ClockService::new();
pub(super) static mut WAIT_SET_SERVICE: wait_set::WaitSetService = wait_set::WaitSetService::new();
pub(super) static mut SCHEDULING_SERVICE: scheduling::SchedulingService =
    scheduling::SchedulingService::new();
pub(super) static mut LIFECYCLE_SERVICE: lifecycle::LifecycleService =
    lifecycle::LifecycleService::new();

/// The launch phase's task and transfer-window tables, and its record of which
/// declared instances were launched.
///
/// Statics rather than `launch_instance_graph` locals, for exactly the reason
/// stated above [`ELF_SCRATCH`] and recorded at length above `main`'s
/// `#[root_task]` attribute — and this is the fourth time that hazard has been
/// paid for. Together these three are ~488 KiB, which `launch_instance_graph`
/// took as a single stack frame: the disassembled prologue subtracted `0x7a000`
/// from `sp` in one step, past the guard page a 1 MiB stack leaves.
///
/// It faulted only by luck of layout. The overflow lands on `FREE_PAGE`, the
/// page [`crate::child_vspace::ScratchPage`] deliberately leaves unmapped, only
/// when preceding `.bss` places the stack low enough; with a larger `.bss`
/// above it the same overflow wrote into mapped slack and corrupted whatever
/// was there, silently and invisibly to every gate. Shrinking the allocator's
/// physical-provenance table from 2 MB to 16 KiB is what moved the stack down
/// far enough to turn that silent corruption into an honest VM fault at
/// `0x3e2ab0`.
///
/// The generation's own tables are generation-lived, like the catalogues above,
/// so nothing is lost by giving them a fixed home: one launch phase runs, once.
///
/// `MaybeUninit` rather than `TaskTable::new()` directly, for the same reason
/// and by the same pattern as `main`'s `OBJECT_ALLOCATOR`. `Option<Task>`'s
/// niche makes `None` the byte `0x2`, not zero, so a `const`-initialized
/// `[Option<Task>; 48]` is not all-zero and the linker must place it in
/// `.data` — 163 KiB of image, and therefore ~40 root CSlots, to store 48
/// non-zero tag bytes. Uninitialized storage is `.bss`, costs nothing in the
/// image, and is written exactly once by [`init_launch_tables`] before any
/// reference is handed out.
pub(super) static mut LAUNCH_TASKS: core::mem::MaybeUninit<TaskTable<MAX_TASKS>> =
    core::mem::MaybeUninit::uninit();
pub(super) static mut LAUNCH_WINDOWS: core::mem::MaybeUninit<WindowTable<MAX_WINDOW_ENTRIES>> =
    core::mem::MaybeUninit::uninit();
pub(super) static mut LAUNCH_INSTANCES: LaunchedInstances = LaunchedInstances::new();

/// Initialize the launch tables and borrow them for the rest of the phase.
///
/// Called once, from the one place that owns the launch phase. Returning the
/// references from the same call that writes them is what keeps "initialized
/// before use" a property of the code rather than a rule a caller must
/// remember.
pub(super) fn init_launch_tables() -> (
    &'static mut TaskTable<MAX_TASKS>,
    &'static mut WindowTable<MAX_WINDOW_ENTRIES>,
    &'static mut LaunchedInstances,
) {
    // SAFETY: root startup is single-threaded and `launch_instance_graph` runs
    // once, so this is the only writer and the only borrow of each static.
    unsafe {
        (&raw mut LAUNCH_TASKS).write(core::mem::MaybeUninit::new(TaskTable::new()));
        (&raw mut LAUNCH_WINDOWS).write(core::mem::MaybeUninit::new(WindowTable::new()));
        (
            (&raw mut LAUNCH_TASKS)
                .cast::<TaskTable<MAX_TASKS>>()
                .as_mut()
                .unwrap(),
            (&raw mut LAUNCH_WINDOWS)
                .cast::<WindowTable<MAX_WINDOW_ENTRIES>>()
                .as_mut()
                .unwrap(),
            (&raw mut LAUNCH_INSTANCES).as_mut().unwrap(),
        )
    }
}

pub(super) const MAX_CAPABILITY_EXPORTS: usize = 64;
#[derive(Clone, Copy)]
pub(super) struct CapabilityExport {
    pub(super) id: u32,
    pub(super) sender: TaskId,
    pub(super) receiver: TaskId,
    pub(super) capability: graph::CapabilityEntry,
    pub(super) ticket: Option<sel4::CPtrBits>,
    pub(super) sender_ticket_slot: Option<sel4::CPtrBits>,
    pub(super) retain: bool,
    pub(super) finalized: bool,
}
pub(super) struct CapabilityExports {
    pub(super) entries: [Option<CapabilityExport>; MAX_CAPABILITY_EXPORTS],
    pub(super) next_id: u32,
    pub(super) exported: usize,
    pub(super) imported: usize,
    pub(super) cancelled: usize,
    pub(super) finalized: usize,
}
impl CapabilityExports {
    const fn new() -> Self {
        Self {
            entries: [None; MAX_CAPABILITY_EXPORTS],
            next_id: 1,
            exported: 0,
            imported: 0,
            cancelled: 0,
            finalized: 0,
        }
    }
    pub(super) fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }
    pub(super) fn get_mut(&mut self, id: u32) -> Option<&mut CapabilityExport> {
        self.entries
            .iter_mut()
            .flatten()
            .find(|entry| entry.id == id)
    }
    pub(super) fn remove(&mut self, id: u32) -> Option<CapabilityExport> {
        let slot = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|entry| entry.id == id))?;
        slot.take()
    }
}
pub(super) static mut CAPABILITY_EXPORTS: CapabilityExports = CapabilityExports::new();
impl ElfScratch {
    /// Copy `elf` into the buffer and return it at a guaranteed 8-byte
    /// alignment. Returns the payload's length when it does not fit, so the
    /// caller reports the bound rather than truncating to it.
    pub(super) fn hold(&mut self, elf: &[u8]) -> Result<&[u8], usize> {
        let destination = self.bytes.get_mut(..elf.len()).ok_or(elf.len())?;
        destination.copy_from_slice(elf);
        Ok(destination)
    }
}

/// Decode the generation's shared-buffer budget resource, if it carries one.
///
/// Located by magic among the `KIND_RESOURCE` objects, exactly as
/// `kernel/src/runtime/generation.rs::budget_object` does — the generation
/// authenticates every object against its digest table, so this decodes bytes
/// whose integrity is already established and enforces only structural
/// validity.
///
/// A generation carrying no budget is not an error: it declares that no
/// component holds any shared-buffer quota, and every holder then resolves to
/// [`HolderQuota::DENY`]. Neither is a malformed one — it also yields `DENY`
/// for every holder, which fails closed. Both are the same answer the retired
/// kernel gives, and it is the conservative one: a component denied a quota it
/// was promised fails at its own probe with a bounded error, whereas a
/// permissive fallback would hand out authority no generation declared.
pub(super) fn shared_buffer_budget<'a>(
    generation: &Generation<'a>,
) -> Option<SharedBufferBudget<'a>> {
    // The *first* magic match decides, and a malformed one is `None` rather
    // than a reason to keep looking. Scanning past it would let a generation
    // carrying one bad budget and one good one resolve the good one, which is
    // not what "the generation declares a budget" means. `budget_object` in the
    // retired kernel returns `Some(Err(..))` here for the same reason.
    let object = (0..generation.object_count())
        .filter_map(|index| generation.object(index).ok())
        .filter(|object| object.kind == KIND_RESOURCE)
        .find(|object| object.bytes.len() >= 8 && object.bytes[..8] == budget_magic::MAGIC)?;
    SharedBufferBudget::decode(object.bytes).ok()
}

/// The ceiling this generation declares for the component named `component`.
///
/// [`HolderQuota::DENY`] when the generation declares no budget or the
/// component is absent from it — authority is never ambient, so a component the
/// budget does not name holds nothing rather than something small.
pub(super) fn declared_quota(
    budget: Option<&SharedBufferBudget<'_>>,
    component: &str,
) -> HolderQuota {
    let Some(budget) = budget else {
        return HolderQuota::DENY;
    };
    match budget.quota_for(&budget_magic::holder_identity(component)) {
        Some(quota) => HolderQuota {
            byte_pages: quota.byte_pages,
            buffer_count: quota.buffer_count,
            mapping_count: quota.mapping_count,
            loan_count: quota.loan_count,
        },
        None => HolderQuota::DENY,
    }
}

/// The private-memory page ceiling this generation declares for `component`
/// (C10.2).
///
/// Zero when the generation declares no budget or the component is absent from
/// it: authority is never ambient, so a component the budget does not name
/// grows nothing rather than something small. That is the same rule
/// [`declared_quota`] applies to shared buffers, and it is what makes omission
/// meaningful instead of a default.
///
/// A malformed budget cannot reach here — `Admission::admit` fails the whole
/// generation closed on one (C10.2's exit condition), which is the asymmetry
/// with the C7.3 path: a shared-buffer budget that will not decode denies every
/// holder and boots, whereas an undecodable private-memory budget is
/// indistinguishable from a quota a component was promised and never got.
pub(super) fn declared_private_memory_pages(
    budget: Option<&PrivateMemoryBudget<'_>>,
    component: &str,
) -> usize {
    let Some(budget) = budget else {
        return 0;
    };
    budget
        .quota_for(&private_memory_budget::holder_identity(component))
        .map_or(0, |quota| quota.page_quota as usize)
}
