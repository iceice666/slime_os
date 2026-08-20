#![no_std]
#![no_main]

//! The seL4 filesystem service: M6.3's other half (P5.4.3).
//!
//! The directory *mechanism* is the root's — a shared namespace root, scoped
//! views, an atomic commit. This is what sits on top: a service that resolves
//! names inside a snapshot tree, reads and writes objects, and derives
//! subdirectory capabilities on request.
//!
//! Derived from the oracle's `filesystem-service.rs`, and deliberately so. That
//! component is *policy* — snapshot layout, path resolution, entry bounds, root
//! transitions — and policy ports. What differs is one thing: the oracle asks
//! the kernel to move object bytes through `store_transact` and an ambient
//! `buffer_addr` pointer; here the same objects come out of
//! `boot_contracts::object_store`, driven over a granted block capability, with
//! payloads crossing in this component's own memory.
//!
//! Everything else — every bound, every refusal, the whole request surface —
//! is the oracle's, because the contract is `contracts/fs/v1` on both.

extern crate alloc;

use boot_contracts::gpt::{self, GptError};
use boot_contracts::object_store::{BlockIo, IoError, ObjectStore};
use slime_proto::block::{self, WireBlockReply, WireBlockRequest};
use slime_proto::{
    capability_transfer::OBJECT_KIND_DIRECTORY,
    fs::{
        self, OFF_SNAPSHOT_COUNT, OFF_SNAPSHOT_ENTRY_HASH, OFF_SNAPSHOT_ENTRY_KIND,
        OFF_SNAPSHOT_ENTRY_NAME, OFF_SNAPSHOT_ENTRY_NAME_LEN, OFF_SNAPSHOT_ENTRY_OBJECT_TYPE,
        OFF_SNAPSHOT_ENTRY_PAYLOAD_LEN, OFF_SNAPSHOT_ENTRY_RESERVED1, OFF_SNAPSHOT_VERSION,
        SNAPSHOT_BYTES, SNAPSHOT_ENTRY_BYTES, SNAPSHOT_HEADER, SNAPSHOT_MAGIC,
        SNAPSHOT_OBJECT_TYPE, SNAPSHOT_VERSION, WireFsReply, WireFsRequest,
    },
    valid_fs_request,
};
use slime_rt::{
    CapabilityDisposition, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG,
};

// B59: rights bit numbering is generated from
// `contracts/generation/v5/schema.zt`. The fs protocol carries a 32-bit rights
// field, so the generated `u64` constants are narrowed here rather than
// re-spelled as separate `u32` literals.
const RIGHT_TRANSFER: u32 = boot_contracts::generation::RIGHT_TRANSFER as u32;
const RIGHT_DIRECTORY_READ: u32 = boot_contracts::generation::RIGHT_DIRECTORY_READ as u32;
const RIGHT_DIRECTORY_WRITE: u32 = boot_contracts::generation::RIGHT_DIRECTORY_WRITE as u32;
const RIGHT_DIRECTORY_LIST: u32 = boot_contracts::generation::RIGHT_DIRECTORY_LIST as u32;
const RIGHT_DIRECTORY_DERIVE: u32 = boot_contracts::generation::RIGHT_DIRECTORY_DERIVE as u32;

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
/// The block device, granted to this component by the generation.
///
/// Slot 2, not 1: the root installs a child's declared authority in a fixed
/// order — input, directory, factories, block — and this component is declared
/// both a directory view and the device, so the view takes 1 and the device
/// follows. The oracle holds a store *endpoint* at its equivalent slot; the
/// store itself lives in this component.
const BLOCK_SLOT: u32 = 2;
/// The declared edge back to init, on which this service announces that its
/// store is open. Init waits on it before spawning the client: opening the
/// store is hundreds of block round trips, and a client that sent its first
/// request into that window got no reply and failed its own arm.
///
/// A native Endpoint reports no peer death, so a readiness announcement is a
/// message rather than something a peer can infer.
const READY_SLOT: u32 = 3;
/// A native Endpoint reports no peer death — `ERR_PEER_DEAD` is a
/// logical-channel answer the cutover deleted — so the loop below cannot learn
/// its client is gone from the endpoint. Init observes the client's exit through
/// a supervision handle and closes this service on the same edge it announced
/// readiness on, which is the only party that can: init spawned the client.
const CLOSE: &[u8] = b"SLIME.FILESYSTEM.CLOSE";
const SECTOR_BYTES: usize = 512;
const MAX_OBJECT_PAYLOAD: u32 = 32 * 1024;
const ZERO_HASH: [u8; 32] = [0; 32];

