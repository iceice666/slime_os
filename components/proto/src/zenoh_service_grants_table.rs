//! The Zenoh service grants table (`contracts/zenoh-service-grants-table/v1`).
//!
//! A table is a header and fixed-size rows, sorted by holder name and then key, with each pair once. `decode` checks
//! every byte the layout defines, padding and reserved bytes included, and returns a view over the input.

pub use crate::zenoh_service_grants_table_wire::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadMagic,
    UnsupportedVersion,
    UnknownRequiredFlags,
    BadBounds,
    BadOrder,
    InvalidRow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table<'a> {
    bytes: &'a [u8],
    row_count: usize,
}

/// One row, borrowed from the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a> {
    pub holder: &'a [u8],
    pub key: &'a [u8],
    pub publish: bool,
    pub subscribe: bool,
}

fn holder_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
}

fn key_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'_' | b'-' | b':' | b'.' | b'~')
}

fn zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

impl<'a> Table<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, DecodeError> {
        let header = WireGrantsHeader::decode(bytes).ok_or(DecodeError::Truncated)?;
        if header.magic != TABLE_MAGIC {
            return Err(DecodeError::BadMagic);
        }
        if header.format_version != FORMAT_VERSION {
            return Err(DecodeError::UnsupportedVersion);
        }
        if header.header_size as usize != HEADER_BYTES {
            return Err(DecodeError::BadBounds);
        }
        if header.required_flags != 0 {
            return Err(DecodeError::UnknownRequiredFlags);
        }
        let row_count = header.row_count as usize;
        if row_count == 0 || row_count > MAX_ROWS {
            return Err(DecodeError::BadBounds);
        }
        let total = HEADER_BYTES + row_count * ENTRY_BYTES;
        if header.total_len as usize != total || bytes.len() != total {
            return Err(DecodeError::BadBounds);
        }
        let table = Self { bytes, row_count };
        let mut previous: Option<Row<'a>> = None;
        for index in 0..row_count {
            let row = table.row(index)?;
            if let Some(before) = previous
                && (before.holder, before.key) >= (row.holder, row.key)
            {
                return Err(DecodeError::BadOrder);
            }
            previous = Some(row);
        }
        Ok(table)
    }

    pub fn row_count(&self) -> usize {
        self.row_count
    }

    /// The bytes of row `index`, as the root copies them out to a reader.
    pub fn row_bytes(&self, index: usize) -> Option<&'a [u8]> {
        if index >= self.row_count {
            return None;
        }
        let start = HEADER_BYTES + index * ENTRY_BYTES;
        self.bytes.get(start..start + ENTRY_BYTES)
    }

    /// Row `index`, validated again: a view built by `decode` has only valid rows.
    pub fn row(&self, index: usize) -> Result<Row<'a>, DecodeError> {
        let bytes = self.row_bytes(index).ok_or(DecodeError::BadBounds)?;
        Self::parse_row(bytes)
    }

    /// Validate one row's bytes. A reader that is handed rows a page at a time uses this on each.
    pub fn parse_row(bytes: &'a [u8]) -> Result<Row<'a>, DecodeError> {
        let entry = WireGrantsEntry::decode(bytes).ok_or(DecodeError::Truncated)?;
        let name_len = usize::from(entry.holder_name_len);
        let key_len = usize::from(entry.key_len);
        if name_len == 0 || name_len > HOLDER_NAME_BYTES || key_len == 0 || key_len > MAX_KEY_BYTES
        {
            return Err(DecodeError::InvalidRow);
        }
        let name = &bytes[OFF_ENTRY_HOLDER_NAME..OFF_ENTRY_HOLDER_NAME + name_len];
        let key = &bytes[OFF_ENTRY_KEY..OFF_ENTRY_KEY + key_len];
        let rights = u32::from(entry.rights);
        if !zero(&entry.holder_name[name_len..])
            || !zero(&entry.key[key_len..])
            || !zero(&entry.reserved)
            || rights == 0
            || rights & !KNOWN_RIGHTS != 0
            || !name.iter().all(|byte| holder_byte(*byte))
            || !key.iter().all(|byte| key_byte(*byte))
            || key.split(|byte| *byte == b'/').any(<[u8]>::is_empty)
        {
            return Err(DecodeError::InvalidRow);
        }
        Ok(Row {
            holder: name,
            key,
            publish: rights & RIGHT_PUBLISH != 0,
            subscribe: rights & RIGHT_SUBSCRIBE != 0,
        })
    }
}
