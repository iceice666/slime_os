use slime_proto::{spawn::WireSpawnRequest, valid_spawn_request};
use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

pub const CONTEXT_SLOT: u32 = 0;

pub fn receive() -> WireSpawnRequest {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(CONTEXT_SLOT, &mut message, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::yield_now(),
            n if n < 0 => slime_rt::exit(1),
            n => {
                if caps.iter().any(|slot| *slot != 0) {
                    slime_rt::exit(1);
                }
                let Some(request) = WireSpawnRequest::decode(&message[..n as usize]) else {
                    slime_rt::exit(1)
                };
                if request.flags != 0 || !valid_spawn_request(&request) {
                    slime_rt::exit(1);
                }
                return request;
            }
        }
    }
}

#[allow(dead_code)]
pub fn field(bytes: &[u8; 8], index: usize) -> Option<&[u8]> {
    let mut offset = 0;
    for current in 0..=index {
        let length = *bytes.get(offset)? as usize;
        let value = bytes.get(offset + 1..offset + 1 + length)?;
        if current == index {
            return Some(value);
        }
        offset += 1 + length;
    }
    None
}

pub fn debug_decimal(mut value: usize) {
    let mut digits = [0u8; 20];
    let mut index = digits.len();
    loop {
        index -= 1;
        digits[index] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    slime_rt::debug_write(&digits[index..]);
}
