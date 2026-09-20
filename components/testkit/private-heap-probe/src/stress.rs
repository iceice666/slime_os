use alloc::{boxed::Box, vec::Vec};
use core::fmt::{self, Write};
use slime_proto::private_memory_probe::{self as protocol, WireMessage};

const BLOCK: usize = 1024 * 1024;
const COUNT: usize = 120;
const SMALL_COUNT: usize = 256;
const SMALL_BYTES: usize = 256;
const PAYLOAD: usize = 2 * COUNT * BLOCK + SMALL_COUNT * SMALL_BYTES;
const REFUSED_BYTES: usize = 16 * BLOCK;

struct Console;
impl Write for Console {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        slime_rt::debug_write(text.as_bytes());
        Ok(())
    }
}

fn trace(args: fmt::Arguments<'_>) {
    let _ = Console.write_fmt(args);
    slime_rt::debug_write(b"\n");
}

fn require(condition: bool, reason: &[u8]) {
    if !condition {
        slime_rt::debug_write(b"[private-heap-probe:stress] FAIL ");
        slime_rt::debug_write(reason);
        slime_rt::debug_write(b"\n");
        slime_rt::exit(1);
    }
}

fn pattern(identity: usize, offset: usize) -> u8 {
    (identity as u8).wrapping_mul(37) ^ (offset as u8).wrapping_mul(13) ^ ((offset >> 12) as u8)
}

fn block(size: usize, identity: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    require(bytes.try_reserve_exact(size).is_ok(), b"payload allocation");
    bytes.resize(size, 0);
    for (offset, byte) in bytes.iter_mut().enumerate() {
        *byte = pattern(identity, offset);
    }
    bytes
}

fn check(bytes: &[u8], identity: usize) {
    for (offset, byte) in bytes.iter().enumerate() {
        require(*byte == pattern(identity, offset), b"payload corruption");
    }
}

struct Payload {
    vectors: Vec<Option<Vec<u8>>>,
    boxes: Vec<Option<Box<[u8]>>>,
    small: Vec<Box<[u8]>>,
}

impl Payload {
    fn new() -> Self {
        let mut value = Self {
            vectors: Vec::new(),
            boxes: Vec::new(),
            small: Vec::new(),
        };
        require(
            value.vectors.try_reserve_exact(COUNT).is_ok(),
            b"vector table",
        );
        require(value.boxes.try_reserve_exact(COUNT).is_ok(), b"box table");
        require(
            value.small.try_reserve_exact(SMALL_COUNT).is_ok(),
            b"small table",
        );
        for index in 0..COUNT {
            value.vectors.push(Some(block(BLOCK, index)));
            value
                .boxes
                .push(Some(block(BLOCK, COUNT + index).into_boxed_slice()));
        }
        for index in 0..SMALL_COUNT {
            value
                .small
                .push(block(SMALL_BYTES, 2 * COUNT + index).into_boxed_slice());
        }
        value.verify();
        value
    }

    fn verify(&self) {
        for index in 0..COUNT {
            if let Some(bytes) = &self.vectors[index] {
                check(bytes, index);
            }
            if let Some(bytes) = &self.boxes[index] {
                check(bytes, COUNT + index);
            }
        }
        for (index, bytes) in self.small.iter().enumerate() {
            check(bytes, 2 * COUNT + index);
        }
    }

    fn holes(&mut self) {
        let before = slime_rt::private_heap_stats();
        for index in (0..COUNT).step_by(2) {
            self.vectors[index] = None;
            self.boxes[index] = None;
        }
        self.verify();
        for index in (0..COUNT).step_by(2) {
            self.vectors[index] = Some(block(BLOCK, index));
            self.boxes[index] = Some(block(BLOCK, COUNT + index).into_boxed_slice());
        }
        self.verify();
        let after = slime_rt::private_heap_stats();
        require(
            before.pages == after.pages && before.growths == after.growths,
            b"holes grew backing",
        );
        require(before.live == after.live, b"holes changed live bytes");
        trace(format_args!(
            "[private-heap-probe:stress] holes reused=1 growths={} pages={}",
            after.growths, after.pages
        ));
    }
}

