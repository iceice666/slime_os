//! Capability objects and the per-task capability table.
//!
//! The kernel object surface is the only authority a component can exercise.
//! M5.1 extends the surface from endpoints/executables to the generic device
//! resources a trusted driver needs: PCI functions, pinned DMA memory,
//! interrupt lines, and shared buffers. Every resource is reached only
//! through a capability whose rights the kernel checks on each operation;
//! no component receives ambient device, DMA, or storage authority.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::AtomicBool;
use spin::Mutex;

use crate::ipc::Endpoint;
use crate::memory::PhysAddr;

pub const MAX_CAPS: usize = 64;

// --- Rights bits ---------------------------------------------------------
// IPC, executable, and M6.1 factory/supervision rights.
pub const RIGHT_SEND: u32 = 1;
pub const RIGHT_RECV: u32 = 2;
pub const RIGHT_TRANSFER: u32 = 4;
pub const RIGHT_EXEC: u32 = 8;
// M5.1/M5.2 device-resource rights. Each kernel gate checks its relevant bit.
pub const RIGHT_MAP_MMIO: u32 = 1 << 4;
pub const RIGHT_DMA_PIN: u32 = 1 << 5;
pub const RIGHT_DMA_RELEASE: u32 = 1 << 6;
pub const RIGHT_IRQ_ACK: u32 = 1 << 7;
pub const RIGHT_BUFFER_WRITE: u32 = 1 << 8;
pub const RIGHT_MAP: u32 = 1 << 9;
pub const RIGHT_BLOCK_READ: u32 = 1 << 10;
pub const RIGHT_BLOCK_WRITE: u32 = 1 << 11;
// M5.4 object-store rights. Each gate lives in `SYS_STORE_TRANSACT`.
pub const RIGHT_STORE_READ: u32 = 1 << 12;
pub const RIGHT_STORE_WRITE: u32 = 1 << 13;
pub const RIGHT_HEALTH_CONFIRM: u32 = 1 << 14;
pub const RIGHT_BOOT_UPDATE: u32 = 1 << 15;
pub const RIGHT_SPAWN: u32 = 1 << 16;
pub const RIGHT_ENDPOINT_CREATE: u32 = 1 << 17;
pub const RIGHT_SUPERVISE: u32 = 1 << 18;
pub const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
pub const RIGHT_DIRECTORY_WRITE: u32 = 1 << 20;
pub const RIGHT_DIRECTORY_LIST: u32 = 1 << 21;
pub const RIGHT_DIRECTORY_DERIVE: u32 = 1 << 22;
pub const RIGHT_INPUT_READ: u32 = 1 << 23;

/// All rights a capability may ever carry. Used to reject unknown bits.
pub const RIGHT_ALL: u32 = RIGHT_SEND
    | RIGHT_RECV
    | RIGHT_TRANSFER
    | RIGHT_EXEC
    | RIGHT_MAP_MMIO
    | RIGHT_DMA_PIN
    | RIGHT_DMA_RELEASE
    | RIGHT_IRQ_ACK
    | RIGHT_BUFFER_WRITE
    | RIGHT_MAP
    | RIGHT_BLOCK_READ
    | RIGHT_BLOCK_WRITE
    | RIGHT_STORE_READ
    | RIGHT_STORE_WRITE
    | RIGHT_HEALTH_CONFIRM
    | RIGHT_BOOT_UPDATE
    | RIGHT_SPAWN
    | RIGHT_ENDPOINT_CREATE
    | RIGHT_SUPERVISE
    | RIGHT_DIRECTORY_READ
    | RIGHT_DIRECTORY_WRITE
    | RIGHT_DIRECTORY_LIST
    | RIGHT_DIRECTORY_DERIVE
    | RIGHT_INPUT_READ;

#[derive(Clone)]
pub struct Capability {
    pub object: KernelObject,
    pub rights: u32,
}

impl Capability {
    /// Return a clone of this capability with rights narrowed to `mask`.
    /// Widening (any bit in `mask` not already held) is rejected.
    pub fn derive(&self, mask: u32) -> Result<Self, CapError> {
        if mask & !RIGHT_ALL != 0 {
            return Err(CapError::BadRights);
        }
        if mask & !self.rights != 0 {
            return Err(CapError::BadRights);
        }
        Ok(Self {
            object: match &self.object {
                KernelObject::Endpoint(endpoint) => KernelObject::Endpoint(endpoint.clone()),
                object => object.clone(),
            },
            rights: mask,
        })
    }
}

