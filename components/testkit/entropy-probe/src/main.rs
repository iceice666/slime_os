#![no_std]
#![no_main]

//! Entropy holders, observed from inside each instance.
//!
//! One image runs as every holder in `sel4-entropy`. Each learns which holder
//! it is from the one entropy grant its generation gives it, never from its
//! name; an instance with no such grant is the unauthorised probe and can
//! only report that it has nothing to call.

use slime_proto::entropy_service::{
    self, ENTROPY_SERVICE_MAGIC, MAX_DRAW, OP_CLOSE, OP_DRAW, STATUS_EXHAUSTED, STATUS_MALFORMED,
    STATUS_OK, STATUS_UNAVAILABLE, WireEntropyServiceReply, WireEntropyServiceRequest,
};
use slime_proto::valid_entropy_service_reply;
use slime_rt::{MAX_MSG, debug_write, exit};

slime_rt::entry!(main);

const BUDGET: u32 = 96;

#[derive(Clone, Copy)]
enum Program {
    /// Draw until the budget is spent, then require the next draw refused.
    Exhaust,
    /// Draw once, then require both out-of-range sizes refused.
    Malformed,
}

const HOLDERS: [(&[u8], &[u8], Program); 3] = [
    (b"entropy-hw-a-entropy", b"entropy-hw-a", Program::Exhaust),
    (b"entropy-hw-b-entropy", b"entropy-hw-b", Program::Malformed),
    (
        b"entropy-seeded-entropy",
        b"entropy-seeded",
        Program::Exhaust,
    ),
];

fn main(_startup_arg: u32) {
    let Some((slot, holder, program)) = HOLDERS.iter().find_map(|(binding, holder, program)| {
        slime_rt::resolve_binding(binding)
            .ok()
            .map(|slot| (slot, *holder, *program))
    }) else {
        debug_write(b"[entropy-probe] holder=entropy-intruder binding_absent=1\n");
        exit(0)
    };
    let probe = Probe { slot, holder };
    match program {
        Program::Exhaust => {
            let mut index = 0u32;
            while index * MAX_DRAW as u32 + MAX_DRAW as u32 <= BUDGET {
                if !probe.draw(index) {
                    probe.finish();
                }
                index += 1;
            }
            probe.require(
                OP_DRAW,
                MAX_DRAW as u32,
                STATUS_EXHAUSTED,
                b"over-budget draw",
            );
            probe
                .line()
                .push(b" exhausted=1 drawn=")
                .number(u64::from(index * MAX_DRAW as u32))
                .emit();
        }
        Program::Malformed => {
            if !probe.draw(0) {
                probe.finish();
            }
            probe.require(OP_DRAW, 0, STATUS_MALFORMED, b"empty draw");
            probe.require(
                OP_DRAW,
                MAX_DRAW as u32 + 1,
                STATUS_MALFORMED,
                b"oversized draw",
            );
            probe.line().push(b" malformed_refused=2").emit();
        }
    }
    probe.finish()
}

struct Probe {
    slot: u32,
    holder: &'static [u8],
}

impl Probe {
    /// Draw one full block and print it. `false` when the service has no
    /// source for this holder, which is reported and ends the program.
    fn draw(&self, index: u32) -> bool {
        let reply = self.call(OP_DRAW, MAX_DRAW as u32);
        match reply.status {
            STATUS_OK if reply.length as usize == MAX_DRAW => {
                self.line()
                    .push(b" draw=")
                    .number(u64::from(index))
                    .push(b" hex=")
                    .hex(&reply.bytes)
                    .emit();
                true
            }
            STATUS_UNAVAILABLE if index == 0 => {
                self.line().push(b" unavailable=1").emit();
                false
            }
            _ => fail(b"draw refused"),
        }
    }

    fn require(&self, op: u32, length: u32, status: i32, what: &[u8]) {
        if self.call(op, length).status != status {
            fail(what);
        }
    }

    fn finish(&self) -> ! {
        self.require(OP_CLOSE, 0, STATUS_OK, b"close");
        exit(0)
    }

    fn line(&self) -> Line {
        let mut line = Line::new(b"[entropy-probe] holder=");
        line.push(self.holder);
        line
    }

    fn call(&self, op: u32, length: u32) -> WireEntropyServiceReply {
        let request = WireEntropyServiceRequest {
            magic: ENTROPY_SERVICE_MAGIC,
            version: entropy_service::FORMAT_VERSION,
            op,
            length,
        };
        let mut answer = [0u8; MAX_MSG];
        let received = slime_rt::call(self.slot, &request.encode(), &mut answer);
        if received < 0 || received as usize != entropy_service::REPLY_LEN {
            fail(b"service call");
        }
        WireEntropyServiceReply::decode(&answer[..entropy_service::REPLY_LEN])
            .filter(valid_entropy_service_reply)
            .unwrap_or_else(|| fail(b"service reply"))
    }
}

/// One serial line, written with a single `debug_write` so another
/// component's output cannot split it.
struct Line {
    bytes: [u8; 160],
    used: usize,
}

impl Line {
    fn new(prefix: &[u8]) -> Self {
        let mut line = Self {
            bytes: [0; 160],
            used: 0,
        };
        line.push(prefix);
        line
    }

    fn push(&mut self, text: &[u8]) -> &mut Self {
        let end = self
            .used
            .checked_add(text.len())
            .filter(|end| *end <= self.bytes.len())
            .unwrap_or_else(|| fail(b"marker bounds"));
        self.bytes[self.used..end].copy_from_slice(text);
        self.used = end;
        self
    }

    fn number(&mut self, mut value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let mut cursor = digits.len();
        loop {
            cursor -= 1;
            digits[cursor] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.push(&digits[cursor..])
    }

    fn hex(&mut self, bytes: &[u8]) -> &mut Self {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        for byte in bytes {
            self.push(&[
                DIGITS[usize::from(byte >> 4)],
                DIGITS[usize::from(byte & 0x0f)],
            ]);
        }
        self
    }

    fn emit(&mut self) {
        self.push(b"\n");
        debug_write(&self.bytes[..self.used]);
    }
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[entropy-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