fn exercise() -> Payload {
    let mut payload = Payload::new();
    let held = slime_rt::private_heap_stats();
    require(
        held.pages <= 65536 && held.live >= PAYLOAD,
        b"capacity accounting",
    );
    trace(format_args!(
        "[private-heap-probe:stress] capacity payload={} overhead={} backed={} pages={} touched=1 vecs=120 boxes=120 small=256",
        PAYLOAD,
        held.live - PAYLOAD,
        held.pages * 4096,
        held.pages
    ));
    payload.holes();
    let mut impossible: Vec<u8> = Vec::new();
    require(
        impossible.try_reserve_exact(REFUSED_BYTES).is_err(),
        b"exhaustion accepted",
    );
    require(
        impossible.capacity() == 0,
        b"failed allocation retained capacity",
    );
    payload.verify();
    let refused = slime_rt::private_heap_stats();
    require(
        refused.pages == held.pages && refused.growths == held.growths && refused.live == held.live,
        b"refusal changed heap",
    );
    trace(format_args!(
        "[private-heap-probe:stress] exhaustion requested={} refused=1 intact=1 pages={}",
        REFUSED_BYTES, refused.pages
    ));
    payload
}

pub fn run(endpoint: u32) -> ! {
    require(
        slime_rt::private_heap_stats().live == 0,
        b"initial live bytes",
    );
    let mut payload = None;
    let mut verified = false;
    loop {
        let mut bytes = [0; slime_rt::MAX_MSG];
        let mut caps = [0; slime_rt::MAX_CAPS_PER_MSG];
        let size = slime_rt::recv_blocking(endpoint, &mut bytes, &mut caps);
        require(size == protocol::MESSAGE_LEN as i64, b"message length");
        let Some(request) = WireMessage::decode(&bytes[..protocol::MESSAGE_LEN]) else {
            require(false, b"decode");
            unreachable!();
        };
        require(
            request.version == protocol::FORMAT_VERSION
                && request.holder == 3
                && request.incarnation == 0
                && request.address == 0
                && request.reserved == [0; 40],
            b"message fields",
        );
        let mut finish = false;
        match request.operation {
            protocol::GROW => {
                require(payload.is_none(), b"duplicate grow");
                payload = Some(exercise());
            }
            protocol::VERIFY => {
                require(payload.is_some(), b"verify before grow");
                if let Some(value) = &payload {
                    value.verify();
                }
                verified = true;
                trace(format_args!(
                    "[private-heap-probe:stress] verified payload={} intact=1",
                    PAYLOAD
                ));
            }
            protocol::FINISH => {
                require(verified && payload.is_some(), b"finish before verification");
                if let Some(value) = &payload {
                    value.verify();
                }
                let before = slime_rt::private_heap_stats();
                drop(payload.take());
                require(
                    slime_rt::private_heap_stats().live == 0,
                    b"release leaked bytes",
                );
                let reuse = Payload::new();
                let after = slime_rt::private_heap_stats();
                require(
                    after.pages == before.pages && after.growths == before.growths,
                    b"released payload grew backing",
                );
                drop(reuse);
                require(
                    slime_rt::private_heap_stats().live == 0,
                    b"reuse leaked bytes",
                );
                trace(format_args!(
                    "[private-heap-probe:stress] released live=0 reused=1 growths={} pages={}",
                    after.growths, after.pages
                ));
                finish = true;
            }
            _ => require(false, b"operation"),
        }
        let reply = WireMessage {
            operation: protocol::ACKNOWLEDGE,
            ..request
        };
        require(
            slime_rt::reply(&reply.encode()) == slime_rt::ERR_SUCCESS,
            b"reply",
        );
        if finish {
            slime_rt::exit(0);
        }
    }
}
