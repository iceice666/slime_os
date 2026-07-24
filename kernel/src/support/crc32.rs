pub use boot_contracts::crc32::crc32;

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
