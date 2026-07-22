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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferDirectoryEntry {
    pub identity: [u8; 32],
    pub generation_offset: usize,
    pub generation_len: usize,
    pub release_offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferInstallResult {
    pub remaining_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferServiceError {
    NotFound,
    BadRelease,
    BadClosure,
    Conflict,
    Device,
}

impl From<ServiceError> for TransferServiceError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::NotFound => Self::NotFound,
            ServiceError::BadRelease => Self::BadRelease,
            ServiceError::BadClosure | ServiceError::BadRequest => Self::BadClosure,
            ServiceError::Conflict => Self::Conflict,
            ServiceError::Device => Self::Device,
        }
    }
}

pub fn read_entries_for_transfer(
    device: &mut BlockDevice,
) -> Result<Vec<TransferDirectoryEntry>, TransferServiceError> {
    let entries = read_directory(device)?;
    read_bootstate_for_root(device, directory_root(&entries))?;
    Ok(entries
        .into_iter()
        .map(|entry| TransferDirectoryEntry {
            identity: entry.identity,
            generation_offset: entry.generation_offset,
            generation_len: entry.generation_len,
            release_offset: entry.release_offset,
        })
        .collect())
}

pub fn read_state_for_transfer(
    device: &mut BlockDevice,
) -> Result<(u8, BootState), TransferServiceError> {
    let entries = read_directory(device)?;
    read_bootstate_for_root(device, directory_root(&entries)).map_err(Into::into)
}

pub fn read_object_by_digest_for_transfer(
    device: &mut BlockDevice,
    entries: &[TransferDirectoryEntry],
    digest: [u8; 32],
    length: usize,
) -> Result<Vec<u8>, TransferServiceError> {
    for entry in entries {
        let bytes = read_range(device, entry.generation_offset, entry.generation_len)?;
        let generation = Generation::decode(&bytes).map_err(|_| ServiceError::BadClosure)?;
        for index in 0..generation.object_count() {
            let object = generation
                .object(index)
                .map_err(|_| ServiceError::BadClosure)?;
            if object.digest == digest && object.bytes.len() == length {
                return Ok(object.bytes.to_vec());
            }
        }
    }
    Err(TransferServiceError::NotFound)
}

pub fn install_and_select_for_transfer(
    device: &mut BlockDevice,
    entries: &mut Vec<TransferDirectoryEntry>,
    selected_slot: u8,
    state: BootState,
    generation_bytes: &[u8],
    release_bytes: &[u8],
    state_root: [u8; 32],
) -> Result<TransferInstallResult, TransferServiceError> {
    if release_bytes.len() != RELEASE_BYTES
        || entries.len() >= MAX_ENTRIES
        || DIRECTORY_OFFSET + DIRECTORY_HEADER + (entries.len() + 1) * DIRECTORY_ENTRY
            > RELEASES_OFFSET
        || state.pending.is_some()
    {
        return Err(TransferServiceError::Conflict);
    }
    let generation =
        Generation::decode(generation_bytes).map_err(|_| TransferServiceError::BadClosure)?;
    let release = Release::decode(release_bytes).map_err(|_| TransferServiceError::BadRelease)?;
    release
        .verify_for_staging(
            &generation,
            &INITIAL_TRUST_ROOT,
            state.accepted_release_sequence,
        )
        .map_err(|_| TransferServiceError::BadRelease)?;
    if entries
        .iter()
        .any(|entry| entry.identity == generation.identity)
    {
        return Err(TransferServiceError::Conflict);
    }
    if generation
        .parent
        .is_some_and(|parent| !entries.iter().any(|entry| entry.identity == parent))
    {
        return Err(TransferServiceError::BadClosure);
    }
    let release_offset = entries
        .iter()
        .map(|entry| entry.release_offset + RELEASE_BYTES)
        .max()
        .unwrap_or(RELEASES_OFFSET)
        .next_multiple_of(RELEASE_BYTES);
    let generation_offset = entries
        .iter()
        .map(|entry| entry.generation_offset + entry.generation_len)
        .max()
        .unwrap_or(GENERATIONS_OFFSET)
        .next_multiple_of(4096);
    if release_offset + RELEASE_BYTES > GENERATIONS_OFFSET
        || generation_offset
            .checked_add(generation_bytes.len())
            .is_none_or(|end| end > BOOT_STORE_BYTES)
    {
        return Err(TransferServiceError::BadClosure);
    }
    entries.push(TransferDirectoryEntry {
        identity: generation.identity,
        generation_offset,
        generation_len: generation_bytes.len(),
        release_offset,
    });
    entries.sort_by_key(|entry| entry.identity);
    let generation_root = transfer_generation_root(entries);
    let after = state
        .stage_pending(
            generation.identity,
            generation.boot_attempts,
            generation_root,
            state_root,
        )
        .map_err(|_| TransferServiceError::Conflict)?;
    after
        .encode()
        .map_err(|_| TransferServiceError::BadClosure)?;

    write_bytes(device, release_offset, release_bytes)?;
    write_bytes(device, generation_offset, generation_bytes)?;
    let transition = persist_transition(device, selected_slot, state, after)?;
    persist_directory_for_transfer(device, entries)?;
    emit_transition("stage-pending", "after-pending-commit", transition);
    Ok(TransferInstallResult {
        remaining_attempts: after.remaining_attempts,
    })
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
    let (selected_slot, state) = read_bootstate_for_root(device, directory_root(&entries))?;
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

fn transfer_generation_root(entries: &[TransferDirectoryEntry]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(&entry.identity);
    }
    hasher.finalize()
}

