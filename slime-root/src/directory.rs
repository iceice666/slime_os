//! Directory namespace state and interned scopes (B45).
//!
//! Namespace roots and scope interning are owned here. Callers resolve a
//! task's typed directory authority through [`crate::task::TaskTable`]; this
//! module keeps no second authority database.

use crate::child_vspace::ScratchPage;
use crate::graph::CapabilityEntry;
use crate::ipc::{IpcError, Response};
use crate::task::{TaskId, TaskTable};
use crate::transfer_window;
use boot_contracts::generation::RIGHT_TRANSFER;

/// Authority over a `Directory` (M6.3, P5.4.3), numbered as the oracle's
/// `capability::RIGHT_DIRECTORY_*` and as `directoryRead` and friends in the
/// generation's rights table.
///
/// Four independent bits rather than a read/write pair: listing a directory and
/// resolving one name in it are different authorities, and derivation is a
/// third — a component may be allowed to *use* a scope without being allowed to
/// hand out narrower views of it.
pub const RIGHT_DIRECTORY_READ: u64 = 1 << 19;
pub const RIGHT_DIRECTORY_WRITE: u64 = 1 << 20;
pub const RIGHT_DIRECTORY_LIST: u64 = 1 << 21;
pub const RIGHT_DIRECTORY_DERIVE: u64 = 1 << 22;

/// Every right a directory capability may carry, for bounding a derive request.
pub const RIGHTS_DIRECTORY_ALL: u64 =
    RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE | RIGHT_DIRECTORY_LIST | RIGHT_DIRECTORY_DERIVE;

/// A handle naming an interned relative directory path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopeId(u16);

pub const MAX_DIRECTORY_PATH: usize = 128;
pub const MAX_DIRECTORY_DEPTH: usize = 8;
pub const MAX_SCOPES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DirectoryScope {
    bytes: [u8; MAX_DIRECTORY_PATH],
    len: u8,
}

