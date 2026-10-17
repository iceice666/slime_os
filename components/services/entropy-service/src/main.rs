#![no_std]
#![no_main]

//! The entropy service: per-holder HMAC-DRBG streams under generation-declared
//! byte budgets, served as `contracts/entropy-service/v1` over each holder's
//! own declared endpoint.
//!
//! Which holders exist, their sources, budgets, and seeds come only from the
//! authenticated `entropy-authority` resource. A holder is identified by the
//! endpoint its request arrived on, so no request can name another holder. A
//! hardware holder's generator is instantiated and reseeded only from device
//! bytes the virtio-rng driver returned; without the device it is answered
//! `unavailable`, never with bytes from any other source.

use boot_contracts::entropy_authority::{self, EntropyAuthority, Source};
use slime_components::hmac_drbg::{HmacDrbg, PERSONALIZATION_PREFIX};
use slime_proto::entropy_service::{
    self, ENTROPY_SERVICE_MAGIC, MAX_DRAW, OP_CLOSE, OP_DRAW, STATUS_DENIED, STATUS_EXHAUSTED,
    STATUS_MALFORMED, STATUS_OK, STATUS_UNAVAILABLE, WireEntropyServiceReply,
    WireEntropyServiceRequest,
};
use slime_proto::entropy_source::{
    self, ENTROPY_SOURCE_MAGIC, MAX_FILL, OP_FILL, OP_RELEASE, WireEntropySourceReply,
    WireEntropySourceRequest,
};
use slime_proto::{valid_entropy_service_request, valid_entropy_source_reply};
use slime_rt::{
    ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, entropy_authority_read, exit,
    resolve_binding, yield_now,
};

slime_rt::entry!(main);

/// Successful draws a hardware generator serves before it is reseeded from a
/// fresh device fill.
const RESEED_INTERVAL: u32 = 64;
const ENTROPY_BYTES: usize = 32;
const NONCE_BYTES: usize = 16;
const NAME_BYTES: usize = entropy_authority::HOLDER_NAME_BYTES;
const BINDING_SUFFIX: &[u8] = b"-entropy";
const PAGE_ROWS: usize = 4;

fn main(_startup_arg: u32) {
    let mut object = [0u8; entropy_authority::MAX_BYTES];
    let len = read_authority(&mut object);
    let table =
        EntropyAuthority::decode(&object[..len]).unwrap_or_else(|_| fail(b"authority decode"));
    let mut holders: [Option<Holder>; entropy_authority::MAX_HOLDERS] =
        core::array::from_fn(|_| None);
    let count = table.holder_count();
    let mut hardware = 0u64;
    for (index, slot) in holders.iter_mut().enumerate().take(count) {
        let row = table
            .holder(index)
            .unwrap_or_else(|| fail(b"authority row"));
        if row.source == Source::Hardware {
            hardware += 1;
        }
        *slot = Some(Holder::declared(&row));
    }
    Line::new(b"[entropy-service] authority holders=")
        .number(count as u64)
        .push(b" hardware=")
        .number(hardware)
        .push(b" seeded=")
        .number(count as u64 - hardware)
        .emit();

    let source = resolve_binding(b"entropy-source").unwrap_or_else(|_| fail(b"source binding"));
    let mut source_ready = None;
    for holder in holders.iter_mut().flatten() {
        holder.endpoint = holder_binding(holder.name());
        match holder.source {
            Source::Seeded => {
                let seed = holder.seed;
                holder.drbg = Some(instantiate(&seed, &[], holder.name()));
            }
            Source::Hardware => {
                let mut fill = [0u8; ENTROPY_BYTES + NONCE_BYTES];
                let ready = request_fill(source, &mut fill);
                match source_ready {
                    None if ready => {
                        Line::new(b"[entropy-service] source=virtio-rng ready reseed_interval=")
                            .number(u64::from(RESEED_INTERVAL))
                            .emit();
                    }
                    None => {
                        debug_write(b"[entropy-service] source=virtio-rng unavailable\n");
                    }
                    Some(previous) if previous != ready => fail(b"source changed availability"),
                    Some(_) => {}
                }
                source_ready = Some(ready);
                if ready {
                    let (entropy, nonce) = fill.split_at(ENTROPY_BYTES);
                    holder.drbg = Some(instantiate(entropy, nonce, holder.name()));
                }
                fill.fill(0);
            }
        }
    }

    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    while holders.iter().flatten().any(|holder| holder.open) {
        let mut served = false;
        for holder in holders.iter_mut().flatten() {
            let received = slime_rt::recv(holder.endpoint, &mut message, &mut caps);
            if received == ERR_WOULDBLOCK {
                continue;
            }
            if received < 0 {
                fail(b"holder receive");
            }
            served = true;
            let request = (received as usize == entropy_service::REQUEST_LEN)
                .then(|| {
                    WireEntropyServiceRequest::decode(&message[..entropy_service::REQUEST_LEN])
                })
                .flatten();
            let mut bytes = [0u8; MAX_DRAW];
            let (status, length) = holder.serve(request, &mut bytes);
            answer(status, &bytes[..length]);
            bytes.fill(0);
            if holder.reseed_due() {
                let mut fill = [0u8; ENTROPY_BYTES];
                let reseeded = request_fill(source, &mut fill)
                    && holder
                        .drbg
                        .as_mut()
                        .is_some_and(|drbg| drbg.reseed(&fill).is_ok());
                fill.fill(0);
                if reseeded {
                    holder.since_reseed = 0;
                } else {
                    // A generator that cannot be refreshed from the device is
                    // retired; the holder is answered `unavailable` from here.
                    holder.drbg = None;
                }
            }
        }
        if !served {
            yield_now();
        }
    }
    release(source);
    exit(0);
}

