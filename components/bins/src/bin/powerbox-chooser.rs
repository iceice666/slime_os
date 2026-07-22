#![no_std]
#![no_main]

use slime_proto::{
    powerbox::{self, WirePowerboxReply, WirePowerboxRequest},
    valid_powerbox_request,
};
use slime_rt::{ERR_PEER_DEAD, ERR_WOULDBLOCK, InputKey, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

const RPC_SLOT: u32 = 0;
const DIRECTORY_SLOT: u32 = 1;
const INPUT_SLOT: u32 = 2;
const RIGHT_TRANSFER: u32 = 1 << 2;
const RIGHT_DIRECTORY_READ: u32 = 1 << 19;
const ALLOWED_FILE_RIGHTS: u32 = RIGHT_DIRECTORY_READ | RIGHT_TRANSFER;
const SELECTED_PATH: &[u8] = b"note";

fn main() {
    slime_rt::debug_write(b"[powerbox] chooser ready\n");
    let mut event_id = 1u64;
    for _ in 0..3 {
        let mut message = [0u8; MAX_MSG];
        let mut received_caps = [0u64; MAX_CAPS_PER_MSG];
        let length = loop {
            match slime_rt::recv(RPC_SLOT, &mut message, &mut received_caps) {
                ERR_WOULDBLOCK => slime_rt::yield_now(),
                ERR_PEER_DEAD => return,
                result if result < 0 => slime_rt::exit(1),
                result => break result as usize,
            }
        };
        if received_caps.iter().any(|slot| *slot != 0) {
            release_caps(&received_caps);
            send_reply(failure(-1), None);
            continue;
        }
        let Some(request) = WirePowerboxRequest::decode(&message[..length]) else {
            send_reply(failure(-1), None);
            continue;
        };
        if !valid_powerbox_request(&request)
            || request.requested_rights & !ALLOWED_FILE_RIGHTS != 0
            || request.requested_rights & RIGHT_DIRECTORY_READ == 0
        {
            slime_rt::debug_write(b"[powerbox] derive closure denied\n");
            send_reply(failure(-2), None);
            continue;
        }
        render_prompt(&request);
        match wait_gesture() {
            Gesture::Cancel => {
                slime_rt::debug_write(b"[powerbox] selection cancelled\n");
                send_reply(cancelled(&request), None);
            }
            Gesture::Select => {
                let slot = slime_rt::directory_derive(
                    DIRECTORY_SLOT,
                    SELECTED_PATH,
                    request.requested_rights,
                )
                .unwrap_or_else(|_| slime_rt::exit(1));
                record_provenance(event_id, &request);
                send_reply(selected(event_id, &request), Some(slot));
                event_id += 1;
            }
        }
    }
    slime_rt::debug_write(b"[powerbox] chooser complete\n");
}

enum Gesture {
    Select,
    Cancel,
}

fn wait_gesture() -> Gesture {
    loop {
        match slime_rt::input_read(INPUT_SLOT) {
            Ok(None) => slime_rt::yield_now(),
            Err(_) => slime_rt::exit(1),
            Ok(Some(event)) if !event.pressed => {}
            Ok(Some(event)) => match event.key {
                InputKey::Enter => return Gesture::Select,
                InputKey::Escape => return Gesture::Cancel,
                _ => {}
            },
        }
    }
}

fn render_prompt(request: &WirePowerboxRequest) {
    slime_rt::debug_write(b"[powerbox] request kind=file purpose=");
    slime_rt::debug_write(&request.purpose[..request.purpose_len as usize]);
    slime_rt::debug_write(b" [Enter=select Escape=cancel]\n");
}

fn record_provenance(event_id: u64, request: &WirePowerboxRequest) {
    slime_rt::debug_write(b"[powerbox-provenance] event=");
    write_u64(event_id);
    slime_rt::debug_write(b" gesture=select kind=file path=note rights=");
    write_hex_u32(request.requested_rights);
    slime_rt::debug_write(b" purpose=");
    slime_rt::debug_write(&request.purpose[..request.purpose_len as usize]);
    slime_rt::debug_write(b"\n");
}

fn selected(event_id: u64, request: &WirePowerboxRequest) -> WirePowerboxReply {
    let mut selected_path = [0u8; 16];
    selected_path[..SELECTED_PATH.len()].copy_from_slice(SELECTED_PATH);
    reply(
        0,
        powerbox::REPLY_FLAG_SELECTED,
        request.requested_rights,
        event_id,
        selected_path,
        request,
    )
}

fn cancelled(request: &WirePowerboxRequest) -> WirePowerboxReply {
    reply(0, powerbox::REPLY_FLAG_CANCELLED, 0, 0, [0; 16], request)
}

fn failure(status: i32) -> WirePowerboxReply {
    WirePowerboxReply {
        magic: powerbox::POWERBOX_MAGIC,
        version: powerbox::FORMAT_VERSION,
        status,
        flags: 0,
        object_kind: powerbox::OBJECT_KIND_FILE,
        purpose_len: 0,
        reserved0: 0,
        granted_rights: 0,
        event_id: 0,
        selected_path: [0; 16],
        purpose: [0; 16],
    }
}

fn reply(
    status: i32,
    flags: u8,
    granted_rights: u32,
    event_id: u64,
    selected_path: [u8; 16],
    request: &WirePowerboxRequest,
) -> WirePowerboxReply {
    let purpose_len = (request.purpose_len as usize).min(16);
    let mut purpose = [0u8; 16];
    purpose[..purpose_len].copy_from_slice(&request.purpose[..purpose_len]);
    WirePowerboxReply {
        magic: powerbox::POWERBOX_MAGIC,
        version: powerbox::FORMAT_VERSION,
        status,
        flags,
        object_kind: request.object_kind,
        purpose_len: purpose_len as u16,
        reserved0: 0,
        granted_rights,
        event_id,
        selected_path,
        purpose,
    }
}

fn send_reply(reply: WirePowerboxReply, capability: Option<u32>) {
    let encoded = reply.encode();
    let slots = capability.map_or([0; 1], |slot| [slot]);
    let caps = if capability.is_some() {
        &slots[..]
    } else {
        &slots[..0]
    };
    loop {
        match slime_rt::send(RPC_SLOT, &encoded, caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            ERR_PEER_DEAD => {
                drop_capability(capability);
                return;
            }
            result if result < 0 => {
                drop_capability(capability);
                slime_rt::exit(1);
            }
            _ => return,
        }
    }
}

fn drop_capability(capability: Option<u32>) {
    if let Some(slot) = capability
        && slime_rt::cap_drop(slot) != 0
    {
        slime_rt::exit(1);
    }
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().copied().filter(|slot| *slot != 0) {
        if slime_rt::cap_drop(slot as u32) != 0 {
            slime_rt::exit(1);
        }
    }
}

fn write_hex_u32(value: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut buffer = [0u8; 10];
    buffer[..2].copy_from_slice(b"0x");
    for index in 0..8 {
        buffer[index + 2] = HEX[((value >> ((7 - index) * 4)) & 0xf) as usize];
    }
    slime_rt::debug_write(&buffer);
}

fn write_u64(mut value: u64) {
    let mut buffer = [0u8; 20];
    let mut cursor = buffer.len();
    if value == 0 {
        slime_rt::debug_write(b"0");
        return;
    }
    while value != 0 {
        cursor -= 1;
        buffer[cursor] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    slime_rt::debug_write(&buffer[cursor..]);
}