#[derive(Clone, Copy)]
struct Entry {
    kind: u8,
    name_len: u8,
    name: [u8; fs::MAX_NAME_BYTES],
    object_type: u32,
    payload_len: u32,
    hash: [u8; 32],
}

impl Entry {
    const EMPTY: Self = Self {
        kind: 0,
        name_len: 0,
        name: [0; fs::MAX_NAME_BYTES],
        object_type: 0,
        payload_len: 0,
        hash: ZERO_HASH,
    };
}

fn main(_startup_arg: u32) {
    if open_store().is_err() {
        slime_rt::debug_write(b"[filesystem] fail: store open\n");
        slime_rt::exit(1);
    }
    if slime_rt::send(READY_SLOT, b"ready", &[]) != slime_rt::ERR_SUCCESS {
        slime_rt::debug_write(b"[filesystem] fail: ready announce\n");
        slime_rt::exit(1);
    }
    slime_rt::debug_write(b"[filesystem] ready\n");
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(READY_SLOT, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => {}
            n if n < 0 => slime_rt::exit(1),
            n if message[..n as usize] == *CLOSE => slime_rt::exit(0),
            _ => slime_rt::exit(1),
        }
        match slime_rt::recv(RPC_SLOT, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => slime_rt::exit(1),
            n => {
                // A directory capability has no kernel object to travel in the
                // message, so its export arrives alone and is claimed here
                // rather than read out of the received-capability array. That
                // array carries only native Endpoint handles now (B46).
                let claimed = slime_rt::capability_import().ok();
                let (reply, received_directory, derived_cap) =
                    handle(&message[..n as usize], claimed);
                send_reply(reply, derived_cap);
                drop_capability(received_directory);
            }
        }
    }
}

fn handle(message: &[u8], claimed: Option<u32>) -> (WireFsReply, Option<u32>, Option<u32>) {
    let Some(directory_slot) = claimed else {
        return (reply(-2, 0, 0, 0, ZERO_HASH), None, None);
    };
    let Some(request) = WireFsRequest::decode(message) else {
        return (reply(-1, 0, 0, 0, ZERO_HASH), Some(directory_slot), None);
    };
    if !valid_fs_request(&request) || !operation_fields_valid(&request) {
        return (reply(-1, 0, 0, 0, ZERO_HASH), Some(directory_slot), None);
    }
    let required_rights = match request.op {
        fs::OP_LIST => RIGHT_DIRECTORY_LIST,
        fs::OP_READ => RIGHT_DIRECTORY_READ,
        fs::OP_WRITE => RIGHT_DIRECTORY_WRITE,
        fs::OP_DERIVE => RIGHT_DIRECTORY_DERIVE,
        _ => return (reply(-1, 0, 0, 0, ZERO_HASH), Some(directory_slot), None),
    };
    let mut root = ZERO_HASH;
    let mut scope = [0u8; MAX_DIRECTORY_PATH];
    let Ok(scope_len) =
        slime_rt::directory_inspect(directory_slot, required_rights, &mut root, &mut scope)
    else {
        return (reply(-2, 0, 0, 0, ZERO_HASH), Some(directory_slot), None);
    };
    if request.op == fs::OP_WRITE && scope_len != 0 {
        return (reply(-2, 0, 0, 0, ZERO_HASH), Some(directory_slot), None);
    }
    let mut root_entries = [Entry::EMPTY; fs::MAX_ENTRIES];
    let entry_count = match load_snapshot(root, &mut root_entries) {
        Ok(count) => count,
        Err(status) => {
            return (
                reply(status, 0, 0, 0, ZERO_HASH),
                Some(directory_slot),
                None,
            );
        }
    };
    let mut entries = root_entries;
    let scoped = match resolve_scope(
        &root_entries,
        entry_count,
        &scope[..scope_len],
        &mut entries,
    ) {
        Ok(count) => count,
        Err(status) => {
            return (
                reply(status, 0, 0, 0, ZERO_HASH),
                Some(directory_slot),
                None,
            );
        }
    };
    let (reply, derived_cap) = dispatch(request, directory_slot, &mut entries, scoped, root);
    (reply, Some(directory_slot), derived_cap)
}

