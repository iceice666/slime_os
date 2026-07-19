use crate::capability::{KernelObject, RIGHT_BLOCK_READ, RIGHT_RECV, RIGHT_SEND, RIGHT_TRANSFER};
use crate::ipc::{self, MAX_CAPS_PER_MSG, MAX_MSG};
use crate::task::{self, TermReason};
use crate::trap::UserFrame;

pub const SYS_YIELD: u64 = 0;
pub const SYS_SEND: u64 = 1;
pub const SYS_RECV: u64 = 2;
pub const SYS_EXIT: u64 = 3;
pub const SYS_SPAWN: u64 = 4;
pub const SYS_DEBUG_WRITE: u64 = 5;
pub const SYS_BLOCK_TRANSACT: u64 = 6;

const USER_TOP: u64 = 0x0000_8000_0000_0000;

fn user_range(addr: u64, len: usize) -> bool {
    let Some(end) = addr.checked_add(len as u64) else {
        return false;
    };
    addr < USER_TOP && end <= USER_TOP
}

fn current_user_range(addr: u64, len: usize, writable: bool) -> bool {
    user_range(addr, len)
        && task::with_current_mut(|task| task.address_space.user_range_mapped(addr, len, writable))
}

pub fn dispatch(frame: &mut UserFrame) {
    match frame.rax {
        SYS_YIELD => task::yield_now(frame),
        SYS_SEND => sys_send(frame),
        SYS_RECV => sys_recv(frame),
        SYS_EXIT => {
            let status = frame.rdi as i64;
            task::terminate(frame, TermReason::Exit(status));
        }
        SYS_SPAWN => sys_spawn(frame),
        SYS_DEBUG_WRITE => sys_debug_write(frame),
        SYS_BLOCK_TRANSACT => sys_block_transact(frame),
        _ => frame.rax = ipc::ERR_INVALID_ARG as u64,
    }
}

fn sys_send(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let buf = frame.rsi as *const u8;
    let len = (frame.rdx as usize).min(MAX_MSG);
    let cap_count = frame.r10 as usize;
    let cap_handles = frame.r8 as *const u32;

    if cap_count > MAX_CAPS_PER_MSG
        || !current_user_range(frame.rsi, len, false)
        || (cap_count > 0
            && !current_user_range(frame.r8, cap_count * core::mem::size_of::<u32>(), false))
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    // SAFETY: the current task's complete user range was validated as mapped.
    let bytes = unsafe { core::slice::from_raw_parts(buf, len) };
    let mut payload = [0u8; MAX_MSG];
    payload[..len].copy_from_slice(bytes);

    let mut handles = [0u32; MAX_CAPS_PER_MSG];
    if cap_count > 0 {
        // SAFETY: the current task's complete user range was validated as mapped.
        let src = unsafe { core::slice::from_raw_parts(cap_handles, cap_count) };
        handles[..cap_count].copy_from_slice(src);
    }

    let ret = task::with_current_mut(|task| {
        let Some(cap) = task.caps.get(slot) else {
            return ipc::ERR_BAD_CAP;
        };
        if cap.rights & RIGHT_SEND == 0 {
            return ipc::ERR_BAD_CAP;
        }
        let KernelObject::Endpoint(endpoint) = &cap.object else {
            return ipc::ERR_BAD_CAP;
        };
        let endpoint = endpoint.clone();
        let mut moved_caps = core::array::from_fn(|_| None);

        for i in 0..cap_count {
            let handle = handles[i];
            if handles[..i].contains(&handle) {
                return ipc::ERR_BAD_CAP;
            }
            let Some(candidate) = task.caps.get(handle) else {
                return ipc::ERR_BAD_CAP;
            };
            if candidate.rights & RIGHT_TRANSFER == 0 {
                return ipc::ERR_BAD_CAP;
            }
        }
        for i in 0..cap_count {
            moved_caps[i] = task.caps.take(handles[i]);
        }

        let result = ipc::send(&endpoint, &payload[..len], &mut moved_caps);
        if result != ipc::ERR_SUCCESS {
            for (i, cap) in moved_caps.iter_mut().enumerate().take(cap_count) {
                if let Some(cap) = cap.take() {
                    task.caps
                        .put(handles[i], cap)
                        .expect("transferred capability slot changed during send");
                }
            }
        }
        result
    });

    frame.rax = ret as u64;
}

