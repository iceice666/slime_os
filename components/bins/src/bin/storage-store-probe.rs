#![no_std]
#![no_main]

use slime_proto::store::{self, WireStoreReply, WireStoreRequest};

const STORE_SLOT: u32 = 0;

// Content-addressed identity of the seeded fixture object: the SHA-256 of
// its 512-byte content, split into 4 little-endian u64 wire words. GET
// returns content whose digest must equal this same identity.
const SEEDED_IDENTITY: (u64, u64, u64, u64) = (
    0x66d51817d9904ea1,
    0xc570fad74052e1be,
    0xcc2ef58fbf3f8cff,
    0x17430ffc09fe2801,
);
const SEEDED_CONTENT_DIGEST: [u8; 32] = [
    0xa1, 0x4e, 0x90, 0xd9, 0x17, 0x18, 0xd5, 0x66, 0xbe, 0xe1, 0x52, 0x40, 0xd7, 0xfa, 0x70, 0xc5,
    0xff, 0x8c, 0x3f, 0xbf, 0x8f, 0xf5, 0x2e, 0xcc, 0x01, 0x28, 0xfe, 0x09, 0xfc, 0x0f, 0x43, 0x17,
];
const UNKNOWN_IDENTITY: u64 = 0xdeadbeefdeadbeef;

slime_rt::entry!(main);

fn main() {
    let mut get_buf = [0u8; 512];
    let mut put_buf = [0u8; 512];
    for (i, byte) in put_buf.iter_mut().enumerate() {
        *byte = (i as u32).wrapping_mul(37).wrapping_add(11) as u8;
    }

    // 1. STAT the seeded object identity.
    let reply = store_call(seeded_request(store::OP_STAT));
    print_str(b"[store-probe] stat-seeded ");
    print_status(reply.status);
    print_newline();

    // 2. GET the seeded object and verify type, length, and content hash.
    let mut request = seeded_request(store::OP_GET);
    request.buffer_addr = get_buf.as_mut_ptr() as u64;
    request.payload_len = 512;
    let reply = store_call(request);
    print_str(b"[store-probe] get-seeded ");
    print_status(reply.status);
    if reply.status == 0 {
        if reply.obj_type == 1
            && reply.payload_len == 512
            && slime_rt::sha256(&get_buf) == SEEDED_CONTENT_DIGEST
        {
            print_str(b" hash-ok");
        } else {
            print_str(b" hash-bad");
        }
    }
    print_newline();

    // 3. PUT a new object (type 7, 512-byte pattern payload).
    let mut request = new_request(store::OP_PUT);
    request.buffer_addr = put_buf.as_mut_ptr() as u64;
    request.obj_type = 7;
    request.payload_len = 512;
    let reply = store_call(request);
    print_str(b"[store-probe] put-new ");
    print_status(reply.status);
    print_newline();
    // Save the returned identity (zeros when the put failed).
    let new_identity = (reply.hash0, reply.hash1, reply.hash2, reply.hash3);

    // 4. GET the new object back and compare bytes.
    let mut request = new_request(store::OP_GET);
    (request.hash0, request.hash1, request.hash2, request.hash3) = new_identity;
    request.buffer_addr = get_buf.as_mut_ptr() as u64;
    request.payload_len = 512;
    let reply = store_call(request);
    print_str(b"[store-probe] get-new ");
    print_status(reply.status);
    if reply.status == 0 {
        if reply.payload_len == 512 && get_buf == put_buf {
            print_str(b" bytes-ok");
        } else {
            print_str(b" bytes-bad");
        }
    }
    print_newline();

    // 5. STAT an unknown identity; must report not-found when the store opens.
    let mut request = new_request(store::OP_STAT);
    request.hash0 = UNKNOWN_IDENTITY;
    request.hash1 = UNKNOWN_IDENTITY;
    request.hash2 = UNKNOWN_IDENTITY;
    request.hash3 = UNKNOWN_IDENTITY;
    let reply = store_call(request);
    print_str(b"[store-probe] stat-unknown ");
    print_status(reply.status);
    print_newline();

    print_str(b"[store-probe] done\n");
}

fn new_request(op: u8) -> WireStoreRequest {
    WireStoreRequest {
        magic: store::STORE_MAGIC,
        version: store::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        buffer_addr: 0,
        obj_type: 0,
        payload_len: 0,
        hash0: 0,
        hash1: 0,
        hash2: 0,
        hash3: 0,
    }
}

fn seeded_request(op: u8) -> WireStoreRequest {
    let mut request = new_request(op);
    (request.hash0, request.hash1, request.hash2, request.hash3) = SEEDED_IDENTITY;
    request
}

fn store_call(request: WireStoreRequest) -> WireStoreReply {
    let mut reply_buf = [0u8; store::REPLY_LEN];
    if slime_rt::store_transact(STORE_SLOT, &request.encode(), &mut reply_buf) < 0 {
        hard_fail();
    }
    match WireStoreReply::decode(&reply_buf) {
        Some(reply)
            if reply.magic == store::STORE_MAGIC && reply.version == store::FORMAT_VERSION =>
        {
            reply
        }
        _ => hard_fail(),
    }
}

fn print_str(bytes: &[u8]) {
    slime_rt::debug_write(bytes);
}

fn print_newline() {
    print_str(b"\n");
}

// Prints the signed status as at most a sign character plus one digit,
// matching the original hand-written formatter: magnitudes above 9 print
// as '?' (never observed for real status codes, but preserved for fidelity).
fn print_status(status: i32) {
    let magnitude = status.unsigned_abs();
    let digit = if magnitude <= 9 {
        b'0' + magnitude as u8
    } else {
        b'?'
    };
    if status < 0 {
        print_str(&[b'-', digit]);
    } else {
        print_str(&[digit]);
    }
}

fn hard_fail() -> ! {
    slime_rt::debug_write(b"[store-probe] syscall-error\n");
    slime_rt::exit(1)
}