impl DirectoryScope {
    const fn root() -> Self {
        Self {
            bytes: [0; MAX_DIRECTORY_PATH],
            len: 0,
        }
    }
    fn path(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    fn derive(&self, relative: &[u8]) -> Option<Self> {
        if !valid_directory_path(relative, true) {
            return None;
        }
        let current = self.len as usize;
        let separator = usize::from(current != 0 && !relative.is_empty());
        let total = current
            .checked_add(separator)?
            .checked_add(relative.len())?;
        if total > MAX_DIRECTORY_PATH {
            return None;
        }
        let mut bytes = [0; MAX_DIRECTORY_PATH];
        bytes[..current].copy_from_slice(&self.bytes[..current]);
        if separator != 0 {
            bytes[current] = b'/';
        }
        bytes[current + separator..total].copy_from_slice(relative);
        if !valid_directory_path(&bytes[..total], true) {
            return None;
        }
        Some(Self {
            bytes,
            len: total as u8,
        })
    }
}

/// Append-only scope interning owned by the directory service.
pub struct ScopeTable {
    paths: [DirectoryScope; MAX_SCOPES],
    len: usize,
}

impl ScopeTable {
    pub const ROOT: ScopeId = ScopeId(0);
    pub const fn new() -> Self {
        Self {
            paths: [DirectoryScope::root(); MAX_SCOPES],
            len: 1,
        }
    }
    pub fn path(&self, id: ScopeId) -> &[u8] {
        self.paths
            .get(id.0 as usize)
            .map_or(&[], DirectoryScope::path)
    }
    pub fn is_root(&self, id: ScopeId) -> bool {
        id == Self::ROOT || self.path(id).is_empty()
    }
    pub fn derive(&mut self, base: ScopeId, relative: &[u8]) -> Option<ScopeId> {
        let derived = self.paths.get(base.0 as usize)?.derive(relative)?;
        if let Some(index) = self.paths[..self.len]
            .iter()
            .position(|scope| *scope == derived)
        {
            return Some(ScopeId(index as u16));
        }
        if self.len >= MAX_SCOPES {
            return None;
        }
        let index = self.len;
        self.paths[index] = derived;
        self.len += 1;
        Some(ScopeId(index as u16))
    }
}

impl Default for ScopeTable {
    fn default() -> Self {
        Self::new()
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

/// Answer one `BlockTransact` (P5.4.2c).
///
/// Three checks before a sector moves, in the order the oracle's
/// `sys_block_transact` makes them:
///
/// 1. the caller's slot must resolve to a `Block` capability — holding a slot
///    number is not authority;
/// 2. the request must decode as a `WireBlockRequest` with the right magic and
///    version, so a malformed frame is refused rather than interpreted;
/// 3. the operation must be covered by the capability's own rights —
///    `blockRead` for `OP_READ`, `blockWrite` for `OP_WRITE` and `OP_FLUSH`.
///
/// The payload travels through the caller's transfer window, like every other
/// windowed operation. One sector per request: `sector_count` above one is
/// refused rather than partially served, because a partial completion has no
/// The namespace root the boot starts from (M6.3, P5.4.3).
///
/// The identity of the directory snapshot `scripts/build/build-directory-fixture.py`
/// commits to the object store, hardcoded exactly as the oracle's
/// `bootstrap::directory_fixture_root` hardcodes it. The root task cannot
/// compute it: resolving a snapshot means reading the store, which is
/// userspace's. What it can do is start the namespace at a root a component
/// will recognise.
///
/// Zero on every plane whose fixture has no directory tree, which is every
/// plane but the filesystem one — and a zero root is what "nothing committed
/// yet" means, so the directory plane's empty-namespace arm still holds.
pub const DIRECTORY_FIXTURE_ROOT: [u8; 32] = [
    0xe8, 0xcd, 0xd1, 0x45, 0x6f, 0xe5, 0x4e, 0x59, 0xe3, 0xb6, 0x1a, 0x65, 0x5a, 0x2f, 0xbb, 0xfa,
    0xf1, 0x6d, 0x89, 0xa8, 0x77, 0x0a, 0xa1, 0x08, 0x05, 0x51, 0xbd, 0x84, 0xf6, 0x6b, 0x0f, 0xf2,
];

pub struct Namespaces {
    roots: [[u8; 32]; MAX_NAMESPACES],
}

/// Namespaces this cutover supports. One, and the resource carries an index so
/// raising it later is a table change rather than a representation change.
const MAX_NAMESPACES: usize = 1;

impl Default for Namespaces {
    fn default() -> Self {
        Self::new()
    }
}

impl Namespaces {
    pub const fn new() -> Self {
        Self {
            roots: [DIRECTORY_FIXTURE_ROOT; MAX_NAMESPACES],
        }
    }

    fn root(&self, namespace: u32) -> Option<[u8; 32]> {
        self.roots.get(namespace as usize).copied()
    }

    /// Replace a namespace root, but only if it still holds `expected`.
    ///
    /// The compare is the point. A writer builds a new tree from the root it
    /// read; if another writer committed in between, that tree is built on a
    /// stale parent and installing it would silently discard the other's work.
    /// A failed compare is `false`, not an error: the caller re-reads and
    /// retries, which is the ordinary path rather than a fault.
    fn commit(&mut self, namespace: u32, expected: [u8; 32], new: [u8; 32]) -> Option<bool> {
        let slot = self.roots.get_mut(namespace as usize)?;
        if *slot != expected {
            return Some(false);
        }
        *slot = new;
        Some(true)
    }
}

/// Answer `DirectoryInspect`: the namespace root this capability sees, and the
/// scope it sees it through.
///
/// `words[0]` is the capability slot and `words[1]` the rights the caller
/// claims to need — checked as a subset of what the capability carries, so a
/// component asking for `directoryWrite` on a read-only view is refused here
/// rather than discovering it at commit time.
///
/// The reply is the 32-byte root followed by the scope path, written through
/// the caller's transfer window because a scope can exceed a message.
pub fn serve_directory_inspect<const TASKS: usize>(
    tasks: &TaskTable<TASKS>,
    namespaces: &Namespaces,
    scopes: &ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
    buffer: &mut sel4::IpcBuffer,
) -> Response {
    let Some(table) = tasks.authority(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let slot = words[0] as u32;
    let required = words[0] >> 32;
    if required == 0 || required & !RIGHTS_DIRECTORY_ALL != 0 {
        return Response::error(IpcError::InvalidOperation);
    }
    let Ok(capability) = table.resolve_directory(slot, required) else {
        return Response::error(IpcError::BadCapability);
    };
    let namespace = capability.namespace;
    let scope = capability.scope;
    let Some(root) = namespaces.root(namespace) else {
        return Response::error(IpcError::BadCapability);
    };
    let path = scopes.path(scope);
    let mut reply = [0u8; 32 + MAX_DIRECTORY_PATH];
    reply[..32].copy_from_slice(&root);
    reply[32..32 + path.len()].copy_from_slice(path);
    let descriptor = match transfer_window::write_staged_region_with(
        window,
        &reply[..32 + path.len()],
        scratch,
        buffer,
    ) {
        Ok(descriptor) => descriptor,
        Err(error) => return Response::error(error),
    };
    sel4::debug_println!(
        "SLIME_GRAPH directory inspected task={} slot={slot} namespace={namespace} scope={}",
        id.0,
        DisplayPath(path),
    );
    Response::success(path.len() as i64, descriptor)
}

/// Answer `DirectoryDerive`: a narrower view of the same namespace.
///
/// Two narrowings at once, and both are one-directional:
///
/// * the **scope** may only lengthen — the request's path is appended to the
///   source's, so a holder of `docs` derives `docs/notes` and can express no
///   path that escapes it. There is no syntax for `..`, because
///   `valid_directory_path` rejects the segment outright;
/// * the **rights** must be a subset of the source's, and `RIGHT_TRANSFER` is
///   checked separately so a view that may not be handed on cannot derive one
///   that may.
///
/// Non-consuming: the source capability stays exactly as it was, matching every
/// other derive-copy in this crate since B25.
pub fn serve_directory_derive<const TASKS: usize>(
    tasks: &mut TaskTable<TASKS>,
    scopes: &mut ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
) -> Response {
    // Same packing as inspect. The path's length is not a word: it comes from
    // the staged descriptor, so a caller cannot claim one length and stage
    // another.
    let slot = words[0] as u32;
    let rights = words[0] >> 32;
    if rights == 0 || rights & !(RIGHTS_DIRECTORY_ALL | RIGHT_TRANSFER) != 0 {
        return Response::error(IpcError::InvalidOperation);
    }
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    let frame = match transfer_window::read_staged_array(window, transfer, words, scratch) {
        Ok(frame) => frame,
        Err(error) => return Response::error(error),
    };
    let staged = frame.bytes();
    if staged.len() > MAX_DIRECTORY_PATH {
        return Response::error(IpcError::InvalidLength);
    }
    let path_len = staged.len();
    let mut path = [0u8; MAX_DIRECTORY_PATH];
    path[..path_len].copy_from_slice(staged);
    let Some(table) = tasks.authority_mut(id) else {
        return Response::error(IpcError::BadCapability);
    };
    // Resolved on `RIGHT_DIRECTORY_DERIVE` alone: holding a view is not
    // authority to hand out narrower ones.
    let Ok(source) = table.resolve_directory(slot, RIGHT_DIRECTORY_DERIVE) else {
        return Response::error(IpcError::BadCapability);
    };
    if rights & !source.rights.bits() != 0 {
        return Response::error(IpcError::BadCapability);
    }
    let Some(derived) = scopes.derive(source.scope, &path[..path_len]) else {
        return Response::error(IpcError::InvalidOperation);
    };
    let Some(capability) = CapabilityEntry::directory(source.namespace, derived, rights) else {
        return Response::error(IpcError::BadCapability);
    };
    let Some(free) = table.free_slot_from(1) else {
        return Response::error(IpcError::DestinationSlotsExhausted);
    };
    if table.install(free, capability).is_err() {
        return Response::error(IpcError::DestinationSlotsExhausted);
    }
    sel4::debug_println!(
        "SLIME_GRAPH directory derived task={} from={slot} to={free} namespace={} scope={} rights={rights:#x}",
        id.0,
        source.namespace,
        DisplayPath(scopes.path(derived)),
    );
    Response::success(free as i64, 0)
}

/// Answer `DirectoryCommit`: replace the namespace root, atomically.
///
/// Two gates the oracle also applies, and each rules out a different attack:
///
/// * `RIGHT_DIRECTORY_WRITE`, so a reader cannot install anything;
/// * an **unscoped** capability, so a holder of `docs` cannot replace the
///   namespace-wide root with its own subtree — which would promote a subtree
///   snapshot to the whole filesystem and delete everything beside it.
///
/// The staged payload is two 32-byte identities: the root the caller believes
/// is live, and the one it built. A mismatch answers `WouldBlock`, which is the
/// retry signal rather than a failure.
pub fn serve_directory_commit<const TASKS: usize>(
    tasks: &TaskTable<TASKS>,
    namespaces: &mut Namespaces,
    scopes: &ScopeTable,
    window: Option<transfer_window::Window>,
    scratch: &ScratchPage,
    id: TaskId,
    words: &[sel4::Word],
    buffer: &mut sel4::IpcBuffer,
) -> Response {
    let slot = words[0] as u32;
    let Some(transfer) = words.get(1).copied() else {
        return Response::error(IpcError::InvalidLength);
    };
    let frame =
        match transfer_window::read_staged_array_with(window, transfer, words, scratch, buffer) {
            Ok(frame) => frame,
            Err(error) => return Response::error(error),
        };
    let staged = frame.bytes();
    if staged.len() != 64 {
        return Response::error(IpcError::InvalidLength);
    }
    let mut expected = [0u8; 32];
    let mut new = [0u8; 32];
    expected.copy_from_slice(&staged[..32]);
    new.copy_from_slice(&staged[32..64]);
    let Some(table) = tasks.authority(id) else {
        return Response::error(IpcError::BadCapability);
    };
    let Ok(capability) = table.resolve_directory(slot, RIGHT_DIRECTORY_WRITE) else {
        return Response::error(IpcError::BadCapability);
    };
    let namespace = capability.namespace;
    let scope = capability.scope;
    if !scopes.is_root(scope) {
        sel4::debug_println!(
            "SLIME_GRAPH directory commit refused task={} slot={slot} namespace={namespace} reason=scoped scope={}",
            id.0,
            DisplayPath(scopes.path(scope)),
        );
        return Response::error(IpcError::BadCapability);
    }
    match namespaces.commit(namespace, expected, new) {
        Some(true) => {
            sel4::debug_println!(
                "SLIME_GRAPH directory committed task={} namespace={namespace} root={:02x}{:02x}{:02x}{:02x}",
                id.0,
                new[0],
                new[1],
                new[2],
                new[3],
            );
            Response::success(0, 0)
        }
        // The root moved under the caller. Not an error: re-read and retry.
        Some(false) => {
            sel4::debug_println!(
                "SLIME_GRAPH directory commit stale task={} namespace={namespace}",
                id.0,
            );
            Response::error(IpcError::WouldBlock)
        }
        None => Response::error(IpcError::BadCapability),
    }
}

/// A scope path in a marker, printed as text when it is text and as `-` when it
/// is empty, so an unscoped view is visibly distinct from a missing field.
struct DisplayPath<'a>(&'a [u8]);

impl core::fmt::Display for DisplayPath<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("-");
        }
        for byte in self.0 {
            formatter.write_str(
                core::str::from_utf8(core::slice::from_ref(byte)).map_err(|_| core::fmt::Error)?,
            )?;
        }
        Ok(())
    }
}
