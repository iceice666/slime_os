use crate::capability::{
    Capability, KernelObject, RIGHT_BLOCK_READ, RIGHT_BLOCK_WRITE, RIGHT_BOOT_UPDATE,
    RIGHT_BUFFER_CREATE, RIGHT_BUFFER_LOAN, RIGHT_BUFFER_MAP, RIGHT_BUFFER_WRITE,
    RIGHT_DIRECTORY_DERIVE, RIGHT_DIRECTORY_LIST, RIGHT_DIRECTORY_READ, RIGHT_DIRECTORY_WRITE,
    RIGHT_ENDPOINT_CREATE, RIGHT_HEALTH_CONFIRM, RIGHT_INPUT_READ, RIGHT_RECV, RIGHT_SEND,
    RIGHT_STORE_READ, RIGHT_STORE_WRITE, RIGHT_SUPERVISE, RIGHT_TRANSFER,
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
pub const SYS_DIRECTORY_INSPECT: u64 = 14;
pub const SYS_DIRECTORY_DERIVE: u64 = 15;
pub const SYS_DIRECTORY_COMMIT: u64 = 16;
pub const SYS_INPUT_READ: u64 = 17;
pub const SYS_GENERATION_TRANSACT: u64 = 18;
pub const SYS_GENERATION_RECEIVE: u64 = 19;
pub const SYS_WAIT: u64 = 20;
pub const SYS_SHARED_BUFFER_CREATE: u64 = 21;
pub const SYS_SHARED_BUFFER_RELEASE: u64 = 22;
pub const SYS_SHARED_BUFFER_MAP: u64 = 23;
pub const SYS_SHARED_BUFFER_UNMAP: u64 = 24;
pub const SYS_SHARED_BUFFER_SEAL: u64 = 25;
pub const SYS_SHARED_BUFFER_LOAN: u64 = 26;
pub const SYS_SHARED_BUFFER_LOAN_MAP: u64 = 27;
pub const SYS_SHARED_BUFFER_RETURN: u64 = 28;
pub const SYS_SHARED_BUFFER_REVOKE: u64 = 29;
/// C8.3 bounded narrow-on-transfer move. Distinct from `SYS_SEND`'s cap
/// attachment, which moves a capability at its full held rights.
pub const SYS_CAP_TRANSFER: u64 = 30;

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
        SYS_DIRECTORY_INSPECT => sys_directory_inspect(frame),
        SYS_DIRECTORY_DERIVE => sys_directory_derive(frame),
        SYS_DIRECTORY_COMMIT => sys_directory_commit(frame),
        SYS_INPUT_READ => sys_input_read(frame),
        SYS_GENERATION_TRANSACT => sys_generation_transact(frame),
        SYS_GENERATION_RECEIVE => sys_generation_receive(frame),
        SYS_WAIT => sys_wait(frame),
        SYS_SHARED_BUFFER_CREATE => sys_shared_buffer_create(frame),
        SYS_SHARED_BUFFER_RELEASE => sys_shared_buffer_release(frame),
        SYS_SHARED_BUFFER_MAP => sys_shared_buffer_map(frame),
        SYS_SHARED_BUFFER_UNMAP => sys_shared_buffer_unmap(frame),
        SYS_SHARED_BUFFER_SEAL => sys_shared_buffer_seal(frame),
        SYS_SHARED_BUFFER_LOAN => sys_shared_buffer_loan(frame),
        SYS_SHARED_BUFFER_LOAN_MAP => sys_shared_buffer_loan_map(frame),
        SYS_SHARED_BUFFER_RETURN => sys_shared_buffer_return(frame),
        SYS_SHARED_BUFFER_REVOKE => sys_shared_buffer_revoke(frame),
        SYS_CAP_TRANSFER => sys_cap_transfer(frame),

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

fn sys_generation_transact(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    if !current_user_range(frame.rsi, crate::generation_proto::REQUEST_LEN, false)
        || !current_user_range(frame.rdx, crate::generation_proto::REPLY_LEN, true)
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let authorized = task::with_current_mut(|task| {
        task.caps.get(slot).is_some_and(|cap| {
            matches!(cap.object, KernelObject::GenerationControl)
                && cap.rights & RIGHT_BOOT_UPDATE != 0
        })
    });
    if !authorized {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let mut request_bytes = [0u8; crate::generation_proto::REQUEST_LEN];
    if !task::copy_from_current(frame.rsi, &mut request_bytes) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let Some(request) = crate::generation_proto::WireGenerationRequest::decode(&request_bytes)
    else {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    };
    let reply = crate::generation_service::transact(&request).encode();
    // SAFETY: `current_user_range` validated the complete writable reply mapping.
    unsafe {
        core::ptr::copy_nonoverlapping(reply.as_ptr(), frame.rdx as *mut u8, reply.len());
    }
    frame.rax = ipc::ERR_SUCCESS as u64;
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
        || (grant_count > 0
            && !current_user_range(
                frame.rsi,
                grant_count * core::mem::size_of::<task::SpawnGrant>(),
                false,
            ))
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
        Err(error) => {
            crate::serial_println!("[spawn] rejected {:?}", error);
            frame.rax = match error {
                task::SpawnError::TooManyTasks | task::SpawnError::BudgetExhausted => {
                    ipc::ERR_OUT_OF_MEMORY as u64
                }
                _ => ipc::ERR_BAD_CAP as u64,
            };
        }
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

/// `SYS_SHARED_BUFFER_CREATE(factory_slot, pages, writable)`: allocate a
/// kernel-identified shared buffer under fixed global bounds and install a
/// `SharedBuffer` capability for it.
///
/// Requires a `SharedBufferFactory` capability carrying `RIGHT_BUFFER_CREATE`.
/// On success `rax` is the new capability slot and `rdx` is the buffer's
/// kernel identity. Denial returns `ERR_BAD_CAP`; a bad size returns
/// `ERR_INVALID_ARG`; byte/object exhaustion returns `ERR_OUT_OF_MEMORY`
/// without disturbing any existing holder.
fn sys_shared_buffer_create(frame: &mut UserFrame) {
    let factory_slot = frame.rdi as u32;
    let pages = frame.rsi as usize;
    let writable = frame.rdx != 0;
    let (allowed, owner, quota) = task::with_current_mut(|task| {
        let ok = task.caps.get(factory_slot).is_some_and(|cap| {
            matches!(cap.object, KernelObject::SharedBufferFactory)
                && cap.rights & RIGHT_BUFFER_CREATE != 0
        }) && task.caps.available_slots() >= 1;
        (ok, task.id, task.shared_buffer_quota)
    });
    if !allowed {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let region = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .create(owner, quota, pages, writable)
    {
        Ok(region) => region,
        Err(error) => {
            frame.rax = shared_buffer_error_code(error) as u64;
            return;
        }
    };
    let id = region.id();
    // The creator may map, loan, and transfer the buffer. Write/seal authority
    // is present only when writable creation was requested.
    let mut rights = RIGHT_BUFFER_MAP | RIGHT_BUFFER_LOAN | RIGHT_TRANSFER;
    if writable {
        rights |= RIGHT_BUFFER_WRITE;
    }
    let inserted = task::with_current_mut(|task| {
        task.caps.insert(Capability {
            object: KernelObject::SharedBuffer(region.clone()),
            rights,
        })
    });
    match inserted {
        Ok(slot) => {
            frame.rax = slot as u64;
            frame.rdx = id;
        }
        Err(_) => {
            // The capability table filled between the pre-check and insert;
            // reclaim the buffer so the allocation does not leak.
            let _ = crate::memory::shared_buffer::SHARED_BUFFER_TABLE
                .lock()
                .release_by(owner, &region);
            frame.rax = ipc::ERR_OUT_OF_MEMORY as u64;
        }
    }
}

/// `SYS_SHARED_BUFFER_RELEASE(buffer_slot)`: reclaim a shared buffer and
/// invalidate the releasing holder's capability. Returns `ERR_SUCCESS`, or
/// `ERR_BAD_CAP` if the slot is not a `SharedBuffer` this table created.
fn sys_shared_buffer_release(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let context = task::with_current_mut(|task| match task.caps.get(slot) {
        Some(Capability {
            object: KernelObject::SharedBuffer(region),
            ..
        }) => Some((task.id, region.clone())),
        _ => None,
    });
    let Some((owner, region)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    if crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .release_by(owner, &region)
        .is_err()
    {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    // The region is reclaimed; drop the releasing holder's capability so it
    // can no longer name the freed buffer.
    let _ = task::with_current_mut(|task| task.caps.take(slot));
    frame.rax = ipc::ERR_SUCCESS as u64;
}

/// `SYS_SHARED_BUFFER_MAP(buffer_slot, virtual_base, offset_bytes,
/// length_bytes, writable)`: map an exact page-aligned subrange of a live
/// shared buffer into the caller's address space.
///
/// `RIGHT_BUFFER_MAP` is always required; `writable != 0` additionally requires
/// `RIGHT_BUFFER_WRITE`. The mapping consumes one unit of the caller's
/// generation-declared mapping quota. Bounds, arithmetic, quota, rights, and
/// seal state are checked before any PTE changes.
fn sys_shared_buffer_map(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let base = frame.rsi;
    let offset = frame.rdx;
    let length = frame.r10;
    let writable = frame.r8 != 0;
    let Ok(length_usize) = usize::try_from(length) else {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    };
    if !user_range(base, length_usize) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    let context = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        let KernelObject::SharedBuffer(region) = &cap.object else {
            return None;
        };
        if cap.rights & RIGHT_BUFFER_MAP == 0 || (writable && cap.rights & RIGHT_BUFFER_WRITE == 0)
        {
            return None;
        }
        Some((
            region.clone(),
            task.id,
            task.shared_buffer_quota,
            task.address_space.pml4(),
        ))
    });
    let Some((region, owner, quota, root)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .map(owner, quota, &region, root, base, offset, length, writable)
    {
        Ok(()) => ipc::ERR_SUCCESS as u64,
        Err(error) => shared_buffer_error_code(error) as u64,
    };
}

/// `SYS_SHARED_BUFFER_UNMAP(buffer_slot, virtual_base)`: remove the caller's
/// exact mapping at `virtual_base` and return its mapping charge.
fn sys_shared_buffer_unmap(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let base = frame.rsi;
    let context = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        if cap.rights & RIGHT_BUFFER_MAP == 0 {
            return None;
        }
        let region = match &cap.object {
            KernelObject::SharedBuffer(region) => region,
            KernelObject::SharedBufferLoan(loan) => loan.region(),
            _ => return None,
        };
        Some((region.clone(), task.id, task.address_space.pml4()))
    });
    let Some((region, owner, root)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .unmap(owner, &region, root, base)
    {
        Ok(()) => ipc::ERR_SUCCESS as u64,
        Err(error) => shared_buffer_error_code(error) as u64,
    };
}

/// `SYS_SHARED_BUFFER_SEAL(buffer_slot)`: irreversibly seal one live buffer
/// read-only. `RIGHT_BUFFER_WRITE` is required: the writer publishes the
/// transition. Every existing writable PTE is downgraded before success.
fn sys_shared_buffer_seal(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let region = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        let KernelObject::SharedBuffer(region) = &cap.object else {
            return None;
        };
        (cap.rights & RIGHT_BUFFER_WRITE != 0).then(|| region.clone())
    });
    let Some(region) = region else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .seal(&region)
    {
        Ok(()) => ipc::ERR_SUCCESS as u64,
        Err(error) => shared_buffer_error_code(error) as u64,
    };
}

/// `SYS_SHARED_BUFFER_LOAN(buffer_slot, supervision_slot, offset, length)`:
/// create a read-only, single-return loan of one exact sealed subrange.
///
/// The source buffer requires `RIGHT_BUFFER_LOAN`; the supervision capability
/// names the exact receiver without ambient task identifiers. On success `rax`
/// is a `SharedBufferLoan` capability slot and `rdx` is its kernel-assigned
/// identity. The slot carries `RIGHT_TRANSFER` so the lender can deliver it to
/// the named receiver over IPC, but map/return authority is bound to that exact
/// receiver: a slot held by any other task names the loan without being able to
/// map or return it, and settlement stays reachable through the receiver's or
/// lender's death and the lender's explicit revoke.
fn sys_shared_buffer_loan(frame: &mut UserFrame) {
    let buffer_slot = frame.rdi as u32;
    let receiver_slot = frame.rsi as u32;
    let offset = frame.rdx;
    let length = frame.r10;
    let context = task::with_current_mut(|task| {
        let buffer = task.caps.get(buffer_slot)?;
        let KernelObject::SharedBuffer(region) = &buffer.object else {
            return None;
        };
        if buffer.rights & RIGHT_BUFFER_LOAN == 0 || task.caps.available_slots() == 0 {
            return None;
        }
        let receiver = task.caps.get(receiver_slot)?;
        let KernelObject::Supervision(receiver_id) = receiver.object else {
            return None;
        };
        if receiver.rights & RIGHT_SUPERVISE == 0 {
            return None;
        }
        Some((
            task.id,
            receiver_id,
            task.shared_buffer_quota,
            region.clone(),
        ))
    });
    let Some((lender, receiver, quota, region)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    if !task::is_live(receiver) {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    let grant = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .loan(lender, receiver, quota, &region, offset, length)
    {
        Ok(grant) => grant,
        Err(error) => {
            frame.rax = shared_buffer_error_code(error) as u64;
            return;
        }
    };
    let loan_id = grant.id();
    let inserted = task::with_current_mut(|task| {
        task.caps.insert(Capability {
            object: KernelObject::SharedBufferLoan(grant),
            rights: RIGHT_BUFFER_MAP | RIGHT_TRANSFER,
        })
    });
    match inserted {
        Ok(slot) => {
            frame.rax = slot as u64;
            frame.rdx = loan_id;
        }
        Err(_) => {
            let _ = crate::memory::shared_buffer::SHARED_BUFFER_TABLE
                .lock()
                .revoke_loan(lender, loan_id, &region);
            frame.rax = ipc::ERR_OUT_OF_MEMORY as u64;
        }
    }
}

/// `SYS_SHARED_BUFFER_LOAN_MAP(loan_slot, virtual_base, offset, length)`:
/// map a read-only subrange relative to the exact outstanding loan.
fn sys_shared_buffer_loan_map(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let base = frame.rsi;
    let offset = frame.rdx;
    let length = frame.r10;
    let Ok(length_usize) = usize::try_from(length) else {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    };
    if !user_range(base, length_usize) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let context = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        let KernelObject::SharedBufferLoan(loan) = &cap.object else {
            return None;
        };
        if cap.rights & RIGHT_BUFFER_MAP == 0 {
            return None;
        }
        Some((
            loan.id(),
            loan.region().clone(),
            task.id,
            task.shared_buffer_quota,
            task.address_space.pml4(),
        ))
    });
    let Some((loan_id, region, receiver, quota, root)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .map_loan(
            receiver, quota, loan_id, &region, root, base, offset, length,
        ) {
        Ok(()) => ipc::ERR_SUCCESS as u64,
        Err(error) => shared_buffer_error_code(error) as u64,
    };
}

/// `SYS_SHARED_BUFFER_RETURN(loan_slot)`: settle one exact loan and invalidate
/// the receiver's loan capability.
fn sys_shared_buffer_return(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let context = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        let KernelObject::SharedBufferLoan(loan) = &cap.object else {
            return None;
        };
        if cap.rights & RIGHT_BUFFER_MAP == 0 {
            return None;
        }
        Some((task.id, loan.id(), loan.region().clone()))
    });
    let Some((receiver, loan_id, region)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    if let Err(error) = crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .return_loan(receiver, loan_id, &region)
    {
        // A `NotFound` means the loan was already settled out from under this
        // receiver — the lender revoked it or died and `reclaim_owner` tore
        // down the mapping. The capability now names a dead loan and grants no
        // access, so drop it here to reclaim the receiver's slot rather than
        // leaving it permanently occupied.
        if matches!(
            error,
            crate::memory::shared_buffer::SharedBufferError::NotFound
        ) {
            let _ = task::with_current_mut(|task| task.caps.take(slot));
        }
        frame.rax = shared_buffer_error_code(error) as u64;
        return;
    }
    let _ = task::with_current_mut(|task| task.caps.take(slot));
    frame.rax = ipc::ERR_SUCCESS as u64;
}

/// `SYS_SHARED_BUFFER_REVOKE(buffer_slot, loan_id)`: explicitly settle one
/// outstanding loan as its lender.
fn sys_shared_buffer_revoke(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let loan_id = frame.rsi;
    let context = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        let KernelObject::SharedBuffer(region) = &cap.object else {
            return None;
        };
        (cap.rights & RIGHT_BUFFER_LOAN != 0).then(|| (task.id, region.clone()))
    });
    let Some((lender, region)) = context else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match crate::memory::shared_buffer::SHARED_BUFFER_TABLE
        .lock()
        .revoke_loan(lender, loan_id, &region)
    {
        Ok(()) => ipc::ERR_SUCCESS as u64,
        Err(error) => shared_buffer_error_code(error) as u64,
    };
}

