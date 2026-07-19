//! IEEE 802.3 CRC-32 (reflected, polynomial 0xEDB88320).
//!
//! M5.4 uses CRC-32 for the GPT header and partition-entry-array checksums.
//! The table is built at compile time; `crc32` is total over byte slices and
//! never panics.

const POLY: u32 = 0xEDB8_8320;

const fn build_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 != 0 {
                (value >> 1) ^ POLY
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const TABLE: [u32; 256] = build_table();

/// CRC-32 of `data` with the standard initial/final XOR (zlib semantics).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc = TABLE[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::crc32;

    #[test_case]
    fn check_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test_case]
    fn empty_input() {
        assert_eq!(crc32(b""), 0);
    }
}
