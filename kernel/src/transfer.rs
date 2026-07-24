use alloc::{vec, vec::Vec};

use boot_contracts::generation::{
    Generation, POLICY_IMMUTABLE, POLICY_PRESERVE, POLICY_SNAPSHOT_BEFORE_UPGRADE,
};
use boot_contracts::release::{INITIAL_TRUST_ROOT, Release};
use boot_contracts::transfer::{
    MAX_TRANSFER_BYTES, STATE_FLAG_READ_ONLY, STATE_FLAG_TRAVEL, TransferManifest,
};

use crate::block_device::BlockDevice;
use crate::block_proto::SECTOR_SIZE;
use crate::capability::PciFunctionInfo;
use crate::generation_service;
use crate::sha256::Sha256;

const BUNDLE_HEADER_SECTOR: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferError {
    Device,
    BadManifest,
    BadClosure,
    BadRelease,
    Conflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferResult {
    pub generation: [u8; 32],
    pub copied_objects: u32,
    pub state_count: u32,
    pub release_sequence: u64,
    pub remaining_attempts: u32,
}

pub fn receive(
    receiver_function: PciFunctionInfo,
    transfer_function: PciFunctionInfo,
) -> Result<TransferResult, TransferError> {
    if receiver_function == transfer_function {
        return Err(TransferError::Conflict);
    }
    let mut transfer = BlockDevice::init(transfer_function).map_err(|_| TransferError::Device)?;
    let header = read_range(&mut transfer, 0, BUNDLE_HEADER_SECTOR)?;
    if header[..8] != boot_contracts::transfer::MAGIC {
        return Err(TransferError::BadManifest);
    }
    let total_len = u64::from_le_bytes(
        header[boot_contracts::transfer::HEADER_TOTAL_LEN_OFFSET
            ..boot_contracts::transfer::HEADER_TOTAL_LEN_OFFSET + 8]
            .try_into()
            .map_err(|_| TransferError::BadManifest)?,
    ) as usize;
    if !(boot_contracts::transfer::HEADER_LEN..=MAX_TRANSFER_BYTES).contains(&total_len)
        || !total_len.is_multiple_of(SECTOR_SIZE)
    {
        return Err(TransferError::BadManifest);
    }
    let bytes = read_range(&mut transfer, 0, total_len)?;
    let manifest = TransferManifest::decode(&bytes).map_err(|_| TransferError::BadManifest)?;
    drop(transfer);
    let mut receiver = BlockDevice::init(receiver_function).map_err(|_| TransferError::Device)?;
    let mut receiver_entries =
        generation_service::read_entries_for_transfer(&mut receiver).map_err(map_service_error)?;
    let (selected_slot, state) =
        generation_service::read_state_for_transfer(&mut receiver).map_err(map_service_error)?;
    let generation_bytes = reconstruct_generation(&manifest, &mut receiver, &receiver_entries)?;
    let generation =
        Generation::decode(&generation_bytes).map_err(|_| TransferError::BadClosure)?;
    if generation.identity != manifest.generation {
        return Err(TransferError::BadClosure);
    }
    if generation.parent != manifest.parent {
        return Err(TransferError::BadClosure);
    }
    if generation.authority_manifest_identity() != manifest.authority_manifest {
        return Err(TransferError::BadClosure);
    }
    let release = Release::decode(manifest.release()).map_err(|_| TransferError::BadRelease)?;
    release
        .verify_for_staging(
            &generation,
            &INITIAL_TRUST_ROOT,
            state.accepted_release_sequence,
        )
        .map_err(|_| TransferError::BadRelease)?;
    if release.sequence != manifest.release_sequence {
        return Err(TransferError::BadRelease);
    }
    validate_states(&manifest, &generation)?;
    if receiver_entries
        .iter()
        .any(|entry| entry.identity == generation.identity)
    {
        return Err(TransferError::Conflict);
    }
    let copied_objects = manifest
        .objects()
        .filter(|object| object.is_ok_and(|object| object.payload.is_some()))
        .count() as u32;
    let installed = generation_service::install_and_select_for_transfer(
        &mut receiver,
        &mut receiver_entries,
        selected_slot,
        state,
        &generation_bytes,
        manifest.release(),
        manifest.source_state_root,
    )
    .map_err(map_service_error)?;
    drop(receiver);
    Ok(TransferResult {
        generation: generation.identity,
        copied_objects,
        state_count: manifest.state_count() as u32,
        release_sequence: release.sequence,
        remaining_attempts: installed.remaining_attempts,
    })
}

fn reconstruct_generation(
    manifest: &TransferManifest<'_>,
    receiver: &mut BlockDevice,
    entries: &[generation_service::TransferDirectoryEntry],
) -> Result<Vec<u8>, TransferError> {
    if manifest.generation_len < manifest.metadata().len()
        || manifest.generation_len > boot_contracts::generation::MAX_GENERATION_BYTES
    {
        return Err(TransferError::BadClosure);
    }
    let mut generation = vec![0; manifest.generation_len];
    generation[..manifest.metadata().len()].copy_from_slice(manifest.metadata());
    let mut payload_cursor = manifest.metadata().len();
    let mut objects = Vec::with_capacity(manifest.object_count());
    for index in 0..manifest.object_count() {
        let object = manifest
            .object(index)
            .map_err(|_| TransferError::BadClosure)?;
        if object.length > boot_contracts::generation::MAX_OBJECT_PAYLOAD_BYTES {
            return Err(TransferError::BadClosure);
        }
        let payload = match object.payload {
            Some(payload) => payload.to_vec(),
            None => generation_service::read_object_by_digest_for_transfer(
                receiver,
                entries,
                object.digest,
                object.length,
            )
            .map_err(map_service_error)?,
        };
        if payload.len() != object.length || crate::sha256::digest(&payload) != object.digest {
            return Err(TransferError::BadClosure);
        }
        let record =
            boot_contracts::generation::HEADER_LEN + index * boot_contracts::generation::OBJECT_LEN;
        let offset_end = record.checked_add(16).ok_or(TransferError::BadClosure)?;
        let source_offset = u64::from_le_bytes(
            generation
                .get(record + 8..offset_end)
                .ok_or(TransferError::BadClosure)?
                .try_into()
                .map_err(|_| TransferError::BadClosure)?,
        ) as usize;
        objects.push((source_offset, payload));
    }
    objects.sort_unstable_by_key(|(source_offset, _)| *source_offset);
    for (source_offset, payload) in objects {
        let end = source_offset
            .checked_add(payload.len())
            .ok_or(TransferError::BadClosure)?;
        if source_offset != payload_cursor || end > generation.len() {
            return Err(TransferError::BadClosure);
        }
        generation[source_offset..end].copy_from_slice(&payload);
        payload_cursor = end;
    }
    if payload_cursor != generation.len() {
        return Err(TransferError::BadClosure);
    }
    Ok(generation)
}

fn validate_states(
    manifest: &TransferManifest<'_>,
    generation: &Generation<'_>,
) -> Result<(), TransferError> {
    let mut expected = Vec::new();
    for index in 0..generation.state_count() {
        let state = generation
            .state(index)
            .map_err(|_| TransferError::BadClosure)?;
        if matches!(
            state.policy,
            POLICY_IMMUTABLE | POLICY_PRESERVE | POLICY_SNAPSHOT_BEFORE_UPGRADE
        ) {
            expected.push((
                binding_identity(state.name),
                state.schema_version,
                state.policy,
            ));
        }
    }
    if expected.len() != manifest.state_count() {
        return Err(TransferError::BadClosure);
    }
    for (index, (binding, schema, policy)) in expected.into_iter().enumerate() {
        let state = manifest
            .state(index)
            .map_err(|_| TransferError::BadClosure)?;
        let expected_flags = STATE_FLAG_TRAVEL
            | if policy == POLICY_IMMUTABLE {
                STATE_FLAG_READ_ONLY
            } else {
                0
            };
        if state.binding != binding
            || state.schema_version != schema
            || state.policy != policy
            || state.flags != expected_flags
            || state.state_root != manifest.source_state_root
        {
            return Err(TransferError::BadClosure);
        }
    }
    Ok(())
}

fn binding_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-state-binding-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}

