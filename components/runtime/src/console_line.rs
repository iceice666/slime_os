//! One console record assembled in a bounded buffer and submitted whole.
//!
//! The console dispatcher prints each `debug_write` call as one record and never
//! joins fragments, so a line built from several calls can be split by a root
//! line. A [`Line`] collects the whole record first and submits it in a single
//! call. Every component links this crate, so this is the record builder a
//! component uses without depending on `slime-components`.

/// The largest record the console dispatcher accepts in one write.
pub const MAX_RECORD_BYTES: usize = 1024;

/// A diagnostic record assembled before its single `debug_write`.
///
/// A record that does not fit `N` bytes, or is not UTF-8, is refused whole:
/// nothing is written, so a reader never sees a fragment as a complete record.
/// `N` may not exceed [`MAX_RECORD_BYTES`].
pub struct Line<const N: usize> {
    bytes: [u8; N],
    len: usize,
    overflowed: bool,
}

impl<const N: usize> Line<N> {
    pub const fn new() -> Self {
        const {
            assert!(
                N <= MAX_RECORD_BYTES,
                "Line capacity exceeds the console record bound"
            )
        };
        Self {
            bytes: [0; N],
            len: 0,
            overflowed: false,
        }
    }

    /// Append raw bytes; overflow marks the whole record refused.
    pub fn bytes(&mut self, part: &[u8]) -> &mut Self {
        match self
            .len
            .checked_add(part.len())
            .and_then(|end| self.bytes.get_mut(self.len..end))
        {
            Some(slot) => {
                slot.copy_from_slice(part);
                self.len += part.len();
            }
            None => self.overflowed = true,
        }
        self
    }

    /// Append an unsigned decimal number.
    pub fn decimal(&mut self, mut value: u64) -> &mut Self {
        let mut digits = [0u8; 20];
        let mut offset = digits.len();
        loop {
            offset -= 1;
            digits[offset] = b'0' + (value % 10) as u8;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        self.bytes(&digits[offset..])
    }

    /// Append a signed decimal number, with `-` for a negative value.
    pub fn signed(&mut self, value: i64) -> &mut Self {
        if value < 0 {
            self.bytes(b"-");
        }
        self.decimal(value.unsigned_abs())
    }

    /// Append lowercase hexadecimal digits without prefix or padding.
    pub fn hex(&mut self, mut value: u64) -> &mut Self {
        let mut digits = [0u8; 16];
        let mut offset = digits.len();
        loop {
            offset -= 1;
            digits[offset] = b"0123456789abcdef"[(value & 0xf) as usize];
            value >>= 4;
            if value == 0 {
                break;
            }
        }
        self.bytes(&digits[offset..])
    }

    /// Append two lowercase hexadecimal digits per byte.
    pub fn hex_bytes(&mut self, bytes: &[u8]) -> &mut Self {
        for byte in bytes {
            self.bytes(&[
                b"0123456789abcdef"[usize::from(byte >> 4)],
                b"0123456789abcdef"[usize::from(byte & 0xf)],
            ]);
        }
        self
    }

    /// Submit the record in one call, returning whether it was submitted.
    pub fn emit(&self) -> bool {
        let record = &self.bytes[..self.len];
        if self.overflowed
            || record.len() > MAX_RECORD_BYTES
            || core::str::from_utf8(record).is_err()
        {
            return false;
        }
        crate::debug_write(record) >= 0
    }
}

impl<const N: usize> Default for Line<N> {
    fn default() -> Self {
        Self::new()
    }
}
