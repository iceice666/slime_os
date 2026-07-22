use alloc::{vec, vec::Vec};

use boot_contracts::bootstate::{BootState, SLOT_BYTES};
use boot_contracts::generation::Generation;
use boot_contracts::release::{INITIAL_TRUST_ROOT, RELEASE_BYTES, Release};

use crate::block_device::BlockDevice;
use crate::block_proto::SECTOR_SIZE;
use crate::generation_proto::{
    GENERATION_E_BAD_CLOSURE, GENERATION_E_BAD_RELEASE, GENERATION_E_CONFLICT, GENERATION_E_DEVICE,
    GENERATION_E_NOT_FOUND, GENERATION_E_OK, MAX_ENTRIES, OP_INSPECT, OP_LIST, OP_ROLLBACK,
    OP_SELECT, OP_STAGE, REPLY_FLAG_KNOWN_GOOD, REPLY_FLAG_PENDING, REPLY_FLAG_RUNNING,
    REPLY_FLAG_STAGED, WireGenerationReply, WireGenerationRequest, identity_words,
    request_identity, valid_request,
};
use crate::sha256::Sha256;

const DIRECTORY_MAGIC: [u8; 8] = *b"SLIMEBT\0";
const DIRECTORY_VERSION: u32 = 1;
const DIRECTORY_HEADER: usize = 96;
const DIRECTORY_ENTRY: usize = 96;
const DIRECTORY_OFFSET: usize = 4096;
const RELEASES_OFFSET: usize = 8192;
const GENERATIONS_OFFSET: usize = 16 * 1024;
const BOOT_STORE_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub selected_slot: u8,
    pub target_slot: u8,
    pub before: BootState,
    pub after: BootState,
}

#[derive(Clone, Copy)]
struct DirectoryEntry {
    identity: [u8; 32],
    generation_offset: usize,
    generation_len: usize,
    release_offset: usize,
}

pub fn transact(request: &WireGenerationRequest) -> WireGenerationReply {
    let mut reply = empty_reply();
    if !valid_request(request) {
        reply.status = crate::generation_proto::GENERATION_E_BAD_REQUEST;
        return reply;
    }
    match crate::block_service::with_device(|device| Ok(execute(device, request))) {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            reply.status = error.status();
            reply
        }
        Err(_) => {
            reply.status = GENERATION_E_DEVICE;
            reply
        }
    }
}

fn execute(
    device: &mut BlockDevice,
    request: &WireGenerationRequest,
) -> Result<WireGenerationReply, ServiceError> {
    let entries = read_directory(device)?;
    let (selected_slot, state) = read_bootstate(device)?;
    match request.op {
        OP_LIST => Ok(list_reply(&entries, state)),
        OP_INSPECT => inspect_reply(device, &entries, state, request_identity(request)),
        OP_STAGE => stage_reply(device, &entries, state, request_identity(request)),
        OP_SELECT => select_reply(
            device,
            &entries,
            selected_slot,
            state,
            request_identity(request),
        ),
        OP_ROLLBACK => rollback_reply(device, selected_slot, state),
        _ => Err(ServiceError::BadRequest),
    }
}

fn list_reply(entries: &[DirectoryEntry], state: BootState) -> WireGenerationReply {
    let mut reply = empty_reply();
    reply.count = entries.len() as u32;
    reply.release_sequence = state.accepted_release_sequence as u32;
    reply.remaining_attempts = state.remaining_attempts;
    if let Some(pending) = state.pending {
        reply.flags = REPLY_FLAG_PENDING;
        set_reply_identity(&mut reply, pending);
    } else if let Some(candidate) = entries
        .iter()
        .find(|entry| entry.identity != state.known_good)
    {
        set_reply_identity(&mut reply, candidate.identity);
    } else {
        set_reply_identity(&mut reply, state.known_good);
    }
    reply
}

fn inspect_reply(
    device: &mut BlockDevice,
    entries: &[DirectoryEntry],
    state: BootState,
    identity: [u8; 32],
) -> Result<WireGenerationReply, ServiceError> {
    let entry = find_entry(entries, identity)?;
    let generation_bytes = read_range(device, entry.generation_offset, entry.generation_len)?;
    let generation = Generation::decode(&generation_bytes).map_err(|_| ServiceError::BadClosure)?;
    let release_bytes = read_range(device, entry.release_offset, RELEASE_BYTES)?;
    let release = Release::decode(&release_bytes).map_err(|_| ServiceError::BadRelease)?;
    release
        .verify_generation(&generation, &INITIAL_TRUST_ROOT)
        .map_err(|_| ServiceError::BadRelease)?;
    let mut reply = empty_reply();
    reply.count = generation.object_count() as u32;
    reply.generation_number = generation.number as u32;
    reply.release_sequence = release.sequence as u32;
    reply.remaining_attempts = state.remaining_attempts;
    reply.flags = flags_for(identity, state, false);
    set_reply_identity(&mut reply, identity);
    Ok(reply)
}

