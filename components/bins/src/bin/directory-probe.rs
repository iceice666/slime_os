#![no_std]
#![no_main]

use slime_proto::fs::{self, WireFsReply, WireFsRequest};
use slime_rt::{ERR_BAD_CAP, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const DIRECTORY_SLOT: u32 = 1;
const PAYLOAD_HASH: [u8; 32] = [
    0x80, 0xe6, 0xbb, 0x6b, 0x33, 0x8c, 0x72, 0xd3, 0xdd, 0x0f, 0xdc, 0x6d, 0x94, 0x25, 0x70, 0x4b,
    0xa6, 0xa0, 0x3f, 0x8d, 0x0c, 0xd8, 0x19, 0x47, 0x0c, 0xf1, 0x04, 0xc6, 0x57, 0x2e, 0x53, 0xd6,
];
const PAYLOAD_LEN: u32 = 30;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
const RIGHT_DIRECTORY_LIST: u32 = 1 << 21;
const ZERO_HASH: [u8; 32] = [0; 32];
fn main() {
    let denied = slime_rt::directory_derive(RPC_SLOT, b"docs", RIGHT_DIRECTORY_READ);
    if denied != Err(ERR_BAD_CAP) {
        fail();
    }
    slime_rt::debug_write(b"[directory-probe] no-cap denied\n");

    let (initial, _) = call(request(fs::OP_READ, b"note", 0, [0; 32]), DIRECTORY_SLOT);
    if initial.status != 0 || reply_hash(initial) != PAYLOAD_HASH {
        fail();
    }

    let interrupted = call(
        request(fs::OP_WRITE, b"orphan.txt", PAYLOAD_LEN, ZERO_HASH),
        DIRECTORY_SLOT,
    )
    .0;
    if interrupted.status == 0 {
        fail();
    }
    let (still_readable, _) = call(request(fs::OP_READ, b"note", 0, ZERO_HASH), DIRECTORY_SLOT);
    if still_readable.status != 0 || reply_hash(still_readable) != PAYLOAD_HASH {
        fail();
    }
    slime_rt::debug_write(b"[directory-probe] interrupted transition preserved root\n");
    slime_rt::debug_write(b"[directory-probe] scoped read ok\n");

    let (write, _) = call(
        request(fs::OP_WRITE, b"new.txt", PAYLOAD_LEN, PAYLOAD_HASH),
        DIRECTORY_SLOT,
    );
    if write.status != 0 {
        fail();
    }
    let (read, _) = call(request(fs::OP_READ, b"new.txt", 0, [0; 32]), DIRECTORY_SLOT);
    if read.status != 0 || reply_hash(read) != PAYLOAD_HASH || read.payload_len != PAYLOAD_LEN {
        fail();
    }
    slime_rt::debug_write(b"[directory-probe] root transition committed\n");

    let (derived, derived_slot) = call(request(fs::OP_DERIVE, b"docs", 0, [0; 32]), DIRECTORY_SLOT);
    let derived_slot = match derived_slot {
        Some(slot) => slot,
        None => fail(),
    };
    if derived.status != 0
        || slime_rt::directory_inspect(
            derived_slot,
            RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_LIST,
            &mut [0; 32],
            &mut [0; slime_rt::MAX_DIRECTORY_PATH],
        )
        .is_err()
        || slime_rt::directory_commit(derived_slot, &[0; 32], &[0; 32]) != ERR_BAD_CAP
    {
        fail();
    }
    slime_rt::debug_write(b"[directory-probe] derive narrowed\n");
    let (scoped_read, _) = call(request(fs::OP_READ, b"note", 0, [0; 32]), derived_slot);
    if scoped_read.status != 0 || reply_hash(scoped_read) != PAYLOAD_HASH {
        fail();
    }
    let (outside_scope, _) = call(request(fs::OP_READ, b"new.txt", 0, [0; 32]), derived_slot);
    if outside_scope.status != -3 {
        fail();
    }
    let (scoped_write, _) = call(
        request(fs::OP_WRITE, b"blocked.txt", PAYLOAD_LEN, PAYLOAD_HASH),
        derived_slot,
    );
    if scoped_write.status != -2 {
        fail();
    }
    let _ = slime_rt::cap_drop(derived_slot);
    slime_rt::debug_write(b"[directory-probe] scoped boundary enforced\n");

    let (malformed, _) = call(
        request(fs::OP_READ, b"bad/name", 0, [0; 32]),
        DIRECTORY_SLOT,
    );
    if malformed.status != -1 {
        fail();
    }
    slime_rt::debug_write(b"[directory-probe] malformed rejected\n");
    slime_rt::debug_write(b"[directory-probe] done\n");
}

fn request(op: u8, name: &[u8], payload_len: u32, hash: [u8; 32]) -> WireFsRequest {
    let mut encoded_name = [0u8; fs::MAX_NAME_BYTES];
    encoded_name[..name.len()].copy_from_slice(name);
    let (hash0, hash1, hash2, hash3) = hash_words(hash);
    WireFsRequest {
        magic: fs::FS_MAGIC,
        version: fs::FORMAT_VERSION,
        op,
        flags: 0,
        name_len: name.len() as u8,
        reserved0: 0,
        name: encoded_name,
        payload_len,
        hash0,
        hash1,
        hash2,
        hash3,
    }
}

fn call(request: WireFsRequest, directory_slot: u32) -> (WireFsReply, Option<u32>) {
    let encoded = request.encode();
    let grant = [directory_slot];
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &grant) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(),
            _ => break,
        }
    }
    let mut reply = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RPC_SLOT, &mut reply, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(),
            n => {
                if caps[0] == 0 || caps[2..].iter().any(|slot| *slot != 0) {
                    fail();
                }
                let returned_directory = caps[0] as u32;
                if returned_directory != directory_slot {
                    if slime_rt::cap_drop(returned_directory) != 0 {
                        fail();
                    }
                    fail();
                }
                let derived = (caps[1] != 0).then_some(caps[1] as u32);
                let decoded = match WireFsReply::decode(&reply[..n as usize]) {
                    Some(reply) => reply,
                    None => fail(),
                };
                return (decoded, derived);
            }
        }
    }
}

fn reply_hash(reply: WireFsReply) -> [u8; 32] {
    let mut hash = [0u8; 32];
    for (index, word) in [reply.hash0, reply.hash1, reply.hash2, reply.hash3]
        .into_iter()
        .enumerate()
    {
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
fn fail() -> ! {
    slime_rt::debug_write(b"[directory-probe] failed\n");
    slime_rt::exit(1)
}