fn bootstore_checksum(device: &mut BlockDevice) -> Result<[u8; 32], TransferServiceError> {
    let mut hasher = Sha256::new();
    let mut sector = [0u8; SECTOR_SIZE];
    for lba in (SLOT_BYTES * 2 / SECTOR_SIZE)..(BOOT_STORE_BYTES / SECTOR_SIZE) {
        device
            .read_sector(lba as u64, &mut sector)
            .map_err(|_| TransferServiceError::Device)?;
        if lba == DIRECTORY_OFFSET / SECTOR_SIZE {
            hasher.update(&sector[..48]);
            hasher.update(&[0u8; 32]);
            hasher.update(&sector[80..]);
        } else {
            hasher.update(&sector);
        }
    }
    Ok(hasher.finalize())
}

fn persist_directory_for_transfer(
    device: &mut BlockDevice,
    entries: &[TransferDirectoryEntry],
) -> Result<(), TransferServiceError> {
    if entries.is_empty() || entries.len() > MAX_ENTRIES {
        return Err(TransferServiceError::BadClosure);
    }
    let mut directory = vec![0u8; DIRECTORY_HEADER + entries.len() * DIRECTORY_ENTRY];
    directory[..8].copy_from_slice(&DIRECTORY_MAGIC);
    directory[8..12].copy_from_slice(&DIRECTORY_VERSION.to_le_bytes());
    directory[12..16].copy_from_slice(&(DIRECTORY_HEADER as u32).to_le_bytes());
    directory[24..28].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    directory[32..40].copy_from_slice(&((entries.len() * DIRECTORY_ENTRY) as u64).to_le_bytes());
    directory[40..48].copy_from_slice(&(BOOT_STORE_BYTES as u64).to_le_bytes());
    for (index, entry) in entries.iter().enumerate() {
        let offset = DIRECTORY_HEADER + index * DIRECTORY_ENTRY;
        directory[offset..offset + 32].copy_from_slice(&entry.identity);
        directory[offset + 32..offset + 40]
            .copy_from_slice(&(entry.generation_offset as u64).to_le_bytes());
        directory[offset + 40..offset + 48]
            .copy_from_slice(&(entry.generation_len as u64).to_le_bytes());
        directory[offset + 48..offset + 56]
            .copy_from_slice(&(entry.release_offset as u64).to_le_bytes());
        directory[offset + 56..offset + 64].copy_from_slice(&(RELEASE_BYTES as u64).to_le_bytes());
    }
    let clear = vec![0u8; RELEASES_OFFSET - DIRECTORY_OFFSET];
    write_bytes(device, DIRECTORY_OFFSET, &clear)?;
    write_bytes(device, DIRECTORY_OFFSET, &directory)?;
    device.flush().map_err(|_| TransferServiceError::Device)?;
    let checksum = bootstore_checksum(device)?;
    write_bytes(device, DIRECTORY_OFFSET + 48, &checksum)?;
    device.flush().map_err(|_| TransferServiceError::Device)?;
    Ok(())
}

fn write_bytes(
    device: &mut BlockDevice,
    offset: usize,
    bytes: &[u8],
) -> Result<(), TransferServiceError> {
    let end = offset
        .checked_add(bytes.len())
        .filter(|end| *end <= BOOT_STORE_BYTES)
        .ok_or(TransferServiceError::BadClosure)?;
    let mut sector = [0u8; SECTOR_SIZE];
    for lba in offset / SECTOR_SIZE..end.div_ceil(SECTOR_SIZE) {
        let sector_start = lba * SECTOR_SIZE;
        let copy_start = offset.max(sector_start);
        let copy_end = end.min(sector_start + SECTOR_SIZE);
        if copy_start != sector_start || copy_end != sector_start + SECTOR_SIZE {
            device
                .read_sector(lba as u64, &mut sector)
                .map_err(|_| TransferServiceError::Device)?;
        } else {
            sector.fill(0);
        }
        sector[copy_start - sector_start..copy_end - sector_start]
            .copy_from_slice(&bytes[copy_start - offset..copy_end - offset]);
        device
            .write_sector(lba as u64, &sector)
            .map_err(|_| TransferServiceError::Device)?;
    }
    Ok(())
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

fn read_bootstate_for_root(
    device: &mut BlockDevice,
    generation_root: [u8; 32],
) -> Result<(u8, BootState), ServiceError> {
    let a = read_range(device, 0, SLOT_BYTES)?;
    let b = read_range(device, SLOT_BYTES, SLOT_BYTES)?;
    let a: &[u8; SLOT_BYTES] = a.as_slice().try_into().unwrap();
    let b: &[u8; SLOT_BYTES] = b.as_slice().try_into().unwrap();
    let a = BootState::decode(a)
        .ok()
        .filter(|state| state.generation_root == generation_root);
    let b = BootState::decode(b)
        .ok()
        .filter(|state| state.generation_root == generation_root);
    match (a, b) {
        (Some(a), Some(b)) if a.sequence > b.sequence => Ok((0, a)),
        (Some(a), Some(b)) if b.sequence > a.sequence => Ok((1, b)),
        (Some(a), Some(b)) if a == b => Ok((0, a)),
        (Some(_), Some(_)) => Err(ServiceError::BadClosure),
        (Some(a), None) => Ok((0, a)),
        (None, Some(b)) => Ok((1, b)),
        (None, None) => Err(ServiceError::BadClosure),
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
        previous = identity;
        entries.push(DirectoryEntry {
            identity,
            generation_offset,
            generation_len,
            release_offset,
        });
    }
    Ok(entries)
}

fn directory_root(entries: &[DirectoryEntry]) -> [u8; 32] {
    let mut root = Sha256::new();
    for entry in entries {
        root.update(&entry.identity);
    }
    root.finalize()
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