fn stage_reply(
    device: &mut BlockDevice,
    entries: &[DirectoryEntry],
    state: BootState,
    identity: [u8; 32],
) -> Result<WireGenerationReply, ServiceError> {
    let entry = find_entry(entries, identity)?;
    let generation_bytes = read_range(device, entry.generation_offset, entry.generation_len)?;
    let generation = Generation::decode(&generation_bytes).map_err(|_| ServiceError::BadClosure)?;
    let release_bytes = read_range(device, entry.release_offset, RELEASE_BYTES)?;
    let release = Release::decode(&release_bytes).map_err(|_| ServiceError::BadRelease)?;
    release
        .verify_for_staging(
            &generation,
            &INITIAL_TRUST_ROOT,
            state.accepted_release_sequence,
        )
        .map_err(|_| ServiceError::BadRelease)?;
    verify_closure(entries, &generation)?;
    if !crate::generation_manager::retain_staged(identity) {
        return Err(ServiceError::Conflict);
    }
    let mut reply = empty_reply();
    reply.generation_number = generation.number as u32;
    reply.release_sequence = release.sequence as u32;
    reply.remaining_attempts = state.remaining_attempts;
    reply.flags = flags_for(identity, state, true);
    set_reply_identity(&mut reply, identity);
    Ok(reply)
}

fn select_reply(
    device: &mut BlockDevice,
    entries: &[DirectoryEntry],
    selected_slot: u8,
    state: BootState,
    identity: [u8; 32],
) -> Result<WireGenerationReply, ServiceError> {
    let entry = find_entry(entries, identity)?;
    let generation_bytes = read_range(device, entry.generation_offset, entry.generation_len)?;
    let generation = Generation::decode(&generation_bytes).map_err(|_| ServiceError::BadClosure)?;
    let release_bytes = read_range(device, entry.release_offset, RELEASE_BYTES)?;
    let release = Release::decode(&release_bytes).map_err(|_| ServiceError::BadRelease)?;
    release
        .verify_for_staging(
            &generation,
            &INITIAL_TRUST_ROOT,
            state.accepted_release_sequence,
        )
        .map_err(|_| ServiceError::BadRelease)?;
    verify_closure(entries, &generation)?;
    if state.pending == Some(identity) {
        let mut reply = empty_reply();
        reply.generation_number = generation.number as u32;
        reply.release_sequence = release.sequence as u32;
        reply.remaining_attempts = state.remaining_attempts;
        reply.flags = flags_for(identity, state, true);
        set_reply_identity(&mut reply, identity);
        return Ok(reply);
    }
    if state.pending.is_some() || identity == crate::boot::generation_identity() {
        return Err(ServiceError::Conflict);
    }
    if !crate::generation_manager::is_staged(identity) {
        return Err(ServiceError::Conflict);
    }
    let after = state
        .stage_pending(
            identity,
            generation.boot_attempts,
            state.generation_root,
            state.state_root,
        )
        .map_err(|_| ServiceError::Conflict)?;
    let transition = persist_transition(device, selected_slot, state, after)?;
    crate::generation_manager::remove_staged(identity);
    crate::generation_manager::record_bootstate(after);
    emit_transition("stage-pending", "after-pending-commit", transition);
    let mut reply = empty_reply();
    reply.generation_number = generation.number as u32;
    reply.release_sequence = release.sequence as u32;
    reply.remaining_attempts = after.remaining_attempts;
    reply.flags = flags_for(identity, after, true);
    set_reply_identity(&mut reply, identity);
    Ok(reply)
}

fn rollback_reply(
    device: &mut BlockDevice,
    selected_slot: u8,
    state: BootState,
) -> Result<WireGenerationReply, ServiceError> {
    if state.pending.is_none() {
        return Err(ServiceError::Conflict);
    }
    let after = state
        .rollback_pending()
        .map_err(|_| ServiceError::Conflict)?;
    let transition = persist_transition(device, selected_slot, state, after)?;
    crate::generation_manager::record_bootstate(after);
    emit_transition("rollback", "rollback-update", transition);
    let mut reply = empty_reply();
    reply.release_sequence = after.accepted_release_sequence as u32;
    reply.remaining_attempts = after.remaining_attempts;
    reply.flags = flags_for(after.known_good, after, false);
    set_reply_identity(&mut reply, after.known_good);
    Ok(reply)
}

fn persist_transition(
    device: &mut BlockDevice,
    selected_slot: u8,
    before: BootState,
    after: BootState,
) -> Result<Transition, ServiceError> {
    let target_slot = 1 - selected_slot;
    let encoded = after.encode().map_err(|_| ServiceError::Conflict)?;
    write_range(device, target_slot as usize * SLOT_BYTES, &encoded)?;
    device.flush().map_err(|_| ServiceError::Device)?;
    Ok(Transition {
        selected_slot,
        target_slot,
        before,
        after,
    })
}