/// One declared holder's session: its budget, its generator, and its endpoint.
struct Holder {
    name: [u8; NAME_BYTES],
    name_len: usize,
    source: Source,
    budget: u32,
    seed: [u8; 32],
    endpoint: u32,
    drawn: u32,
    since_reseed: u32,
    drbg: Option<HmacDrbg>,
    open: bool,
}

impl Holder {
    fn declared(row: &entropy_authority::Holder) -> Self {
        let declared = row.name_bytes();
        let mut name = [0u8; NAME_BYTES];
        name[..declared.len()].copy_from_slice(declared);
        Self {
            name,
            name_len: declared.len(),
            source: row.source,
            budget: row.byte_budget,
            seed: row.seed,
            endpoint: 0,
            drawn: 0,
            since_reseed: 0,
            drbg: None,
            open: true,
        }
    }

    fn name(&self) -> &[u8] {
        &self.name[..self.name_len]
    }

    /// Answer one request, returning its status and how many of `out`'s
    /// leading bytes it carries. A refused draw charges nothing.
    fn serve(
        &mut self,
        request: Option<WireEntropyServiceRequest>,
        out: &mut [u8],
    ) -> (i32, usize) {
        let Some(request) = request.filter(valid_entropy_service_request) else {
            return (STATUS_MALFORMED, 0);
        };
        if !self.open {
            return (STATUS_DENIED, 0);
        }
        match request.op {
            OP_CLOSE => {
                self.open = false;
                self.drbg = None;
                (STATUS_OK, 0)
            }
            OP_DRAW => {
                let length = request.length as usize;
                let Some(drbg) = self.drbg.as_mut() else {
                    return (STATUS_UNAVAILABLE, 0);
                };
                if self
                    .drawn
                    .checked_add(request.length)
                    .is_none_or(|total| total > self.budget)
                {
                    return (STATUS_EXHAUSTED, 0);
                }
                if drbg.generate(&mut out[..length]).is_err() {
                    fail(b"generate");
                }
                self.drawn += request.length;
                if self.source == Source::Hardware {
                    self.since_reseed += 1;
                }
                (STATUS_OK, length)
            }
            _ => (STATUS_MALFORMED, 0),
        }
    }

    fn reseed_due(&self) -> bool {
        self.open
            && self.source == Source::Hardware
            && self.drbg.is_some()
            && self.since_reseed >= RESEED_INTERVAL
    }
}