fn sys_block_transact(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let request_address = frame.rsi;
    let reply_address = frame.rdx;
    if !current_user_range(request_address, crate::block_proto::REQUEST_LEN, false)
        || !current_user_range(reply_address, crate::block_proto::REPLY_LEN, true)
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let authorized = task::with_current_mut(|task| {
        task.caps.get(slot).is_some_and(|cap| {
            cap.rights & RIGHT_BLOCK_READ != 0 && matches!(cap.object, KernelObject::BlockDevice)
        })
    });
    if !authorized {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }

    let mut request = [0u8; crate::block_proto::REQUEST_LEN];
    unsafe {
        core::ptr::copy_nonoverlapping(
            request_address as *const u8,
            request.as_mut_ptr(),
            request.len(),
        )
    };
    let decoded = match crate::block_proto::decode_request(&request) {
        Ok(decoded) => decoded,
        Err(_) => {
            let mut reply = [0u8; crate::block_proto::REPLY_LEN];
            crate::block_service::transact(&request, &mut reply);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    reply.as_ptr(),
                    reply_address as *mut u8,
                    reply.len(),
                )
            };
            frame.rax = ipc::ERR_SUCCESS as u64;
            return;
        }
    };
    let payload_len = decoded.sector_count as usize * crate::block_proto::SECTOR_SIZE;
    if decoded.op == crate::block_proto::OP_READ
        && !current_user_range(decoded.buffer_phys, payload_len, true)
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    let mut reply = [0u8; crate::block_proto::REPLY_LEN];
    crate::block_service::transact(&request, &mut reply);
    unsafe {
        core::ptr::copy_nonoverlapping(reply.as_ptr(), reply_address as *mut u8, reply.len())
    };
    frame.rax = ipc::ERR_SUCCESS as u64;
}

fn sys_recv(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let buf = frame.rsi as *mut u8;
    let cap_out = frame.rdx as *mut u64;

    if !current_user_range(frame.rsi, MAX_MSG, true)
        || !current_user_range(
            frame.rdx,
            MAX_CAPS_PER_MSG * core::mem::size_of::<u64>(),
            true,
        )
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    let mut kbuf = [0u8; MAX_MSG];
    let mut cap_handles = [0u64; MAX_CAPS_PER_MSG];
    let ret = task::with_current_mut(|task| {
        let Some(cap) = task.caps.get(slot) else {
            return ipc::ERR_BAD_CAP;
        };
        if cap.rights & RIGHT_RECV == 0 {
            return ipc::ERR_BAD_CAP;
        }
        let KernelObject::Endpoint(endpoint) = &cap.object else {
            return ipc::ERR_BAD_CAP;
        };
        let endpoint = endpoint.clone();
        ipc::recv(&endpoint, &mut kbuf, &mut cap_handles, &mut task.caps)
    });

    if ret >= 0 {
        let len = ret as usize;
        // SAFETY: the current task's complete writable user ranges were validated.
        unsafe {
            core::ptr::copy_nonoverlapping(kbuf.as_ptr(), buf, len);
            core::ptr::copy_nonoverlapping(cap_handles.as_ptr(), cap_out, MAX_CAPS_PER_MSG);
        }
    }
    frame.rax = ret as u64;
}

fn sys_spawn(frame: &mut UserFrame) {
    let executable_slot = frame.rdi as u32;
    let cap_count = frame.rdx as usize;
    let component_name = frame.r10;
    if cap_count > crate::capability::MAX_CAPS
        || (cap_count > 0
            && !current_user_range(frame.rsi, cap_count * core::mem::size_of::<u32>(), false))
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let mut slot_buffer = [0u32; crate::capability::MAX_CAPS];
    let slot_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            slot_buffer.as_mut_ptr().cast::<u8>(),
            cap_count * core::mem::size_of::<u32>(),
        )
    };
    if !task::copy_from_current(frame.rsi, slot_bytes) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let slots = &slot_buffer[..cap_count];
    match task::spawn_from_cap(executable_slot, slots) {
        Ok(id) => {
            if let Some(name) = component_name_from_id(component_name) {
                crate::bootstrap::record_spawn(name, id);
            }
            frame.rax = id;
        }
        Err(_) => frame.rax = ipc::ERR_BAD_CAP as u64,
    }
}

fn component_name_from_id(id: u64) -> Option<&'static str> {
    match id {
        1 => Some("console"),
        2 => Some("dango"),
        3 => Some("sysinfo"),
        4 => Some("echo-agent"),
        5 => Some("storage-probe"),
        _ => None,
    }
}

fn sys_debug_write(frame: &mut UserFrame) {
    let buf = frame.rdi as *const u8;
    let len = frame.rsi as usize;
    if !current_user_range(frame.rdi, len, false) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    // SAFETY: the current task's complete user range was validated as mapped.
    let bytes = unsafe { core::slice::from_raw_parts(buf, len) };
    crate::serial::write_bytes(bytes);
    frame.rax = len as u64;
}