fn read_range(
    device: &mut BlockDevice,
    offset: usize,
    len: usize,
) -> Result<Vec<u8>, TransferError> {
    let end = offset.checked_add(len).ok_or(TransferError::BadManifest)?;
    if end > device.capacity_sectors() as usize * SECTOR_SIZE {
        return Err(TransferError::BadManifest);
    }
    let mut out = vec![0; len];
    let mut sector = [0; SECTOR_SIZE];
    for lba in offset / SECTOR_SIZE..end.div_ceil(SECTOR_SIZE) {
        device
            .read_sector(lba as u64, &mut sector)
            .map_err(|_| TransferError::Device)?;
        let sector_start = lba * SECTOR_SIZE;
        let copy_start = offset.max(sector_start);
        let copy_end = end.min(sector_start + SECTOR_SIZE);
        out[copy_start - offset..copy_end - offset]
            .copy_from_slice(&sector[copy_start - sector_start..copy_end - sector_start]);
    }
    Ok(out)
}

fn map_service_error(error: generation_service::TransferServiceError) -> TransferError {
    match error {
        generation_service::TransferServiceError::BadRelease => TransferError::BadRelease,
        generation_service::TransferServiceError::Conflict => TransferError::Conflict,
        generation_service::TransferServiceError::Device => TransferError::Device,
        generation_service::TransferServiceError::BadClosure
        | generation_service::TransferServiceError::NotFound => TransferError::BadClosure,
    }
}