#[derive(Clone)]
pub enum KernelObject {
    Endpoint(Endpoint),
    EndpointFactory,
    Input,
    Executable {
        name: Option<&'static str>,
        bytes: &'static [u8],
        spawn_budget: u16,
    },
    Supervision(crate::task::TaskId),
    PciFunction(PciFunctionInfo),
    DmaMemory(DmaRegion),
    Irq(IrqLine),
    SharedBuffer(SharedRegion),
    BlockDevice(PciFunctionInfo),
    /// Authority over the GPT-validated, content-addressed object store
    /// partition (M5.4). Created by the kernel bootstrap; the store service
    /// resolves and bounds the partition through GPT validation.
    ObjectStore,
    /// Unforgeable authority over one bounded namespace scope. The userspace
    /// filesystem service owns snapshot policy; the kernel stores only the
    /// current root identity and the scope enforced at operation gates.
    Directory(DirectoryAuthority),
    /// Authority over the running generation and bounded BootState updates.
    GenerationControl,
}

impl KernelObject {
    /// Rights bits meaningful for this object kind. `RIGHT_TRANSFER` is a
    /// meta-right valid on every capability; every other bit names the
    /// operation it gates on the specific object. The authoritative
    /// object-by-rights matrix lives in `docs/capability-matrix.md`.
    pub fn valid_rights(&self) -> u32 {
        let object_rights = match self {
            KernelObject::Endpoint(_) => RIGHT_SEND | RIGHT_RECV,
            KernelObject::EndpointFactory => RIGHT_ENDPOINT_CREATE,
            KernelObject::Input => RIGHT_INPUT_READ,
            KernelObject::Executable { .. } => RIGHT_EXEC | RIGHT_SPAWN,
            KernelObject::Supervision(_) => RIGHT_SUPERVISE,
            KernelObject::PciFunction(_) => RIGHT_MAP_MMIO | RIGHT_DMA_PIN | RIGHT_DMA_RELEASE,
            KernelObject::DmaMemory(_) => RIGHT_DMA_RELEASE,
            KernelObject::Irq(_) => RIGHT_IRQ_ACK,
            KernelObject::SharedBuffer(_) => RIGHT_BUFFER_WRITE | RIGHT_MAP,
            KernelObject::BlockDevice(_) => RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE,
            KernelObject::ObjectStore => RIGHT_STORE_READ | RIGHT_STORE_WRITE,
            KernelObject::Directory(_) => {
                RIGHT_DIRECTORY_READ
                    | RIGHT_DIRECTORY_WRITE
                    | RIGHT_DIRECTORY_LIST
                    | RIGHT_DIRECTORY_DERIVE
            }
            KernelObject::GenerationControl => RIGHT_HEALTH_CONFIRM | RIGHT_BOOT_UPDATE,
        };
        object_rights | RIGHT_TRANSFER
    }
}

pub const MAX_DIRECTORY_PATH: usize = 48;
pub const MAX_DIRECTORY_DEPTH: usize = 4;

#[derive(Clone)]
pub struct DirectoryAuthority {
    namespace: Arc<DirectoryNamespace>,
    scope: [u8; MAX_DIRECTORY_PATH],
    scope_len: u8,
}

struct DirectoryNamespace {
    root: Mutex<[u8; 32]>,
}

impl DirectoryAuthority {
    pub fn root(root: [u8; 32]) -> Self {
        Self {
            namespace: Arc::new(DirectoryNamespace {
                root: Mutex::new(root),
            }),
            scope: [0; MAX_DIRECTORY_PATH],
            scope_len: 0,
        }
    }

    pub fn derive(&self, relative: &[u8]) -> Result<Self, CapError> {
        if !valid_directory_path(relative, true) {
            return Err(CapError::BadScope);
        }
        let scope_len = self.scope_len as usize;
        let separator = usize::from(scope_len != 0 && !relative.is_empty());
        let new_len = scope_len
            .checked_add(separator)
            .and_then(|len| len.checked_add(relative.len()))
            .filter(|len| *len <= MAX_DIRECTORY_PATH)
            .ok_or(CapError::BadScope)?;
        let mut scope = [0; MAX_DIRECTORY_PATH];
        scope[..scope_len].copy_from_slice(&self.scope[..scope_len]);
        if separator != 0 {
            scope[scope_len] = b'/';
        }
        scope[scope_len + separator..new_len].copy_from_slice(relative);
        if !valid_directory_path(&scope[..new_len], true) {
            return Err(CapError::BadScope);
        }
        Ok(Self {
            namespace: self.namespace.clone(),
            scope,
            scope_len: new_len as u8,
        })
    }

