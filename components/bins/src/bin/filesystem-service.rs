#![no_std]
#![no_main]

use slime_proto::{
    fs::{self, WireFsReply, WireFsRequest},
    store::{self, WireStoreReply, WireStoreRequest},
    valid_fs_request,
};
use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_DIRECTORY_PATH, MAX_MSG};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const STORE_SLOT: u32 = 1;
const MAX_OBJECT_PAYLOAD: u32 = 32 * 1024;
const RIGHT_TRANSFER: u32 = 1 << 2;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
const RIGHT_DIRECTORY_WRITE: u32 = 1 << 20;
const RIGHT_DIRECTORY_LIST: u32 = 1 << 21;
const RIGHT_DIRECTORY_DERIVE: u32 = 1 << 22;
const SNAPSHOT_MAGIC: [u8; 8] = *b"SLIMEDIR";
const SNAPSHOT_VERSION: u32 = 1;
const SNAPSHOT_HEADER: usize = 16;
const ENTRY_BYTES: usize = 64;
const SNAPSHOT_BYTES: usize = SNAPSHOT_HEADER + fs::MAX_ENTRIES * ENTRY_BYTES;
const SNAPSHOT_OBJECT_TYPE: u32 = 0x4452_4953;
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

fn main() {
    slime_rt::debug_write(b"[filesystem] ready\n");
    loop {
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        match slime_rt::recv(RPC_SLOT, &mut message, &mut received_caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => slime_rt::exit(0),
            n if n < 0 => slime_rt::exit(1),
            n => {
                let (reply, directory_cap, derived_cap) =
                    handle(&message[..n as usize], &received_caps);
                send_reply(reply, directory_cap, derived_cap);
            }
        }
    }
}

fn handle(
    message: &[u8],
    received_caps: &[u64; MAX_CAPS_PER_MSG],
) -> (WireFsReply, Option<u32>, Option<u32>) {
    if received_caps[0] == 0 || received_caps[1..].iter().any(|slot| *slot != 0) {
        release_caps(received_caps);
        return (reply(-2, 0, 0, 0, ZERO_HASH), None, None);
    }
    let directory_slot = received_caps[0] as u32;
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
                RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_LIST | RIGHT_TRANSFER,
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
        || u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != SNAPSHOT_VERSION
    {
        return Err(-7);
    }
    let count = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    if count > fs::MAX_ENTRIES {
        return Err(-7);
    }
    let mut previous: Option<&[u8]> = None;
    for (index, entry) in entries.iter_mut().take(count).enumerate() {
        let offset = SNAPSHOT_HEADER + index * ENTRY_BYTES;
        let kind = bytes[offset];
        let name_len = bytes[offset + 1] as usize;
        let name_bytes = &bytes[offset + 4..offset + 4 + fs::MAX_NAME_BYTES];
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
        hash.copy_from_slice(&bytes[offset + 28..offset + 60]);
        *entry = Entry {
            kind,
            name_len: name_len as u8,
            name,
            object_type: u32::from_le_bytes(bytes[offset + 20..offset + 24].try_into().unwrap()),
            payload_len: u32::from_le_bytes(bytes[offset + 24..offset + 28].try_into().unwrap()),
            hash,
        };
        previous = Some(&entry.name[..name_len]);
    }
    if bytes[SNAPSHOT_HEADER + count * ENTRY_BYTES..]
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
    bytes[8..12].copy_from_slice(&SNAPSHOT_VERSION.to_le_bytes());
    bytes[12..16].copy_from_slice(&(count as u32).to_le_bytes());
    for (index, entry) in entries.iter().take(count).enumerate() {
        let offset = SNAPSHOT_HEADER + index * ENTRY_BYTES;
        bytes[offset] = entry.kind;
        bytes[offset + 1] = entry.name_len;
        bytes[offset + 4..offset + 4 + fs::MAX_NAME_BYTES].copy_from_slice(&entry.name);
        bytes[offset + 20..offset + 24].copy_from_slice(&entry.object_type.to_le_bytes());
        bytes[offset + 24..offset + 28].copy_from_slice(&entry.payload_len.to_le_bytes());
        bytes[offset + 28..offset + 60].copy_from_slice(&entry.hash);
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

fn store_stat(hash: [u8; 32]) -> Result<(u32, u32), i32> {
    let reply = store_call(store_request(store::OP_STAT, hash, 0, 0));
    if reply.status == 0 {
        Ok((reply.obj_type, reply.payload_len))
    } else {
        Err(reply.status)
    }
}

fn store_get(hash: [u8; 32], out: &mut [u8]) -> Result<(), i32> {
    let reply = store_call(store_request(
        store::OP_GET,
        hash,
        out.as_mut_ptr() as u64,
        out.len() as u32,
    ));
    if reply.status == 0 && reply.payload_len as usize == out.len() {
        Ok(())
    } else {
        Err(reply.status)
    }
}

fn store_put(object_type: u32, payload: &[u8]) -> Result<[u8; 32], i32> {
    let mut request = store_request(
        store::OP_PUT,
        ZERO_HASH,
        payload.as_ptr() as u64,
        payload.len() as u32,
    );
    request.obj_type = object_type;
    let reply = store_call(request);
    if reply.status == 0 {
        Ok(words_to_hash(
            reply.hash0,
            reply.hash1,
            reply.hash2,
            reply.hash3,
        ))
    } else {
        Err(reply.status)
    }
}

fn store_request(op: u8, hash: [u8; 32], address: u64, len: u32) -> WireStoreRequest {
    let (hash0, hash1, hash2, hash3) = hash_words(hash);
    WireStoreRequest {
        magic: store::STORE_MAGIC,
        version: store::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        buffer_addr: address,
        obj_type: 0,
        payload_len: len,
        hash0,
        hash1,
        hash2,
        hash3,
    }
}

fn store_call(request: WireStoreRequest) -> WireStoreReply {
    let mut reply = [0u8; store::REPLY_LEN];
    if slime_rt::store_transact(STORE_SLOT, &request.encode(), &mut reply) < 0 {
        slime_rt::exit(1);
    }
    WireStoreReply::decode(&reply).unwrap_or_else(|| slime_rt::exit(1))
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

fn send_reply(reply: WireFsReply, directory_cap: Option<u32>, derived_cap: Option<u32>) {
    let encoded = reply.encode();
    let mut caps = [0u32; 2];
    let mut cap_count = 0;
    if let Some(slot) = directory_cap {
        caps[cap_count] = slot;
        cap_count += 1;
    }
    if let Some(slot) = derived_cap {
        caps[cap_count] = slot;
        cap_count += 1;
    }
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &caps[..cap_count]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => slime_rt::exit(0),
            result if result < 0 => slime_rt::exit(1),
            _ => return,
        }
    }
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().copied().filter(|slot| *slot != 0) {
        if slime_rt::cap_drop(slot as u32) != 0 {
            slime_rt::exit(1);
        }
    }
}