/// Page the authority table in through the root's identity-gated read and
/// return the length of the assembled object.
fn read_authority(object: &mut [u8; entropy_authority::MAX_BYTES]) -> usize {
    let mut rows = 0usize;
    loop {
        let mut page = [0u8; PAGE_ROWS * entropy_authority::ENTRY_BYTES];
        let read =
            entropy_authority_read(rows, &mut page).unwrap_or_else(|_| fail(b"authority read"));
        if read == 0 {
            break;
        }
        if read > PAGE_ROWS || rows + read > entropy_authority::MAX_HOLDERS {
            fail(b"authority page");
        }
        let bytes = read * entropy_authority::ENTRY_BYTES;
        let start = entropy_authority::HEADER_BYTES + rows * entropy_authority::ENTRY_BYTES;
        object[start..start + bytes].copy_from_slice(&page[..bytes]);
        rows += read;
    }
    let header = entropy_authority::header(rows).unwrap_or_else(|| fail(b"authority header"));
    object[..entropy_authority::HEADER_BYTES].copy_from_slice(&header);
    entropy_authority::HEADER_BYTES + rows * entropy_authority::ENTRY_BYTES
}

fn holder_binding(name: &[u8]) -> u32 {
    let mut binding = [0u8; NAME_BYTES + BINDING_SUFFIX.len()];
    binding[..name.len()].copy_from_slice(name);
    binding[name.len()..name.len() + BINDING_SUFFIX.len()].copy_from_slice(BINDING_SUFFIX);
    resolve_binding(&binding[..name.len() + BINDING_SUFFIX.len()])
        .unwrap_or_else(|_| fail(b"holder binding"))
}

fn instantiate(entropy: &[u8], nonce: &[u8], name: &[u8]) -> HmacDrbg {
    let mut personalization = [0u8; PERSONALIZATION_PREFIX.len() + NAME_BYTES];
    let len = PERSONALIZATION_PREFIX.len() + name.len();
    personalization[..PERSONALIZATION_PREFIX.len()].copy_from_slice(PERSONALIZATION_PREFIX);
    personalization[PERSONALIZATION_PREFIX.len()..len].copy_from_slice(name);
    HmacDrbg::instantiate(entropy, nonce, &personalization[..len])
        .unwrap_or_else(|_| fail(b"instantiate"))
}

/// Fill `out` from the device. `false` means the driver has no device; any
/// other refusal or a reply that is not exactly the requested bytes is fatal.
fn request_fill(source: u32, out: &mut [u8]) -> bool {
    let reply = source_call(source, OP_FILL, out.len());
    match reply.status {
        entropy_source::STATUS_OK if reply.length as usize == out.len() => {
            out.copy_from_slice(&reply.bytes[..out.len()]);
            true
        }
        entropy_source::STATUS_UNAVAILABLE => false,
        _ => fail(b"source fill"),
    }
}

fn release(source: u32) {
    if source_call(source, OP_RELEASE, 0).status != entropy_source::STATUS_OK {
        fail(b"source release");
    }
}

fn source_call(source: u32, op: u32, length: usize) -> WireEntropySourceReply {
    if length > MAX_FILL {
        fail(b"source request");
    }
    let request = WireEntropySourceRequest {
        magic: ENTROPY_SOURCE_MAGIC,
        version: entropy_source::FORMAT_VERSION,
        op,
        length: length as u32,
    };
    let mut answer = [0u8; MAX_MSG];
    let received = slime_rt::call(source, &request.encode(), &mut answer);
    if received < 0 || received as usize != entropy_source::REPLY_LEN {
        fail(b"source call");
    }
    WireEntropySourceReply::decode(&answer[..entropy_source::REPLY_LEN])
        .filter(valid_entropy_source_reply)
        .unwrap_or_else(|| fail(b"source reply"))
}

fn answer(status: i32, bytes: &[u8]) {
    let mut reply = WireEntropyServiceReply {
        magic: ENTROPY_SERVICE_MAGIC,
        version: entropy_service::FORMAT_VERSION,
        status,
        length: bytes.len() as u32,
        bytes: [0; MAX_DRAW],
    };
    reply.bytes[..bytes.len()].copy_from_slice(bytes);
    let _ = slime_rt::reply(&reply.encode());
}

/// One serial line, written with a single `debug_write` so another
/// component's output cannot split it.
struct Line {
    bytes: [u8; 96],
    used: usize,
}

impl Line {
    fn new(prefix: &[u8]) -> Self {
        let mut line = Self {
            bytes: [0; 96],
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

    fn emit(&mut self) {
        self.push(b"\n");
        debug_write(&self.bytes[..self.used]);
    }
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[entropy-service] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