/// `SYS_CAP_TRANSFER(endpoint_slot, capability_slot, descriptor_ptr)`: move one
/// capability to the endpoint's peer with its rights narrowed to the exact mask
/// the accompanying descriptor declares (C8.3).
///
/// The kernel's only new C8 mechanism, and deliberately generic: it knows
/// nothing of routes, schemas, or graph roles. It enforces four rules, and a
/// userspace broker composes a fabric out of them.
///
/// 1. **Transfer authority at the source.** The moved capability must carry
///    `RIGHT_TRANSFER`, the same condition `SYS_SEND` cap attachment applies.
/// 2. **Narrow only.** The destination mask must be a subset of the source
///    rights *and* of the object's meaningful rights. Widening is rejected
///    before anything moves.
/// 3. **Transfer authority is not inherited.** `RIGHT_TRANSFER` is dropped at
///    the destination unless the descriptor sets `FLAG_RETAIN_TRANSFER`, so a
///    provisioned endpoint is non-delegable by default rather than by
///    convention.
/// 4. **The descriptor describes the move.** Its declared `object_kind` must be
///    the moved capability's real kind, and the peer parses the same bytes the
///    kernel enforced, so the descriptor cannot advertise authority the
///    receiver did not get.
///
/// The move consumes the source capability, so the object never has two
/// holders, and a failed send restores the original at its full rights rather
/// than dropping it. One window is not closed here: a message already queued
/// for a task that then dies is discarded with its channel, so a capability
/// moved to a peer that terminates before calling `recv` is destroyed rather
/// than returned. `SYS_SEND`'s cap attachment has the same shape; closing it
/// needs queue-draining on endpoint teardown, which is a kernel-wide change
/// rather than a property of this syscall. The failure is a leak — never a
/// duplication and never a widening.
///
/// Returns `ERR_SUCCESS`; `ERR_BAD_CAP` for a missing slot, missing transfer
/// authority, a widening mask, or a kind mismatch; `ERR_INVALID_ARG` for a
/// malformed descriptor; `ERR_PEER_DEAD` or `ERR_WOULDBLOCK` from the
/// underlying send, with the source intact.
fn sys_cap_transfer(frame: &mut UserFrame) {
    use crate::capability_transfer_proto::{
        TRANSFER_LEN, WireCapabilityTransfer, destination_rights, kind_matches, valid_transfer,
    };

    let endpoint_slot = frame.rdi as u32;
    let capability_slot = frame.rsi as u32;
    if !current_user_range(frame.rdx, TRANSFER_LEN, false) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let mut descriptor_bytes = [0u8; TRANSFER_LEN];
    if !task::copy_from_current(frame.rdx, &mut descriptor_bytes) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let Some(descriptor) = WireCapabilityTransfer::decode(&descriptor_bytes) else {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    };
    if !valid_transfer(&descriptor) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let rights = destination_rights(&descriptor);
    if rights == 0 {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }

    // Resolve, validate, and consume under one borrow of the table so no
    // window exists where the capability is neither held nor moved.
    let prepared = task::with_current_mut(|task| {
        if endpoint_slot == capability_slot {
            return Err(ipc::ERR_BAD_CAP);
        }
        let Some(channel) = task.caps.get(endpoint_slot) else {
            return Err(ipc::ERR_BAD_CAP);
        };
        if channel.rights & RIGHT_SEND == 0 {
            return Err(ipc::ERR_BAD_CAP);
        }
        let KernelObject::Endpoint(endpoint) = &channel.object else {
            return Err(ipc::ERR_BAD_CAP);
        };
        let endpoint = endpoint.clone();
        let Some(source) = task.caps.get(capability_slot) else {
            return Err(ipc::ERR_BAD_CAP);
        };
        // Moving requires transfer authority, exactly as `SYS_SEND` does.
        if source.rights & RIGHT_TRANSFER == 0 {
            return Err(ipc::ERR_BAD_CAP);
        }
        if !kind_matches(descriptor.object_kind, &source.object) {
            return Err(ipc::ERR_BAD_CAP);
        }
        // `derive` rejects any bit the source does not hold; `insert` at the
        // destination rejects any bit meaningless for the object kind. Check
        // the latter here so a widening mask never consumes the source.
        if rights & !source.object.valid_rights() != 0 {
            return Err(ipc::ERR_BAD_CAP);
        }
        let moved = source.derive(rights).map_err(|_| ipc::ERR_BAD_CAP)?;
        let original = task
            .caps
            .take(capability_slot)
            .expect("source capability present under this borrow");
        Ok((endpoint, moved, original))
    });
    let (endpoint, moved, original) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            frame.rax = error as u64;
            return;
        }
    };

    let mut payload: [Option<Capability>; MAX_CAPS_PER_MSG] = core::array::from_fn(|_| None);
    payload[0] = Some(moved);
    let result = ipc::send(&endpoint, &descriptor_bytes, &mut payload);
    if result != ipc::ERR_SUCCESS {
        // Nothing crossed, so the move did not happen: restore the source at
        // its original rights rather than leaving the holder short a capability.
        task::with_current_mut(|task| {
            task.caps
                .put(capability_slot, original)
                .expect("source slot stayed free across a failed transfer")
        });
    }
    frame.rax = result as u64;
}

