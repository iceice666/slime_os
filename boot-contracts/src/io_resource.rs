//! Generation-authenticated per-driver hardware-resource budgets.
use crate::sha256::Sha256;
pub const MAGIC: [u8; 8] = *b"SLIMEIO\0";
include!("generated/io_resource.rs");
pub const MAX_BYTES: usize = HEADER_BYTES + MAX_DRIVERS * ENTRY_BYTES;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    /// Two driver instances named the same device ordinal (B84).
    DuplicateDevice,
    Impossible,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DriverQuota {
    pub driver_identity: [u8; 32],
    pub mmio_bytes: u32,
    pub mmio_mappings: u32,
    pub dma_pages: u32,
    pub dma_mappings: u32,
    pub irq_sources: u32,
    pub outstanding_requests: u32,
    pub buffer_loans: u32,
    /// Which attached transport this driver instance drives, zero-based in the
    /// platform's stable device order (B84). Declared, never inferred: a plane
    /// with two disks declares one driver executable twice, and nothing in the
    /// instance's grants distinguishes the two.
    pub device: u32,
}
#[derive(Debug, Clone, Copy)]
pub struct IoResourceBudget<'a> {
    bytes: &'a [u8],
    driver_count: usize,
}
impl<'a> IoResourceBudget<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        if bytes.len() < HEADER_BYTES || bytes.len() > MAX_BYTES {
            return Err(DecodeError::Truncated);
        }
        if bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END] != MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if u32_at(bytes, OFF_HEADER_FORMAT_VERSION)? != FORMAT_VERSION
            || u32_at(bytes, OFF_HEADER_HEADER_SIZE)? as usize != HEADER_BYTES
        {
            return Err(DecodeError::UnsupportedVersion);
        }
        if u64_at(bytes, OFF_HEADER_REQUIRED_FLAGS)? != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let driver_count = u32_at(bytes, OFF_HEADER_DRIVER_COUNT)? as usize;
        let total_len = u32_at(bytes, OFF_HEADER_TOTAL_LEN)? as usize;
        if driver_count > MAX_DRIVERS
            || total_len != HEADER_BYTES + driver_count * ENTRY_BYTES
            || total_len != bytes.len()
        {
            return Err(DecodeError::BadBounds);
        }
        let mut previous = [0u8; 32];
        for index in 0..driver_count {
            let entry = decode_entry(bytes, index)?;
            if entry.driver_identity == [0; 32] || (index > 0 && entry.driver_identity <= previous)
            {
                return Err(DecodeError::BadOrder);
            }
            previous = entry.driver_identity;
            // Two driver instances naming one transport is refused here rather
            // than left to whichever installs second (B84). A device is
            // exclusive: two drivers programming one queue would each observe
            // the other's completions, and the root's own `WrongDevice` check
            // cannot see it because each driver's record is individually
            // coherent. Quadratic over at most `MAX_DRIVERS`, and only at
            // admission.
            for earlier in 0..index {
                if decode_entry(bytes, earlier)?.device == entry.device {
                    return Err(DecodeError::DuplicateDevice);
                }
            }
        }
        Ok(Self {
            bytes,
            driver_count,
        })
    }
    pub const fn driver_count(&self) -> usize {
        self.driver_count
    }
    pub fn driver(&self, index: usize) -> Option<DriverQuota> {
        (index < self.driver_count)
            .then(|| decode_entry(self.bytes, index).expect("validated IO-resource entry"))
    }
    pub fn quota_for(&self, identity: &[u8; 32]) -> Option<DriverQuota> {
        (0..self.driver_count)
            .filter_map(|index| self.driver(index))
            .find(|entry| entry.driver_identity == *identity)
    }
    pub fn validate_against(&self, maxima: DriverQuota) -> Result<(), DecodeError> {
        let mut totals = [0u32; 7];
        for index in 0..self.driver_count {
            let q = self.driver(index).expect("validated entry");
            if q.mmio_bytes > maxima.mmio_bytes
                || q.mmio_mappings > maxima.mmio_mappings
                || q.dma_pages > maxima.dma_pages
                || q.dma_mappings > maxima.dma_mappings
                || q.irq_sources > maxima.irq_sources
                || q.outstanding_requests > maxima.outstanding_requests
                || q.buffer_loans > maxima.buffer_loans
                || q.mmio_mappings > q.mmio_bytes
                || q.dma_mappings > q.dma_pages
                // `maxima.device` is the count of device slots the root can
                // back, so an ordinal at or above it names a transport that
                // cannot exist. Refused at admission rather than at first
                // MMIO: a driver that boots and then cannot find its device
                // has already announced readiness.
                || q.device >= maxima.device
            {
                return Err(DecodeError::Impossible);
            }
            for (total, value) in totals.iter_mut().zip([
                q.mmio_bytes,
                q.mmio_mappings,
                q.dma_pages,
                q.dma_mappings,
                q.irq_sources,
                q.outstanding_requests,
                q.buffer_loans,
            ]) {
                *total = total.saturating_add(value);
            }
        }
        if totals[0] > maxima.mmio_bytes
            || totals[1] > maxima.mmio_mappings
            || totals[2] > maxima.dma_pages
            || totals[3] > maxima.dma_mappings
            || totals[4] > maxima.irq_sources
            || totals[5] > maxima.outstanding_requests
            || totals[6] > maxima.buffer_loans
        {
            return Err(DecodeError::Impossible);
        }
        Ok(())
    }
}
pub fn driver_identity(name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"slime-io-resource-driver-v1");
    hasher.update(&(name.len() as u16).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.finalize()
}
fn decode_entry(bytes: &[u8], index: usize) -> Result<DriverQuota, DecodeError> {
    let offset = HEADER_BYTES + index * ENTRY_BYTES;
    let entry = bytes
        .get(offset..offset + ENTRY_BYTES)
        .ok_or(DecodeError::Truncated)?;
    Ok(DriverQuota {
        driver_identity: entry[OFF_ENTRY_DRIVER_IDENTITY..OFF_ENTRY_DRIVER_IDENTITY_END]
            .try_into()
            .expect("generated IO-resource layout"),
        mmio_bytes: u32_at(entry, OFF_ENTRY_MMIO_BYTES)?,
        mmio_mappings: u32_at(entry, OFF_ENTRY_MMIO_MAPPINGS)?,
        dma_pages: u32_at(entry, OFF_ENTRY_DMA_PAGES)?,
        dma_mappings: u32_at(entry, OFF_ENTRY_DMA_MAPPINGS)?,
        irq_sources: u32_at(entry, OFF_ENTRY_IRQ_SOURCES)?,
        outstanding_requests: u32_at(entry, OFF_ENTRY_OUTSTANDING_REQUESTS)?,
        buffer_loans: u32_at(entry, OFF_ENTRY_BUFFER_LOANS)?,
        device: u32_at(entry, OFF_ENTRY_DEVICE)?,
    })
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, DecodeError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(DecodeError::Truncated)?
            .try_into()
            .unwrap(),
    ))
}
#[cfg(test)]
mod tests {
    extern crate alloc;
    use super::*;
    /// `device` follows `id` so distinct drivers get distinct transports, which
    /// is what a valid table looks like; `at` overrides it where a test is
    /// specifically about the device ordinal.
    fn quota(id: u8) -> DriverQuota {
        at(id, u32::from(id))
    }
    fn at(id: u8, device: u32) -> DriverQuota {
        DriverQuota {
            driver_identity: [id; 32],
            mmio_bytes: 4096,
            mmio_mappings: 1,
            dma_pages: 4,
            dma_mappings: 1,
            irq_sources: 1,
            outstanding_requests: 2,
            buffer_loans: 2,
            device,
        }
    }
    fn build(entries: &[DriverQuota]) -> alloc::vec::Vec<u8> {
        let total = HEADER_BYTES + entries.len() * ENTRY_BYTES;
        let mut bytes = alloc::vec![0u8;total];
        bytes[OFF_HEADER_MAGIC..OFF_HEADER_MAGIC_END].copy_from_slice(&MAGIC);
        bytes[OFF_HEADER_FORMAT_VERSION..OFF_HEADER_FORMAT_VERSION_END]
            .copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        bytes[OFF_HEADER_HEADER_SIZE..OFF_HEADER_HEADER_SIZE_END]
            .copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
        bytes[OFF_HEADER_DRIVER_COUNT..OFF_HEADER_DRIVER_COUNT_END]
            .copy_from_slice(&(entries.len() as u32).to_le_bytes());
        bytes[OFF_HEADER_TOTAL_LEN..OFF_HEADER_TOTAL_LEN_END]
            .copy_from_slice(&(total as u32).to_le_bytes());
        for (index, q) in entries.iter().enumerate() {
            let o = HEADER_BYTES + index * ENTRY_BYTES;
            bytes[o + OFF_ENTRY_DRIVER_IDENTITY..o + OFF_ENTRY_DRIVER_IDENTITY_END]
                .copy_from_slice(&q.driver_identity);
            for (start, end, value) in [
                (OFF_ENTRY_MMIO_BYTES, OFF_ENTRY_MMIO_BYTES_END, q.mmio_bytes),
                (
                    OFF_ENTRY_MMIO_MAPPINGS,
                    OFF_ENTRY_MMIO_MAPPINGS_END,
                    q.mmio_mappings,
                ),
                (OFF_ENTRY_DMA_PAGES, OFF_ENTRY_DMA_PAGES_END, q.dma_pages),
                (
                    OFF_ENTRY_DMA_MAPPINGS,
                    OFF_ENTRY_DMA_MAPPINGS_END,
                    q.dma_mappings,
                ),
                (
                    OFF_ENTRY_IRQ_SOURCES,
                    OFF_ENTRY_IRQ_SOURCES_END,
                    q.irq_sources,
                ),
                (
                    OFF_ENTRY_OUTSTANDING_REQUESTS,
                    OFF_ENTRY_OUTSTANDING_REQUESTS_END,
                    q.outstanding_requests,
                ),
                (
                    OFF_ENTRY_BUFFER_LOANS,
                    OFF_ENTRY_BUFFER_LOANS_END,
                    q.buffer_loans,
                ),
                (OFF_ENTRY_DEVICE, OFF_ENTRY_DEVICE_END, q.device),
            ] {
                bytes[o + start..o + end].copy_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }
    #[test]
    fn decodes_and_looks_up() {
        let a = quota(1);
        let b = quota(2);
        let bytes = build(&[a, b]);
        let budget = IoResourceBudget::decode(&bytes).unwrap();
        assert_eq!(budget.driver_count(), 2);
        assert_eq!(budget.quota_for(&[2; 32]), Some(b));
    }
    #[test]
    fn malformed_fails() {
        assert_eq!(
            IoResourceBudget::decode(&build(&[quota(2), quota(1)])).unwrap_err(),
            DecodeError::BadOrder
        );
        let mut bytes = build(&[quota(1)]);
        bytes[OFF_HEADER_DRIVER_COUNT..OFF_HEADER_DRIVER_COUNT_END]
            .copy_from_slice(&2u32.to_le_bytes());
        assert_eq!(
            IoResourceBudget::decode(&bytes).unwrap_err(),
            DecodeError::BadBounds
        );
    }
    #[test]
    fn aggregate_overcommit_fails() {
        let bytes = build(&[quota(1), quota(2)]);
        let budget = IoResourceBudget::decode(&bytes).unwrap();
        assert_eq!(
            budget.validate_against(quota(9)),
            Err(DecodeError::Impossible)
        );
        assert!(
            IoResourceBudget::decode(&build(&[quota(1)]))
                .unwrap()
                .validate_against(quota(9))
                .is_ok()
        );
    }

    /// B84: a transport is exclusive. Two driver instances naming one device
    /// would each program the same queue and observe the other's completions,
    /// and neither record is individually incoherent — so the table itself has
    /// to refuse the pair.
    #[test]
    fn two_drivers_cannot_name_one_device() {
        assert_eq!(
            IoResourceBudget::decode(&build(&[at(1, 0), at(2, 0)])).unwrap_err(),
            DecodeError::DuplicateDevice
        );
        assert!(IoResourceBudget::decode(&build(&[at(1, 0), at(2, 1)])).is_ok());
    }

    /// An ordinal at or above the root's device count names a transport that
    /// cannot exist. Refused at admission, not at the driver's first MMIO: a
    /// driver that boots and then cannot find its device has already announced
    /// readiness.
    #[test]
    fn a_device_beyond_the_platform_is_refused() {
        let two_devices = at(9, 2);
        assert_eq!(
            IoResourceBudget::decode(&build(&[at(1, 2)]))
                .unwrap()
                .validate_against(two_devices),
            Err(DecodeError::Impossible)
        );
        assert!(
            IoResourceBudget::decode(&build(&[at(1, 1)]))
                .unwrap()
                .validate_against(two_devices)
                .is_ok()
        );
    }
}