fn operation_fields_valid(request: &WireFsRequest) -> bool {
    match request.op {
        fs::OP_LIST | fs::OP_READ | fs::OP_DERIVE => {
            request.payload_len == 0 && request_hash(request) == ZERO_HASH
        }
        fs::OP_WRITE => request.payload_len <= MAX_OBJECT_PAYLOAD,
        _ => false,
    }
}

fn dispatch(
    request: WireFsRequest,
    directory_slot: u32,
    entries: &mut [Entry; fs::MAX_ENTRIES],
    entry_count: usize,
    root: [u8; 32],
) -> (WireFsReply, Option<u32>) {
    let name = &request.name[..request.name_len as usize];
    match request.op {
        fs::OP_LIST => (reply(0, entry_count as u32, 0, 0, ZERO_HASH), None),
        fs::OP_READ => (
            match find_entry(entries, entry_count, name) {
                Some(entry) if entry.kind == 1 => {
                    reply(0, 0, entry.object_type, entry.payload_len, entry.hash)
                }
                _ => reply(-3, 0, 0, 0, ZERO_HASH),
            },
            None,
        ),
        fs::OP_DERIVE => {
            if !matches!(find_entry(entries, entry_count, name), Some(entry) if entry.kind == 2) {
                return (reply(-3, 0, 0, 0, ZERO_HASH), None);
            }
            match slime_rt::directory_derive(
                directory_slot,
                name,
                RIGHT_DIRECTORY_READ
                    | RIGHT_DIRECTORY_LIST
                    | RIGHT_DIRECTORY_DERIVE
                    | RIGHT_TRANSFER,
            ) {
                Ok(slot) => (reply(0, 0, 0, 0, ZERO_HASH), Some(slot)),
                Err(_) => (reply(-2, 0, 0, 0, ZERO_HASH), None),
            }
        }
        fs::OP_WRITE => (
            write_entry(request, directory_slot, entries, entry_count, root),
            None,
        ),
        _ => (reply(-1, 0, 0, 0, ZERO_HASH), None),
    }
}

fn write_entry(
    request: WireFsRequest,
    directory_slot: u32,
    entries: &mut [Entry; fs::MAX_ENTRIES],
    entry_count: usize,
    root: [u8; 32],
) -> WireFsReply {
    let name = &request.name[..request.name_len as usize];
    let hash = request_hash(&request);
    let Ok((object_type, payload_len)) = store_stat(hash) else {
        return reply(-3, 0, 0, 0, ZERO_HASH);
    };
    if payload_len != request.payload_len {
        return reply(-1, 0, 0, 0, ZERO_HASH);
    }
    let index = match find_index(entries, entry_count, name) {
        Some(index) if entries[index].kind == 1 => index,
        Some(_) => return reply(-1, 0, 0, 0, ZERO_HASH),
        None if entry_count < fs::MAX_ENTRIES => entry_count,
        None => return reply(-4, 0, 0, 0, ZERO_HASH),
    };
    entries[index] = Entry {
        kind: 1,
        name_len: name.len() as u8,
        name: request.name,
        object_type,
        payload_len,
        hash,
    };
    let new_count = entry_count.max(index + 1);
    sort_entries(entries, new_count);
    let snapshot = encode_snapshot(entries, new_count);
    let Ok(new_root) = store_put(SNAPSHOT_OBJECT_TYPE, &snapshot) else {
        return reply(-5, 0, 0, 0, ZERO_HASH);
    };
    match slime_rt::directory_commit(directory_slot, &root, &new_root) {
        0 => reply(0, new_count as u32, object_type, payload_len, hash),
        ERR_WOULDBLOCK => reply(-6, 0, 0, 0, ZERO_HASH),
        _ => reply(-2, 0, 0, 0, ZERO_HASH),
    }
}

fn resolve_scope(
    root_entries: &[Entry; fs::MAX_ENTRIES],
    root_count: usize,
    scope: &[u8],
    out: &mut [Entry; fs::MAX_ENTRIES],
) -> Result<usize, i32> {
    if scope.is_empty() {
        return Ok(root_count);
    }
    let mut current = *root_entries;
    let mut count = root_count;
    for segment in scope.split(|byte| *byte == b'/') {
        let entry = find_entry(&current, count, segment).ok_or(-3)?;
        if entry.kind != 2 {
            return Err(-3);
        }
        count = load_snapshot(entry.hash, out)?;
        current = *out;
    }
    *out = current;
    Ok(count)
}

