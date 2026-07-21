use crate::capability::{
    Capability, KernelObject, RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE,
    RIGHT_ENDPOINT_CREATE, RIGHT_HEALTH_CONFIRM, RIGHT_RECV, RIGHT_SEND, RIGHT_STORE_READ,
    RIGHT_STORE_WRITE, RIGHT_TRANSFER,
};
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
pub const SYS_STORE_TRANSACT: u64 = 7;
pub const SYS_HEALTH_CONFIRM: u64 = 8;
pub const SYS_UNHEALTHY: u64 = 9;
pub const SYS_RECOVERY_RECONSTRUCT: u64 = 10;
pub const SYS_ENDPOINT_CREATE: u64 = 11;
pub const SYS_SUPERVISION_STATUS: u64 = 12;
pub const SYS_CAP_DROP: u64 = 13;

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
        SYS_STORE_TRANSACT => sys_store_transact(frame),
        SYS_HEALTH_CONFIRM => sys_health_confirm(frame),
        SYS_UNHEALTHY => task::terminate(frame, TermReason::Unhealthy),
        SYS_RECOVERY_RECONSTRUCT => sys_recovery_reconstruct(frame),
        SYS_ENDPOINT_CREATE => sys_endpoint_create(frame),
        SYS_SUPERVISION_STATUS => sys_supervision_status(frame),
        SYS_CAP_DROP => sys_cap_drop(frame),
        _ => frame.rax = ipc::ERR_INVALID_ARG as u64,
    }
}