fn read_bootstate(device: &mut BlockDevice) -> Result<(u8, BootState), ServiceError> {
    let a = read_range(device, 0, SLOT_BYTES)?;
    let b = read_range(device, SLOT_BYTES, SLOT_BYTES)?;
    let a: &[u8; SLOT_BYTES] = a.as_slice().try_into().unwrap();
    let b: &[u8; SLOT_BYTES] = b.as_slice().try_into().unwrap();
    match (BootState::decode(a), BootState::decode(b)) {
        (Ok(a), Ok(b)) if a.sequence > b.sequence => Ok((0, a)),
        (Ok(a), Ok(b)) if b.sequence > a.sequence => Ok((1, b)),
        (Ok(a), Ok(b)) if a == b => Ok((0, a)),
        (Ok(_), Ok(_)) => Err(ServiceError::BadClosure),
        (Ok(a), Err(_)) => Ok((0, a)),
        (Err(_), Ok(b)) => Ok((1, b)),
        (Err(_), Err(_)) => Err(ServiceError::BadClosure),
    }
}

fn read_directory(device: &mut BlockDevice) -> Result<Vec<DirectoryEntry>, ServiceError> {
    let header = read_range(device, DIRECTORY_OFFSET, DIRECTORY_HEADER)?;
    if header[..8] != DIRECTORY_MAGIC
        || u32_at(&header, 8)? != DIRECTORY_VERSION
        || u32_at(&header, 12)? as usize != DIRECTORY_HEADER
        || u64_at(&header, 16)? != 0
        || u32_at(&header, 28)? != 0
        || u64_at(&header, 40)? as usize != BOOT_STORE_BYTES
    {
        return Err(ServiceError::BadClosure);
    }
    let count = u32_at(&header, 24)? as usize;
    if !(1..=MAX_ENTRIES).contains(&count)
        || u64_at(&header, 32)? as usize != count * DIRECTORY_ENTRY
    {
        return Err(ServiceError::BadClosure);
    }
    let raw = read_range(
        device,
        DIRECTORY_OFFSET + DIRECTORY_HEADER,
        count * DIRECTORY_ENTRY,
    )?;
    let mut entries = Vec::with_capacity(count);
    let mut previous = [0u8; 32];
    let mut root = Sha256::new();
    for index in 0..count {
        let record = &raw[index * DIRECTORY_ENTRY..(index + 1) * DIRECTORY_ENTRY];
        let identity: [u8; 32] = record[..32].try_into().unwrap();
        let generation_offset = u64_at(record, 32)? as usize;
        let generation_len = u64_at(record, 40)? as usize;
        let release_offset = u64_at(record, 48)? as usize;
        let release_len = u64_at(record, 56)? as usize;
        if (index > 0 && identity <= previous)
            || generation_offset < GENERATIONS_OFFSET
            || !generation_offset.is_multiple_of(4096)
            || generation_len == 0
            || generation_offset
                .checked_add(generation_len)
                .is_none_or(|end| end > BOOT_STORE_BYTES)
            || release_offset < RELEASES_OFFSET
            || !release_offset.is_multiple_of(RELEASE_BYTES)
            || release_len != RELEASE_BYTES
            || release_offset + release_len > GENERATIONS_OFFSET
            || record[64..].iter().any(|byte| *byte != 0)
        {
            return Err(ServiceError::BadClosure);
        }
        root.update(&identity);
        entries.push(DirectoryEntry {
            identity,
            generation_offset,
            generation_len,
            release_offset,
        });
        previous = identity;
    }
    let (_, state) = read_bootstate(device)?;
    if root.finalize() != state.generation_root {
        return Err(ServiceError::BadClosure);
    }
    Ok(entries)
}

fn verify_closure(
    entries: &[DirectoryEntry],
    generation: &Generation<'_>,
) -> Result<(), ServiceError> {
    if generation
        .parent
        .is_some_and(|parent| !entries.iter().any(|entry| entry.identity == parent))
    {
        return Err(ServiceError::BadClosure);
    }
    for index in 0..generation.object_count() {
        let object = generation
            .object(index)
            .map_err(|_| ServiceError::BadClosure)?;
        if crate::sha256::digest(object.bytes) != object.digest {
            return Err(ServiceError::BadClosure);
        }
    }
    Ok(())
}

fn find_entry(
    entries: &[DirectoryEntry],
    identity: [u8; 32],
) -> Result<DirectoryEntry, ServiceError> {
    entries
        .iter()
        .copied()
        .find(|entry| entry.identity == identity)
        .ok_or(ServiceError::NotFound)
}