fn shared_buffer_error_code(error: crate::memory::shared_buffer::SharedBufferError) -> i64 {
    use crate::memory::shared_buffer::SharedBufferError;
    match error {
        SharedBufferError::BadSize
        | SharedBufferError::BadRange
        | SharedBufferError::MapConflict => ipc::ERR_INVALID_ARG,
        SharedBufferError::OutOfFrames
        | SharedBufferError::ObjectsExhausted
        | SharedBufferError::BytesExhausted
        | SharedBufferError::QuotaExceeded
        | SharedBufferError::MappingsExhausted
        | SharedBufferError::LoansExhausted => ipc::ERR_OUT_OF_MEMORY,
        SharedBufferError::NotFound
        | SharedBufferError::WriteDenied
        | SharedBufferError::NotSealed => ipc::ERR_BAD_CAP,
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

fn sys_directory_inspect(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let required = frame.rsi;
    if required == 0
        || required
            & !(RIGHT_DIRECTORY_READ
                | RIGHT_DIRECTORY_LIST
                | RIGHT_DIRECTORY_WRITE
                | RIGHT_DIRECTORY_DERIVE)
            != 0
        || !current_user_range(frame.rdx, 32, true)
        || !current_user_range(frame.r10, crate::capability::MAX_DIRECTORY_PATH, true)
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let inspected = task::with_current_mut(|task| {
        let cap = task.caps.get(slot)?;
        if cap.rights & required != required {
            return None;
        }
        let KernelObject::Directory(directory) = &cap.object else {
            return None;
        };
        let mut scope = [0u8; crate::capability::MAX_DIRECTORY_PATH];
        let scope_len = directory.scope().len();
        scope[..scope_len].copy_from_slice(directory.scope());
        Some((directory.root_identity(), scope, scope_len))
    });
    let Some((root, scope, scope_len)) = inspected else {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    unsafe {
        core::ptr::copy_nonoverlapping(root.as_ptr(), frame.rdx as *mut u8, root.len());
        core::ptr::copy_nonoverlapping(scope.as_ptr(), frame.r10 as *mut u8, scope.len());
    }
    frame.rax = scope_len as u64;
}

fn sys_directory_derive(frame: &mut UserFrame) {
    let slot = frame.rdi as u32;
    let path_len = frame.rdx as usize;
    let rights = frame.r10;
    let allowed_rights = RIGHT_DIRECTORY_READ
        | RIGHT_DIRECTORY_WRITE
        | RIGHT_DIRECTORY_LIST
        | RIGHT_DIRECTORY_DERIVE
        | RIGHT_TRANSFER;
    if path_len > crate::capability::MAX_DIRECTORY_PATH
        || (path_len > 0 && !current_user_range(frame.rsi, path_len, false))
        || rights == 0
        || rights & !allowed_rights != 0
    {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let mut path = [0u8; crate::capability::MAX_DIRECTORY_PATH];
    if path_len > 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(frame.rsi as *const u8, path.as_mut_ptr(), path_len)
        };
    }
    let derived = task::with_current_mut(|task| {
        let source = task
            .caps
            .get(slot)
            .ok_or(crate::capability::CapError::BadSlot)?;
        if source.rights & RIGHT_DIRECTORY_DERIVE == 0
            || rights & !source.rights != 0
            || rights & RIGHT_TRANSFER != 0 && source.rights & RIGHT_TRANSFER == 0
        {
            return Err(crate::capability::CapError::BadRights);
        }
        let KernelObject::Directory(directory) = &source.object else {
            return Err(crate::capability::CapError::WrongObject);
        };
        let object = KernelObject::Directory(directory.derive(&path[..path_len])?);
        task.caps.insert(Capability { object, rights })
    });
    frame.rax = match derived {
        Ok(slot) => slot as u64,
        Err(crate::capability::CapError::TableFull) => ipc::ERR_OUT_OF_MEMORY as u64,
        Err(_) => ipc::ERR_BAD_CAP as u64,
    };
}

/// Atomically commits only through an unscoped directory writer. This prevents
/// a subdirectory snapshot from replacing the namespace-wide root.
fn sys_directory_commit(frame: &mut UserFrame) {
    if !current_user_range(frame.rsi, 32, false) || !current_user_range(frame.rdx, 32, false) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let mut expected = [0u8; 32];
    let mut new = [0u8; 32];
    unsafe {
        core::ptr::copy_nonoverlapping(frame.rsi as *const u8, expected.as_mut_ptr(), 32);
        core::ptr::copy_nonoverlapping(frame.rdx as *const u8, new.as_mut_ptr(), 32);
    }
    let committed = task::with_current_mut(|task| {
        let cap = task.caps.get(frame.rdi as u32)?;
        if cap.rights & RIGHT_DIRECTORY_WRITE == 0 {
            return None;
        }
        let KernelObject::Directory(directory) = &cap.object else {
            return None;
        };
        if !directory.scope().is_empty() {
            return None;
        }
        Some(directory.commit_root(expected, new))
    });
    frame.rax = match committed {
        Some(true) => ipc::ERR_SUCCESS as u64,
        Some(false) => ipc::ERR_WOULDBLOCK as u64,
        None => ipc::ERR_BAD_CAP as u64,
    };
}

fn sys_input_read(frame: &mut UserFrame) {
    let authorized = task::with_current_mut(|task| {
        task.caps.get(frame.rdi as u32).is_some_and(|cap| {
            matches!(cap.object, KernelObject::Input) && cap.rights & RIGHT_INPUT_READ != 0
        })
    });
    if !authorized {
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    }
    crate::input::pump_script();
    let Some(event) = crate::input::pop_event() else {
        frame.rax = ipc::ERR_WOULDBLOCK as u64;
        return;
    };
    frame.rax = 0;
    frame.rdx = encode_key_event(event);
}

fn encode_key_event(event: crate::input::KeyEvent) -> u64 {
    let code = match event.code {
        crate::input::KeyCode::Escape => 1,
        crate::input::KeyCode::Backspace => 2,
        crate::input::KeyCode::Tab => 3,
        crate::input::KeyCode::Enter => 4,
        crate::input::KeyCode::LeftControl => 5,
        crate::input::KeyCode::LeftShift => 6,
        crate::input::KeyCode::RightShift => 7,
        crate::input::KeyCode::LeftAlt => 8,
        crate::input::KeyCode::Space => 9,
        crate::input::KeyCode::Up => 10,
        crate::input::KeyCode::Down => 11,
        crate::input::KeyCode::Left => 12,
        crate::input::KeyCode::Right => 13,
        crate::input::KeyCode::Character(character) => 0x100 | character as u32,
        crate::input::KeyCode::Unknown(code) => 0x1_0000 | u32::from(code),
    };
    u64::from(code) | u64::from(event.pressed) << 32
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
    crate::frame_buffer::write_bytes(bytes);
    frame.rax = len as u64;
}
fn sys_generation_receive(frame: &mut UserFrame) {
    let receiver_slot = frame.rdi as u32;
    let transfer_slot = frame.rsi as u32;
    let authorized = task::with_current_mut(|task| {
        let receiver = task.caps.get(receiver_slot)?;
        let transfer = task.caps.get(transfer_slot)?;
        if receiver.rights & (RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_BOOT_UPDATE)
            != (RIGHT_BLOCK_READ | RIGHT_BLOCK_WRITE | RIGHT_BOOT_UPDATE)
            || transfer.rights & (RIGHT_BLOCK_READ | RIGHT_TRANSFER)
                != (RIGHT_BLOCK_READ | RIGHT_TRANSFER)
        {
            return None;
        }
        let KernelObject::BlockDevice(receiver) = receiver.object else {
            return None;
        };
        let KernelObject::BlockDevice(transfer) = transfer.object else {
            return None;
        };
        Some((receiver, transfer))
    });
    let Some((receiver, transfer)) = authorized else {
        crate::serial_println!(
            "[transfer] unauthorized receiver_slot={} transfer_slot={}",
            receiver_slot,
            transfer_slot
        );
        frame.rax = ipc::ERR_BAD_CAP as u64;
        return;
    };
    frame.rax = match task::without_interrupts(|| crate::transfer::receive(receiver, transfer)) {
        Ok(result) => {
            crate::serial_println!(
                "[transfer] generation received objects={} states={} release={} attempts={}",
                result.copied_objects,
                result.state_count,
                result.release_sequence,
                result.remaining_attempts
            );
            ipc::ERR_SUCCESS as u64
        }
        Err(error) => {
            crate::serial_println!("[transfer] receive rejected: {:?}", error);
            ipc::ERR_INVALID_ARG as u64
        }
    };
}

/// Maximum wait sources per `SYS_WAIT` call. Bounds the kernel-side copy, and
/// bounds the live ingress sources a C8.2 fabric graph may declare: a fabric
/// that cannot register every wake source would have to poll.
pub const MAX_WAIT_SOURCES: usize = 8;

/// Wait-source kinds, packed into the high 32 bits of each descriptor. Must
/// match the userspace shim's `WAIT_*` constants.
const WAIT_KIND_ENDPOINT: u32 = 0;
const WAIT_KIND_INPUT: u32 = 1;
const WAIT_KIND_SUPERVISION: u32 = 2;

/// `SYS_WAIT(descriptors_ptr, count)`: park the caller until one of up to
/// `MAX_WAIT_SOURCES` sources becomes ready. Each descriptor is a `u64`
/// packing `kind << 32 | slot`. Returns 0 always (userspace re-polls each
/// source through its non-blocking ABI after waking); `ERR_INVALID_ARG` for a
/// malformed request, without blocking.
fn sys_wait(frame: &mut UserFrame) {
    let count = frame.rsi as usize;
    if count == 0 || count > MAX_WAIT_SOURCES {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    let byte_len = count * core::mem::size_of::<u64>();
    if !current_user_range(frame.rdi, byte_len, false) {
        frame.rax = ipc::ERR_INVALID_ARG as u64;
        return;
    }
    // SAFETY: the current task's complete user range was validated as mapped.
    let raw = unsafe { core::slice::from_raw_parts(frame.rdi as *const u64, count) };
    let mut sources = [task::WaitSource::Input; MAX_WAIT_SOURCES];
    for (slot, descriptor) in sources.iter_mut().zip(raw.iter().copied()) {
        let kind = (descriptor >> 32) as u32;
        let cap_slot = descriptor as u32;
        *slot = match kind {
            WAIT_KIND_ENDPOINT => task::WaitSource::Endpoint(cap_slot),
            WAIT_KIND_INPUT => task::WaitSource::Input,
            WAIT_KIND_SUPERVISION => task::WaitSource::Supervision(cap_slot),
            _ => {
                frame.rax = ipc::ERR_INVALID_ARG as u64;
                return;
            }
        };
    }
    task::wait(frame, &sources[..count]);
}
