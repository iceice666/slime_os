use slime_proto::{
    spawn::{REQUEST_LEN, WireSpawnFrame, WireSpawnRequest},
    spawn_arguments::SpawnArguments,
    valid_spawn_request,
};
use slime_rt::{MAX_CAPS_PER_MSG, MAX_MSG};

pub const CONTEXT_SLOT: u32 = 0;

pub struct LaunchContext {
    pub request: WireSpawnRequest,
    // Included by path into each consumer; a consumer that takes no arguments
    // still validates them before it runs.
    #[allow(dead_code)]
    pub arguments: SpawnArguments,
}

impl core::ops::Deref for LaunchContext {
    type Target = WireSpawnRequest;
    fn deref(&self) -> &Self::Target {
        &self.request
    }
}

pub fn receive() -> LaunchContext {
    receive_from(CONTEXT_SLOT).unwrap_or_else(|_| slime_rt::exit(1))
}

/// Read only the declared launch-context endpoint. Capabilities belong to the
/// child launch grants, never to an argument continuation.
pub fn receive_from(slot: u32) -> Result<LaunchContext, ()> {
    let mut message = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    let n = slime_rt::recv_blocking(slot, &mut message, &mut caps);
    if n != REQUEST_LEN as i64 || caps.iter().any(|slot| *slot != 0) {
        return Err(());
    }
    let request = WireSpawnRequest::decode(&message).ok_or(())?;
    if request.flags != 0 || !valid_spawn_request(&request) {
        return Err(());
    }
    let mut arguments = SpawnArguments::new(&request).ok_or(())?;
    while !arguments.is_complete() {
        let n = slime_rt::recv_blocking(slot, &mut message, &mut caps);
        if n != REQUEST_LEN as i64 || caps.iter().any(|slot| *slot != 0) {
            return Err(());
        }
        let frame = WireSpawnFrame::decode(&message).ok_or(())?;
        arguments.push(&frame).map_err(|_| ())?;
    }
    Ok(LaunchContext { request, arguments })
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
