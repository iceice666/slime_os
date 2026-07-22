#![no_std]
#![no_main]

use slime_proto::{
    powerbox::{self, WirePowerboxReply, WirePowerboxRequest},
    valid_powerbox_reply,
};
use slime_rt::{ERR_BAD_CAP, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const RIGHT_TRANSFER: u32 = 1 << 2;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
const RIGHT_DIRECTORY_WRITE: u32 = 1 << 20;

fn main() {
    let mut root = [0u8; 32];
    let mut scope = [0u8; slime_rt::MAX_DIRECTORY_PATH];
    if slime_rt::directory_inspect(RPC_SLOT, RIGHT_DIRECTORY_READ, &mut root, &mut scope)
        != Err(ERR_BAD_CAP)
        || slime_rt::directory_derive(RPC_SLOT, b"note", RIGHT_DIRECTORY_READ) != Err(ERR_BAD_CAP)
    {
        fail();
    }
    slime_rt::debug_write(b"[powerbox-probe] manifest directory absent\n");

    let selected = call(request(
        RIGHT_DIRECTORY_READ | RIGHT_TRANSFER,
        b"Open the selected note",
    ));
    if selected.reply.status != 0
        || selected.reply.flags != powerbox::REPLY_FLAG_SELECTED
        || selected.reply.granted_rights != RIGHT_DIRECTORY_READ | RIGHT_TRANSFER
        || selected.reply.event_id == 0
        || selected.cap_count != 1
    {
        fail();
    }
    let selected_slot = selected.capability.unwrap_or_else(|| fail());
    let scope_len =
        slime_rt::directory_inspect(selected_slot, RIGHT_DIRECTORY_READ, &mut root, &mut scope)
            .unwrap_or_else(|_| fail());
    if &scope[..scope_len] != b"note"
        || slime_rt::directory_inspect(selected_slot, RIGHT_DIRECTORY_WRITE, &mut root, &mut scope)
            != Err(ERR_BAD_CAP)
    {
        fail();
    }
    slime_rt::debug_write(b"[powerbox-probe] selected single object received\n");
    if slime_rt::cap_drop(selected_slot) != 0 {
        fail();
    }

    let denied = call(request(
        RIGHT_DIRECTORY_READ | RIGHT_DIRECTORY_WRITE | RIGHT_TRANSFER,
        b"Attempt rights widening",
    ));
    if denied.reply.status >= 0 || denied.reply.flags != 0 || denied.cap_count != 0 {
        fail();
    }
    slime_rt::debug_write(b"[powerbox-probe] derive closure enforced\n");

    let cancelled = call(request(RIGHT_DIRECTORY_READ, b"Cancel this selection"));
    if cancelled.reply.status != 0
        || cancelled.reply.flags != powerbox::REPLY_FLAG_CANCELLED
        || cancelled.reply.granted_rights != 0
        || cancelled.reply.event_id != 0
        || cancelled.cap_count != 0
    {
        fail();
    }
    slime_rt::debug_write(b"[powerbox-probe] cancellation minted nothing\n");
    slime_rt::debug_write(b"[powerbox-probe] done\n");
}

struct Response {
    reply: WirePowerboxReply,
    capability: Option<u32>,
    cap_count: usize,
}

fn request(rights: u32, purpose: &[u8]) -> WirePowerboxRequest {
    let mut encoded_purpose = [0u8; powerbox::MAX_PURPOSE_BYTES];
    encoded_purpose[..purpose.len()].copy_from_slice(purpose);
    WirePowerboxRequest {
        magic: powerbox::POWERBOX_MAGIC,
        version: powerbox::FORMAT_VERSION,
        object_kind: powerbox::OBJECT_KIND_FILE,
        reserved0: 0,
        purpose_len: purpose.len() as u16,
        requested_rights: rights,
        purpose: encoded_purpose,
        reserved: [0; 8],
    }
}

fn call(request: WirePowerboxRequest) -> Response {
    let encoded = request.encode();
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, &[]) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(),
            _ => break,
        }
    }
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(RPC_SLOT, &mut message, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            result if result < 0 => fail(),
            length => {
                let reply = WirePowerboxReply::decode(&message[..length as usize])
                    .filter(valid_powerbox_reply)
                    .unwrap_or_else(|| fail());
                let cap_count = caps.iter().filter(|slot| **slot != 0).count();
                if cap_count > 1 || caps[1..].iter().any(|slot| *slot != 0) {
                    fail();
                }
                return Response {
                    reply,
                    capability: (caps[0] != 0).then_some(caps[0] as u32),
                    cap_count,
                };
            }
        }
    }
}

fn fail() -> ! {
    slime_rt::debug_write(b"[powerbox-probe] failed\n");
    slime_rt::exit(1)
}