    pub fn scope(&self) -> &[u8] {
        &self.scope[..self.scope_len as usize]
    }

    pub fn root_identity(&self) -> [u8; 32] {
        *self.namespace.root.lock()
    }

    /// Atomically replace the namespace root. Scoped authorities are read-only
    /// views for root-transition purposes; mutating a subtree requires rebuilding
    /// and committing its parent chain through an unscoped writer.
    pub fn commit_root(&self, expected: [u8; 32], new: [u8; 32]) -> bool {
        if self.scope_len != 0 {
            return false;
        }
        let mut root = self.namespace.root.lock();
        if *root != expected {
            return false;
        }
        *root = new;
        true
    }
}

pub fn valid_directory_path(path: &[u8], allow_empty: bool) -> bool {
    if path.is_empty() {
        return allow_empty;
    }
    if path.len() > MAX_DIRECTORY_PATH || path[0] == b'/' || path[path.len() - 1] == b'/' {
        return false;
    }
    let mut depth = 0;
    for segment in path.split(|byte| *byte == b'/') {
        if segment.is_empty()
            || segment == b"."
            || segment == b".."
            || !segment
                .iter()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
        {
            return false;
        }
        depth += 1;
        if depth > MAX_DIRECTORY_DEPTH {
            return false;
        }
    }
    true
}

/// A bounded PCI segment/bus/device/function resource.
///
/// Created by the kernel PCI enumerator and granted to a driver component.
/// Carrying this capability is the only way a component may map the
/// function's MMIO BARs or pin DMA pages for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PciFunctionInfo {
    pub segment: u16,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    /// Base class, subclass, and programming interface as a packed u32:
    /// bits 0..7 programming interface, 8..15 subclass, 16..23 base class.
    pub class_code: u32,
}

/// A pinned, contiguous physical region owned by a driver for one device
/// operation. Cloning (e.g. across IPC transfer) shares the outstanding
/// flag so the kernel can refuse reclamation while a request is in flight.
#[derive(Debug)]
pub struct DmaRegion {
    inner: Arc<DmaRegionInner>,
}

#[derive(Debug)]
pub struct DmaRegionInner {
    pub phys: PhysAddr,
    pub pages: usize,
    pub outstanding: AtomicBool,
}

impl DmaRegion {
    pub fn new(phys: PhysAddr, pages: usize) -> Self {
        Self {
            inner: Arc::new(DmaRegionInner {
                phys,
                pages,
                outstanding: AtomicBool::new(false),
            }),
        }
    }
    pub fn phys(&self) -> PhysAddr {
        self.inner.phys
    }
    pub fn pages(&self) -> usize {
        self.inner.pages
    }
    /// `true` when a device request referencing this buffer is in flight.
    pub fn outstanding(&self) -> bool {
        self.inner
            .outstanding
            .load(core::sync::atomic::Ordering::Acquire)
    }
    pub fn set_outstanding(&self, value: bool) {
        self.inner
            .outstanding
            .store(value, core::sync::atomic::Ordering::Release);
    }
    /// Compare by backing allocation identity (shared `Arc`).
    pub fn ptr_eq(&self, other: &DmaRegion) -> bool {
        core::ptr::eq(
            &*self.inner as *const _ as *const u8,
            &*other.inner as *const _ as *const u8,
        )
    }
}

impl Clone for DmaRegion {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

/// A vectored interrupt line (GSI + assigned vector). Acknowledgement is
/// gated by [`RIGHT_IRQ_ACK`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IrqLine {
    pub gsi: u32,
    pub vector: u8,
}

/// A shared-memory region grantable between components. `writable` records
/// whether the holder may write; [`RIGHT_BUFFER_WRITE`] gates the syscall.
#[derive(Debug)]
pub struct SharedRegion {
    inner: Arc<SharedRegionInner>,
}

#[derive(Debug)]
pub struct SharedRegionInner {
    pub phys: PhysAddr,
    pub pages: usize,
    pub writable: bool,
}

