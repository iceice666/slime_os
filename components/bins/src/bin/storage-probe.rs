#![no_std]
#![no_main]

use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

const BLOCK_SLOT: u32 = 0;

// SHA-256 of the fixture's 512-byte sector 0 (contracts/block/v1 fixture),
// verified against the value the original hand-written sha256_sector
// produced in storage-probe.S.
const EXPECTED_DIGEST: [u8; 32] = [
    0xd2, 0x43, 0x5a, 0xe7, 0xc5, 0xe5, 0x52, 0x90, 0xd0, 0x39, 0x80, 0x66, 0xc4, 0x79, 0x49, 0x7b,
    0xcb, 0xa9, 0xfa, 0x3e, 0xd2, 0x08, 0x7d, 0xe4, 0x85, 0x67, 0xbf, 0xe7, 0x49, 0xe6, 0xfa, 0x67,
];

slime_rt::entry!(main);

fn main() {
    let mut sector = [0u8; 512];

    let request = WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op: block::OP_READ,
        flags: 0,
        reserved: 0,
        lba: 0,
        sector_count: 1,
        buffer_pages: 1,
        buffer_phys: sector.as_mut_ptr() as u64,
    };

    let reply = block_call(&request);
    if reply.status != 0 || reply.sectors_done != 1 {
        fail();
    }

    // The fixture begins with this stable prefix.
    if &sector[..8] != b"Slime OS" {
        fail();
    }

    if slime_rt::sha256(&sector) != EXPECTED_DIGEST {
        fail();
    }

    // The read-only capability cannot authorize a write request.
    let mut write_request = request;
    write_request.op = block::OP_WRITE;
    let mut discard = [0u8; 64];
    if slime_rt::block_transact(BLOCK_SLOT, &write_request.encode(), &mut discard) != -1 {
        fail();
    }

    // A buffer declaration smaller than the payload is rejected before I/O.
    let mut oversized = request;
    oversized.sector_count = 9;
    if block_call(&oversized).status != -5 {
        fail();
    }

    // The device capacity is eight sectors; LBA 8 returns out-of-range.
    let mut out_of_range = request;
    out_of_range.lba = 8;
    if block_call(&out_of_range).status != -3 {
        fail();
    }

    slime_rt::debug_write(b"[storage-probe] sector 0 verified\n");
}

fn block_call(request: &WireBlockRequest) -> WireBlockReply {
    let mut reply_buf = [0u8; 64];
    if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply_buf) < 0 {
        fail();
    }
    match WireBlockReply::decode(&reply_buf) {
        Some(reply)
            if reply.magic == block::BLOCK_MAGIC && reply.version == block::FORMAT_VERSION =>
        {
            reply
        }
        _ => fail(),
    }
}

fn fail() -> ! {
    slime_rt::debug_write(b"[storage-probe] failed\n");
    slime_rt::exit(1)
}