fn load_snapshot(root: [u8; 32], out: &mut [Entry; fs::MAX_ENTRIES]) -> Result<usize, i32> {
    let mut bytes = [0u8; SNAPSHOT_BYTES];
    store_get(root, &mut bytes)?;
    decode_snapshot(&bytes, out)
}

fn decode_snapshot(
    bytes: &[u8; SNAPSHOT_BYTES],
    entries: &mut [Entry; fs::MAX_ENTRIES],
) -> Result<usize, i32> {
    if bytes[..8] != SNAPSHOT_MAGIC
        || u32::from_le_bytes(
            bytes[OFF_SNAPSHOT_VERSION..OFF_SNAPSHOT_VERSION + 4]
                .try_into()
                .unwrap(),
        ) != SNAPSHOT_VERSION
    {
        return Err(-7);
    }
    let count = u32::from_le_bytes(
        bytes[OFF_SNAPSHOT_COUNT..OFF_SNAPSHOT_COUNT + 4]
            .try_into()
            .unwrap(),
    ) as usize;
    if count > fs::MAX_ENTRIES {
        return Err(-7);
    }
    let mut previous: Option<&[u8]> = None;
    for (index, entry) in entries.iter_mut().take(count).enumerate() {
        let offset = SNAPSHOT_HEADER + index * SNAPSHOT_ENTRY_BYTES;
        let kind = bytes[offset + OFF_SNAPSHOT_ENTRY_KIND];
        let name_len = bytes[offset + OFF_SNAPSHOT_ENTRY_NAME_LEN] as usize;
        let name_bytes = &bytes[offset + OFF_SNAPSHOT_ENTRY_NAME
            ..offset + OFF_SNAPSHOT_ENTRY_NAME + fs::MAX_NAME_BYTES];
        if !matches!(kind, 1 | 2)
            || name_len == 0
            || name_len > fs::MAX_NAME_BYTES
            || name_bytes[name_len..].iter().any(|byte| *byte != 0)
            || name_bytes[..name_len]
                .iter()
                .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-')))
        {
            return Err(-7);
        }
        if previous.is_some_and(|name| name >= &name_bytes[..name_len]) {
            return Err(-7);
        }
        let mut name = [0u8; fs::MAX_NAME_BYTES];
        name.copy_from_slice(name_bytes);
        let mut hash = [0u8; 32];
        hash.copy_from_slice(
            &bytes[offset + OFF_SNAPSHOT_ENTRY_HASH..offset + OFF_SNAPSHOT_ENTRY_RESERVED1],
        );
        *entry = Entry {
            kind,
            name_len: name_len as u8,
            name,
            object_type: u32::from_le_bytes(
                bytes[offset + OFF_SNAPSHOT_ENTRY_OBJECT_TYPE
                    ..offset + OFF_SNAPSHOT_ENTRY_PAYLOAD_LEN]
                    .try_into()
                    .unwrap(),
            ),
            payload_len: u32::from_le_bytes(
                bytes[offset + OFF_SNAPSHOT_ENTRY_PAYLOAD_LEN..offset + OFF_SNAPSHOT_ENTRY_HASH]
                    .try_into()
                    .unwrap(),
            ),
            hash,
        };
        previous = Some(&entry.name[..name_len]);
    }
    if bytes[SNAPSHOT_HEADER + count * SNAPSHOT_ENTRY_BYTES..]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(-7);
    }
    Ok(count)
}

