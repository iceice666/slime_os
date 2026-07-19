#![no_std]
#![no_main]

use slime_proto::block::{self, WireBlockReply, WireBlockRequest};

const BLOCK_SLOT: u32 = 0;

slime_rt::entry!(main);

fn main() {
    let mut sector = [0x5au8; 512];
    let buffer_ptr = sector.as_mut_ptr() as u64;

    // Every injected request must fail with its structured status and must
    // not poison the next request. Replays consume the exact last recorded
    // input.
    expect_status(
        block_request(
            block::OP_WRITE,
            block::FLAG_INJECT_REQUEST_FAILURE,
            3,
            1,
            buffer_ptr,
        ),
        -7,
    );
    expect_status(
        block_request(block::OP_READ, block::FLAG_REPLAY_LAST, 0, 1, buffer_ptr),
        -7,
    );

    expect_status(
        block_request(block::OP_READ, block::FLAG_INJECT_TIMEOUT, 0, 1, buffer_ptr),
        -8,
    );
    expect_status(
        block_request(block::OP_READ, block::FLAG_INJECT_RESET, 0, 1, buffer_ptr),
        -7,
    );

    expect_status(
        block_request(block::OP_FLUSH, block::FLAG_INJECT_FLUSH_FAILURE, 0, 0, 0),
        -7,
    );

    expect_status(
        block_request(
            block::OP_WRITE,
            block::FLAG_INJECT_INTERRUPTED,
            3,
            1,
            buffer_ptr,
        ),
        -7,
    );

    // Out-of-range write is rejected before any device data transfer.
    expect_status(block_request(block::OP_WRITE, 0, 8, 1, buffer_ptr), -3);

    // The service/device must remain usable after every failure and reset.
    expect_status(block_request(block::OP_READ, 0, 0, 1, buffer_ptr), 0);

    slime_rt::debug_write(b"[storage-fault-probe] recovery and replay verified\n");
}

fn expect_status(reply: WireBlockReply, expected: i32) {
    if reply.status != expected {
        fail();
    }
}

fn block_request(
    op: u8,
    flags: u8,
    lba: u64,
    sector_count: u32,
    buffer_ptr: u64,
) -> WireBlockReply {
    let (buffer_pages, buffer_phys) = if sector_count != 0 {
        (1, buffer_ptr)
    } else {
        (0, 0)
    };
    let request = WireBlockRequest {
        magic: block::BLOCK_MAGIC,
        version: block::FORMAT_VERSION,
        op,
        flags,
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
    slime_rt::debug_write(b"[storage-fault-probe] failed\n");
    slime_rt::exit(1)
}
