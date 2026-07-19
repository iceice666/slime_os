#![no_std]
#![no_main]

use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

const BLOCK_SLOT: u32 = 0;
const TARGET_LBA: u64 = 2;
const MARKER: [u8; 16] = *b"LSM5.3 MABLE\x00\x00\x00\x00";

slime_rt::entry!(main);

fn main() {
    let mut sector = [0u8; 512];

    // Read sector 2 first. Existing content means this is boot #2: verify only.
    if block_request(block::OP_READ, TARGET_LBA, 1, sector.as_mut_ptr() as u64).status != 0 {
        fail();
    }

    if sector[..8] != MARKER[..8] {
        sector[..16].copy_from_slice(&MARKER);
        for (i, byte) in sector.iter_mut().enumerate().skip(16) {
            *byte = (i as u32).wrapping_mul(29).wrapping_add(7) as u8;
        }

        if block_request(block::OP_WRITE, TARGET_LBA, 1, sector.as_mut_ptr() as u64).status != 0 {
            fail();
        }
        if block_request(block::OP_FLUSH, 0, 0, 0).status != 0 {
            fail();
        }

        // Clear and read back immediately.
        sector = [0u8; 512];
    }

    if block_request(block::OP_READ, TARGET_LBA, 1, sector.as_mut_ptr() as u64).status != 0 {
        fail();
    }
    if sector[..16] != MARKER[..] {
        fail();
    }

    slime_rt::debug_write(b"[storage-writer] durable sector verified\n");
}

fn block_request(op: u8, lba: u64, sector_count: u32, buffer_ptr: u64) -> WireBlockReply {
    let (buffer_pages, buffer_phys) = if sector_count != 0 {
        (1, buffer_ptr)
    } else {
        (0, 0)
    };
    let request = WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags: 0,
        reserved: 0,
        lba,
        sector_count,
        buffer_pages,
        buffer_phys,
    };
    let mut reply_buf = [0u8; block::REPLY_LEN];
    if slime_rt::block_transact(BLOCK_SLOT, &request.encode(), &mut reply_buf) < 0 {
        fail();
    }
    WireBlockReply::decode(&reply_buf).expect("kernel always returns a full-length reply")
}

fn fail() -> ! {
    slime_rt::debug_write(b"[storage-writer] failed\n");
    slime_rt::exit(1)
}