fn encode_snapshot(entries: &[Entry; fs::MAX_ENTRIES], count: usize) -> [u8; SNAPSHOT_BYTES] {
    let mut bytes = [0u8; SNAPSHOT_BYTES];
    bytes[..8].copy_from_slice(&SNAPSHOT_MAGIC);
    bytes[OFF_SNAPSHOT_VERSION..OFF_SNAPSHOT_COUNT]
        .copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    bytes[OFF_SNAPSHOT_COUNT..SNAPSHOT_HEADER].copy_from_slice(&(count as u32).to_le_bytes());
    for (index, entry) in entries.iter().take(count).enumerate() {
        let offset = SNAPSHOT_HEADER + index * SNAPSHOT_ENTRY_BYTES;
        bytes[offset + OFF_SNAPSHOT_ENTRY_KIND] = entry.kind;
        bytes[offset + OFF_SNAPSHOT_ENTRY_NAME_LEN] = entry.name_len;
        bytes[offset + OFF_SNAPSHOT_ENTRY_NAME
            ..offset + OFF_SNAPSHOT_ENTRY_NAME + fs::MAX_NAME_BYTES]
            .copy_from_slice(&entry.name);
        bytes[offset + OFF_SNAPSHOT_ENTRY_OBJECT_TYPE..offset + OFF_SNAPSHOT_ENTRY_PAYLOAD_LEN]
            .copy_from_slice(&entry.object_type.to_le_bytes());
        bytes[offset + OFF_SNAPSHOT_ENTRY_PAYLOAD_LEN..offset + OFF_SNAPSHOT_ENTRY_HASH]
            .copy_from_slice(&entry.payload_len.to_le_bytes());
        bytes[offset + OFF_SNAPSHOT_ENTRY_HASH..offset + OFF_SNAPSHOT_ENTRY_RESERVED1]
            .copy_from_slice(&entry.hash);
    }
    bytes
}

fn find_entry<'a>(entries: &'a [Entry], count: usize, name: &[u8]) -> Option<&'a Entry> {
    find_index(entries, count, name).map(|index| &entries[index])
}

fn find_index(entries: &[Entry], count: usize, name: &[u8]) -> Option<usize> {
    entries[..count]
        .iter()
        .position(|entry| &entry.name[..entry.name_len as usize] == name)
}

fn sort_entries(entries: &mut [Entry], count: usize) {
    entries[..count].sort_unstable_by(|left, right| {
        left.name[..left.name_len as usize].cmp(&right.name[..right.name_len as usize])
    });
}

fn request_hash(request: &WireFsRequest) -> [u8; 32] {
    words_to_hash(request.hash0, request.hash1, request.hash2, request.hash3)
}