fn sys_send(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let buf = frame.rsi as *const u8;
    let len = (frame.rdx as usize).min(MAX_MSG);
    let cap_handles = frame.r10 as *const u32;
    let cap_count = frame.r8 as usize;

    if cap_count > MAX_CAPS_PER_MSG
        || !current_user_range(frame.rsi, len, false)
        || (cap_count > 0
            && !current_user_range(frame.r10, cap_count * core::mem::size_of::<u32>(), false))
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
    let capability = task::with_current_mut(|task| {
        task.caps.get(slot).and_then(|cap| match cap.object {
            KernelObject::BlockDevice(function) => Some((function, cap.rights)),
            _ => None,
        })
    });
    let Some((function, rights)) = capability else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };

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
            crate::block_service::transact(function, &request, &mut reply);
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
    let replay = decoded.flags == crate::block_proto::FLAG_REPLAY_LAST;
    let required_right = match decoded.op {
        crate::block_proto::OP_READ => RIGHT_BLOCK_READ,
        crate::block_proto::OP_WRITE | crate::block_proto::OP_FLUSH => RIGHT_BLOCK_WRITE,
        _ => 0,
    };
    if rights & required_right == 0 {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let payload_len = decoded.sector_count as usize * crate::block_proto::SECTOR_SIZE;
    let invalid_payload = if replay {
        false
    } else {
        match decoded.op {
            crate::block_proto::OP_READ => {
                !current_user_range(decoded.buffer_phys, payload_len, true)
            }
            crate::block_proto::OP_WRITE => {
                !current_user_range(decoded.buffer_phys, payload_len, false)
            }
            crate::block_proto::OP_FLUSH => false,
            _ => true,
        }
    };
    if invalid_payload {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    let mut reply = [0u8; crate::block_proto::REPLY_LEN];
    crate::block_service::transact(function, &request, &mut reply);
    unsafe {
        core::ptr::copy_nonoverlapping(reply.as_ptr(), reply_address as *mut u8, reply.len())
    };
    frame.rax = ipc::ERR_SUCCESS as u64;
}

fn sys_store_transact(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let request_address = frame.rsi;
    let reply_address = frame.rdx;
    if !current_user_range(request_address, crate::store_proto::REQUEST_LEN, false)
        || !current_user_range(reply_address, crate::store_proto::REPLY_LEN, true)
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let rights = task::with_current_mut(|task| {
        task.caps
            .get(slot)
            .and_then(|cap| matches!(cap.object, KernelObject::ObjectStore).then_some(cap.rights))
    });
    let Some(rights) = rights else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };

    let mut request = [0u8; crate::store_proto::REQUEST_LEN];
    unsafe {
        core::ptr::copy_nonoverlapping(
            request_address as *const u8,
            request.as_mut_ptr(),
            request.len(),
        )
    };
    let decoded = match crate::store_proto::decode_request(&request) {
        Ok(decoded) => decoded,
        Err(_) => {
            // Let the service encode the structured protocol error reply.
            let mut reply = [0u8; crate::store_proto::REPLY_LEN];
            crate::store_service::transact(&request, &mut reply);
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
    let required_right = match decoded.op {
        crate::store_proto::OP_STAT | crate::store_proto::OP_GET => RIGHT_STORE_READ,
        crate::store_proto::OP_PUT => RIGHT_STORE_WRITE,
        _ => 0,
    };
    if rights & required_right == 0 {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let payload_valid = match decoded.op {
        crate::store_proto::OP_STAT => true,
        crate::store_proto::OP_GET => {
            decoded.payload_len == 0
                || current_user_range(decoded.buffer_addr, decoded.payload_len as usize, true)
        }
        crate::store_proto::OP_PUT => {
            decoded.payload_len == 0
                || current_user_range(decoded.buffer_addr, decoded.payload_len as usize, false)
        }
        _ => false,
    };
    if !payload_valid {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    let mut reply = [0u8; crate::store_proto::REPLY_LEN];
    crate::store_service::transact(&request, &mut reply);
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

fn sys_health_confirm(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let authorized = task::with_current_mut(|task| {
        task.caps.get(slot).is_some_and(|cap| {
            matches!(cap.object, KernelObject::GenerationControl)
                && cap.rights & RIGHT_HEALTH_CONFIRM != 0
        })
    });
    if !authorized {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    frame.rax = if crate::generation_manager::confirm_running_pending() {
        ipc::ERR_SUCCESS as u64
    } else {
        ipc::ERR_INVALID_ARG as u64
    };
}

fn sys_recovery_reconstruct(frame: &mut UserFrame) {
    let generation_control_slot = frame.rdi as u32;
    let block_slot = frame.rsi as u32;
    let flags = frame.rdx as u32;
    let (control, block) = task::with_current_mut(|task| {
        let control = task.caps.get(generation_control_slot).is_some_and(|cap| {
            matches!(cap.object, KernelObject::GenerationControl)
                && cap.rights & RIGHT_BOOT_UPDATE != 0
        });
        let block = task.caps.get(block_slot).and_then(|cap| match cap.object {
            KernelObject::BlockDevice(function)
                if cap.rights & (RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE)
                    == RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE =>
            {
                Some(function)
            }
            _ => None,
        });
        (control, block)
    });
    let authorized = control.then_some(block).flatten();
    let Some(function) = authorized else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::recovery::reconstruct(function, flags) {
        Ok(result) => {
            crate::serial_println!(
                "[recovery] reconstructed generation={:02x?} state_root={:02x?}",
                result.generation,
                result.state_root,
            );
            ipc::ERR_SUCCESS as u64
        }
        Err(error) => {
            crate::serial_println!("[recovery] reconstruction rejected: {:?}", error);
            ipc::ERR_INVALID_ARG as u64
        }
    };
}

fn sys_spawn(frame: &mut UserFrame) {
    let executable_slot = frame.rdi as u32;
    let grant_count = frame.rdx as usize;
    if grant_count > crate::capability::MAX_CAPS
        || (grant_count > 0 && !current_user_range(frame.rsi, grant_count * 8, false))
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let mut grant_buffer = [task::SpawnGrant { slot: 0, rights: 0 }; crate::capability::MAX_CAPS];
    let grant_bytes = unsafe {
        core::slice::from_raw_parts_mut(
            grant_buffer.as_mut_ptr().cast::<u8>(),
            grant_count * core::mem::size_of::<task::SpawnGrant>(),
        )
    };
    if !task::copy_from_current(frame.rsi, grant_bytes) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    match task::spawn_from_cap(executable_slot, &grant_buffer[..grant_count]) {
        Ok((id, handle)) => {
            frame.rax = id;
            frame.rdx = handle as u64;
        }
        Err(task::SpawnError::TooManyTasks | task::SpawnError::BudgetExhausted) => {
            frame.rax = ipc::ERR_OUT_OF_MEMORY as u64
        }
        Err(_) => frame.rax = ipc::ERR_BAD_CAP as u64,
    }
}

fn sys_endpoint_create(frame: &mut UserFrame) {
    let factory_slot = frame.rdi as u32;
    let allowed = task::with_current_mut(|task| {
        task.caps.get(factory_slot).is_some_and(|cap| {
            matches!(cap.object, KernelObject::EndpointFactory)
                && cap.rights & RIGHT_ENDPOINT_CREATE != 0
        }) && task.caps.available_slots() >= 2
    });
    if !allowed {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let (a, b) = ipc::channel();
    let inserted = task::with_current_mut(|task| {
        let a_slot = task.caps.insert(Capability {
            object: KernelObject::Endpoint(a),
            rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        })?;
        let b_slot = match task.caps.insert(Capability {
            object: KernelObject::Endpoint(b),
            rights: RIGHT_SEND | RIGHT_RECV | RIGHT_TRANSFER,
        }) {
            Ok(slot) => slot,
            Err(error) => {
                let _ = task.caps.take(a_slot);
                return Err(error);
            }
        };
        Ok((a_slot, b_slot))
    });
    match inserted {
        Ok((a, b)) => {
            frame.rax = a as u64;
            frame.rdx = b as u64;
        }
        Err(_) => frame.rax = ipc::ERR_OUT_OF_MEMORY as u64,
    }
}

fn sys_supervision_status(frame: &mut UserFrame) {
    match task::supervision_status(frame.rdi as u32) {
        Ok(None) => frame.rax = ipc::ERR_WOULDBLOCK as u64,
        Ok(Some(TermReason::Exit(status))) => {
            frame.rax = 0;
            frame.rdx = status as u64;
        }
        Ok(Some(TermReason::Fault(reason))) => {
            frame.rax = 1;
            frame.rdx = reason_code(reason);
        }
        Ok(Some(TermReason::Timeout)) => frame.rax = 2,
        Ok(Some(TermReason::PeerLoss)) => frame.rax = 3,
        Ok(Some(TermReason::Unhealthy)) => frame.rax = 4,
        Err(_) => frame.rax = ipc::ERR_BAD_CAP as u64,
    }
}

fn sys_cap_drop(frame: &mut UserFrame) {
    frame.rax = if task::with_current_mut(|task| task.caps.remove(frame.rdi as u32)).is_ok() {
        ipc::ERR_SUCCESS as u64
    } else {
        ipc::ERR_BAD_CAP as u64
    };
}

fn reason_code(reason: task::UserFaultReason) -> u64 {
    match reason {
        task::UserFaultReason::DivByZero => 1,
        task::UserFaultReason::UndefinedOp => 2,
        task::UserFaultReason::GeneralProt => 3,
        task::UserFaultReason::PageFault => 4,
        task::UserFaultReason::Unknown(vector) => 0x100 | vector as u64,
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