fn flags_for(identity: [u8; 32], state: BootState, staged: bool) -> u32 {
    let mut flags = 0;
    if identity == state.known_good {
        flags |= REPLY_FLAG_KNOWN_GOOD;
    }
    if state.pending == Some(identity) {
        flags |= REPLY_FLAG_PENDING;
    }
    if identity == crate::boot::generation_identity() {
        flags |= REPLY_FLAG_RUNNING;
    }
    if staged {
        flags |= REPLY_FLAG_STAGED;
    }
    flags
}

fn empty_reply() -> WireGenerationReply {
    WireGenerationReply {
        magic: crate::generation_proto::GENERATION_MAGIC,
        version: crate::generation_proto::FORMAT_VERSION,
        status: GENERATION_E_OK,
        flags: 0,
        count: 0,
        generation_number: 0,
        release_sequence: 0,
        remaining_attempts: 0,
        generation0: 0,
        generation1: 0,
        generation2: 0,
        generation3: 0,
    }
}

fn set_reply_identity(reply: &mut WireGenerationReply, identity: [u8; 32]) {
    let words = identity_words(identity);
    reply.generation0 = words[0];
    reply.generation1 = words[1];
    reply.generation2 = words[2];
    reply.generation3 = words[3];
}

fn emit_transition(action: &str, commit: &str, transition: Transition) {
    let action = match action {
        "stage-pending" => boot_contracts::trace::Action::StagePending,
        "rollback" => boot_contracts::trace::Action::Rollback,
        _ => return,
    };
    let commit = match commit {
        "after-pending-commit" => boot_contracts::trace::Commit::AfterPendingCommit,
        "rollback-update" => boot_contracts::trace::Commit::RollbackUpdate,
        _ => return,
    };
    let line = boot_contracts::trace::Record {
        action,
        commit,
        selected_slot: transition.selected_slot,
        target_slot: Some(transition.target_slot),
        sequence_before: transition.before.sequence,
        sequence_after: transition.after.sequence,
        attempts_before: transition.before.remaining_attempts,
        attempts_after: transition.after.remaining_attempts,
        known_good: transition.after.known_good,
        pending: transition.after.pending,
        generation_root: transition.after.generation_root,
        state_root: transition.after.state_root,
    }
    .render();
    crate::serial_println!("{}", line.as_str());
}

fn read_range(
    device: &mut BlockDevice,
    offset: usize,
    len: usize,
) -> Result<Vec<u8>, ServiceError> {
    let end = offset.checked_add(len).ok_or(ServiceError::BadClosure)?;
    if end > BOOT_STORE_BYTES {
        return Err(ServiceError::BadClosure);
    }
    let mut out = vec![0; len];
    let first = offset / SECTOR_SIZE;
    let last = end.div_ceil(SECTOR_SIZE);
    let mut sector = [0u8; SECTOR_SIZE];
    for lba in first..last {
        device
            .read_sector(lba as u64, &mut sector)
            .map_err(|_| ServiceError::Device)?;
        let sector_start = lba * SECTOR_SIZE;
        let copy_start = offset.max(sector_start);
        let copy_end = end.min(sector_start + SECTOR_SIZE);
        out[copy_start - offset..copy_end - offset]
            .copy_from_slice(&sector[copy_start - sector_start..copy_end - sector_start]);
    }
    Ok(out)
}

fn write_range(device: &mut BlockDevice, offset: usize, bytes: &[u8]) -> Result<(), ServiceError> {
    if !offset.is_multiple_of(SECTOR_SIZE) || !bytes.len().is_multiple_of(SECTOR_SIZE) {
        return Err(ServiceError::BadClosure);
    }
    for (index, sector) in bytes.chunks_exact(SECTOR_SIZE).enumerate() {
        device
            .write_sector((offset / SECTOR_SIZE + index) as u64, sector)
            .map_err(|_| ServiceError::Device)?;
    }
    Ok(())
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, ServiceError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(ServiceError::BadClosure)?
            .try_into()
            .unwrap(),
    ))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, ServiceError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(ServiceError::BadClosure)?
            .try_into()
            .unwrap(),
    ))
}

#[derive(Debug)]
enum ServiceError {
    BadRequest,
    NotFound,
    BadRelease,
    BadClosure,
    Conflict,
    Device,
}

impl ServiceError {
    pub fn status(&self) -> i32 {
        match self {
            Self::BadRequest => crate::generation_proto::GENERATION_E_BAD_REQUEST,
            Self::NotFound => GENERATION_E_NOT_FOUND,
            Self::BadRelease => GENERATION_E_BAD_RELEASE,
            Self::BadClosure => GENERATION_E_BAD_CLOSURE,
            Self::Conflict => GENERATION_E_CONFLICT,
            Self::Device => GENERATION_E_DEVICE,
        }
    }
}