fn words_to_hash(a: u64, b: u64, c: u64, d: u64) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for (index, word) in [a, b, c, d].into_iter().enumerate() {
        hash[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    hash
}

fn hash_words(hash: [u8; 32]) -> (u64, u64, u64, u64) {
    (
        u64::from_le_bytes(hash[0..8].try_into().unwrap()),
        u64::from_le_bytes(hash[8..16].try_into().unwrap()),
        u64::from_le_bytes(hash[16..24].try_into().unwrap()),
        u64::from_le_bytes(hash[24..32].try_into().unwrap()),
    )
}

/// The object store, opened once over the granted block capability.
///
/// A `static mut` because the three helpers below are called from deep inside
/// the request handlers the oracle wrote, and threading a `&mut ObjectStore`
/// through all of them would be a rewrite of code whose whole value is that it
/// is unchanged. The component is single-threaded and the store is opened
/// before the serve loop starts.
static mut STORE: Option<(ObjectStore, BlockCapability)> = None;

/// Open the store. Called once, before any request is served.
fn open_store() -> Result<(), i32> {
    let mut io = BlockCapability;
    let capacity = device_capacity(&mut io).ok_or(-1)?;
    let mut reader = |lba: u64, out: &mut [u8; SECTOR_BYTES]| -> Result<(), GptError> {
        io.read_sector(lba, out).map_err(|_| GptError::Device)
    };
    let selected = gpt::validate_store_partition(&mut reader, capacity).map_err(|_| -1)?;
    let store = ObjectStore::open(&mut io, &selected.partition).map_err(|_| -1)?;
    // SAFETY: single-threaded, and this runs before the serve loop.
    unsafe { STORE = Some((store, io)) };
    Ok(())
}

fn with_store<T>(body: impl FnOnce(&mut ObjectStore, &mut BlockCapability) -> T) -> T {
    // SAFETY: as `open_store`. The reference does not escape `body`.
    let (store, io) = unsafe { (&raw mut STORE).as_mut() }
        .and_then(Option::as_mut)
        .unwrap_or_else(|| slime_rt::exit(1));
    body(store, io)
}

fn store_stat(hash: [u8; 32]) -> Result<(u32, u32), i32> {
    with_store(|store, _| store.stat(&hash).ok_or(-2))
}

fn store_get(hash: [u8; 32], out: &mut [u8]) -> Result<(), i32> {
    with_store(|store, io| match store.get(io, &hash, out) {
        Ok((_, len)) if len == out.len() => Ok(()),
        Ok(_) => Err(-1),
        Err(_) => Err(-2),
    })
}

fn store_put(object_type: u32, payload: &[u8]) -> Result<[u8; 32], i32> {
    with_store(|store, io| store.put(io, object_type, payload).map_err(|_| -1))
}

/// The device, reached through the granted capability.
struct BlockCapability;

impl BlockIo for BlockCapability {
    fn read_sector(&mut self, lba: u64, out: &mut [u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_READ, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_sector(BLOCK_SLOT, &request.encode(), &mut reply, out);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, data: &[u8; SECTOR_BYTES]) -> Result<(), IoError> {
        let request = block_request(block::OP_WRITE, lba);
        let mut reply = [0u8; block::REPLY_LEN];
        let status =
            slime_rt::block_transact_write(BLOCK_SLOT, &request.encode(), data, &mut reply);
        if status < 0 || decode_block_reply(&reply).sectors_done != 1 {
            return Err(IoError::Device);
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        let request = block_request(block::OP_FLUSH, 0);
        let mut reply = [0u8; block::REPLY_LEN];
        if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply) < 0 {
            return Err(IoError::Device);
        }
        Ok(())
    }
}

/// The device's sector count, measured by binary search over readable LBAs.
fn device_capacity(io: &mut BlockCapability) -> Option<u64> {
    let mut sector = [0u8; SECTOR_BYTES];
    io.read_sector(0, &mut sector).ok()?;
    let mut low = 0u64;
    let mut high = 1u64;
    while io.read_sector(high, &mut sector).is_ok() {
        low = high;
        high = high.checked_mul(2)?;
    }
    while high - low > 1 {
        let middle = low + (high - low) / 2;
        if io.read_sector(middle, &mut sector).is_ok() {
            low = middle;
        } else {
            high = middle;
        }
    }
    Some(low + 1)
}

fn block_request(op: u8, lba: u64) -> WireBlockRequest {
    WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count: if op == block::OP_FLUSH { 0 } else { 1 },
        buffer_phys: 0,
        buffer_pages: 0,
    }
}

fn decode_block_reply(bytes: &[u8; block::REPLY_LEN]) -> WireBlockReply {
    WireBlockReply::decode(bytes).unwrap_or(WireBlockReply {
        magic: 0,
        version: 0,
        status: -1,
        sectors_done: 0,
    })
}

fn reply(
    status: i32,
    entry_count: u32,
    object_type: u32,
    payload_len: u32,
    hash: [u8; 32],
) -> WireFsReply {
    let (hash0, hash1, hash2, hash3) = hash_words(hash);
    WireFsReply {
        magic: fs::FS_MAGIC,
        version: fs::FORMAT_VERSION,
        status,
        entry_count,
        object_type,
        payload_len,
        hash0,
        hash1,
        hash2,
        hash3,
        reserved: 0,
    }
}

fn send_reply(reply: WireFsReply, derived_cap: Option<u32>) {
    let encoded = reply.encode();
    loop {
        let result = match derived_cap {
            Some(slot) => slime_rt::capability_delegate(
                RPC_SLOT,
                slot,
                CapabilityDisposition::Move,
                OBJECT_KIND_DIRECTORY,
                // `transfer` and `directoryDerive` travel with the view because
                // a request *through* it is made by narrowing a transferable
                // copy and delegating that: post-cutover a directory capability
                // crosses as an export the receiver claims, so a view its holder
                // cannot copy out of is one it could use exactly once.
                //
                // Scope, not rights, is what bounds it. The derived view names a
                // subtree, and `directory_derive` composes scopes forward only —
                // the boundary arms below observe exactly that: a name outside
                // the subtree is refused, and a write is refused for want of
                // `directoryWrite`, which this mask does not carry.
                u64::from(
                    RIGHT_DIRECTORY_READ
                        | RIGHT_DIRECTORY_LIST
                        | RIGHT_DIRECTORY_DERIVE
                        | RIGHT_TRANSFER,
                ),
                &encoded,
            ),
            None => slime_rt::send(RPC_SLOT, &encoded, &[]),
        };
        match result {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => {
                drop_capability(derived_cap);
                slime_rt::exit(1);
            }
            _ => return,
        }
    }
}

fn drop_capability(capability: Option<u32>) {
    if let Some(slot) = capability
        && slime_rt::cap_drop(slot) != 0
    {
        slime_rt::exit(1);
    }
}