impl SharedRegion {
    pub fn new(phys: PhysAddr, pages: usize, writable: bool) -> Self {
        Self {
            inner: Arc::new(SharedRegionInner {
                phys,
                pages,
                writable,
            }),
        }
    }
    pub fn phys(&self) -> PhysAddr {
        self.inner.phys
    }
    pub fn pages(&self) -> usize {
        self.inner.pages
    }
    pub fn writable(&self) -> bool {
        self.inner.writable
    }
}

impl Clone for SharedRegion {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

pub struct CapabilityTable {
    slots: [Option<Capability>; MAX_CAPS],
}

impl CapabilityTable {
    pub fn new() -> Self {
        Self {
            slots: core::array::from_fn(|_| None),
        }
    }

    pub fn insert(&mut self, cap: Capability) -> Result<u32, CapError> {
        if cap.rights & !cap.object.valid_rights() != 0 {
            return Err(CapError::BadRights);
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if slot.is_none() {
                *slot = Some(cap);
                return Ok(i as u32);
            }
        }
        Err(CapError::TableFull)
    }

    pub fn get(&self, slot: u32) -> Option<&Capability> {
        self.slots.get(slot as usize)?.as_ref()
    }

    pub fn take(&mut self, slot: u32) -> Option<Capability> {
        self.slots.get_mut(slot as usize)?.take()
    }

    pub fn remove(&mut self, slot: u32) -> Result<Capability, CapError> {
        self.take(slot).ok_or(CapError::BadSlot)
    }
    pub fn put(&mut self, slot: u32, cap: Capability) -> Result<(), CapError> {
        let Some(dst) = self.slots.get_mut(slot as usize) else {
            return Err(CapError::BadSlot);
        };
        if dst.is_some() {
            return Err(CapError::BadSlot);
        }
        *dst = Some(cap);
        Ok(())
    }

    pub fn available_slots(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_none()).count()
    }

    pub fn drain(&mut self) -> Vec<Capability> {
        let mut caps = Vec::new();
        for slot in self.slots.iter_mut() {
            if let Some(cap) = slot.take() {
                caps.push(cap);
            }
        }
        caps
    }
}

impl Default for CapabilityTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn directory_derive_shares_root_and_narrows_scope() {
        let root = [0x11; 32];
        let authority = DirectoryAuthority::root(root);
        let docs = authority.derive(b"docs").expect("derive docs");
        let nested = docs.derive(b"manual").expect("derive nested");

        assert_eq!(docs.scope(), b"docs");
        assert_eq!(nested.scope(), b"docs/manual");
        assert_eq!(nested.root_identity(), root);
        let replacement = [0x22; 32];
        assert!(authority.commit_root(root, replacement));
        assert_eq!(nested.root_identity(), replacement);
        assert!(!docs.commit_root(replacement, [0x33; 32]));
        assert_eq!(authority.root_identity(), replacement);
    }

    #[test_case]
    fn directory_paths_are_bounded_and_canonical() {
        for path in [b"/docs".as_slice(), b"docs/", b"docs//manual", b".", b".."] {
            assert!(matches!(
                DirectoryAuthority::root([0; 32]).derive(path),
                Err(CapError::BadScope)
            ));
        }
        assert!(matches!(
            DirectoryAuthority::root([0; 32]).derive(b"a/b/c/d/e"),
            Err(CapError::BadScope)
        ));
    }

    #[test_case]
    fn directory_rights_are_object_specific_and_narrow_only() {
        let cap = Capability {
            object: KernelObject::Directory(DirectoryAuthority::root([0; 32])),
            rights: RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_DERIVE | RIGHT_TRANSFER,
        };
        let narrowed = cap
            .derive(RIGHT_DIRECTORY_READ)
            .expect("narrow read capability");
        assert_eq!(narrowed.rights, RIGHT_DIRECTORY_READ);
        assert!(cap.derive(RIGHT_DIRECTORY_WRITE).is_err());
        let mut table = CapabilityTable::new();
        assert!(table.insert(cap).is_ok());
        assert!(
            table
                .insert(Capability {
                    object: KernelObject::Directory(DirectoryAuthority::root([0; 32])),
                    rights: RIGHT_STORE_READ,
                })
                .is_err()
        );
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapError {
    TableFull,
    BadSlot,
    /// Unknown rights bits, or a derive/transfer that would widen rights.
    BadRights,
    /// The capability is the wrong object kind for the requested operation.
    WrongObject,
    /// A requested directory scope is malformed or not a subdirectory.
    BadScope,
}
