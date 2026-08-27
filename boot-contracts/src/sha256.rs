//! SHA-256 (FIPS 180-4), backed by RustCrypto's `sha2` crate rather than a
//! hand-written compression function. This module is the one place the rest
//! of the workspace names a hasher, so every caller keeps its streaming
//! `Sha256::{new,update,finalize}` shape and a plain `[u8; 32]` output with no
//! `GenericArray` in its public surface.

use sha2::{Digest, Sha256 as Inner};

/// SHA-256 streaming hasher.
pub struct Sha256(Inner);

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self(Inner::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }

    pub fn finalize(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

pub fn digest(data: &[u8]) -> [u8; 32] {
    Inner::digest(data).into()
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;

    fn hex(digest: [u8; 32]) -> alloc::string::String {
        use core::fmt::Write as _;
        let mut out = alloc::string::String::new();
        for byte in digest {
            write!(out, "{byte:02x}").expect("hex write");
        }
        out
    }

    /// Published FIPS 180-4 vectors. A hand-rolled compression function that is
    /// self-consistent but wrong would pass every other test in this crate,
    /// because everything else compares one of these digests against another.
    /// These are the only assertions anchoring the implementation to the real
    /// algorithm.
    #[test]
    fn nist_vectors_match() {
        assert_eq!(
            hex(digest(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(digest(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(digest(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    /// One million 'a' bytes, fed in awkward slices. This is the FIPS long
    /// vector and it exercises the 64-bit length counter as well as thousands of
    /// block boundaries; the odd chunk size keeps `update`'s buffered path live
    /// throughout rather than hitting only aligned blocks.
    #[test]
    fn the_long_nist_vector_matches_when_fed_unaligned() {
        let mut hasher = Sha256::new();
        let block = [b'a'; 7];
        let mut remaining = 1_000_000usize;
        while remaining > 0 {
            let take = block.len().min(remaining);
            hasher.update(&block[..take]);
            remaining -= take;
        }
        assert_eq!(
            hex(hasher.finalize()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// Streaming must equal one-shot for every split point across a
    /// multi-block input. `update`'s buffered branch, its `chunks_exact` fast
    /// path, and the handoff between them all depend on the split, and a
    /// mishandled boundary would show up here and nowhere else.
    #[test]
    fn every_split_point_agrees_with_one_shot() {
        let data: alloc::vec::Vec<u8> = (0..200u32).map(|index| index as u8).collect();
        let expected = digest(&data);
        for split in 0..=data.len() {
            let mut hasher = Sha256::new();
            hasher.update(&data[..split]);
            hasher.update(&data[split..]);
            assert_eq!(hasher.finalize(), expected, "split at {split}");
        }
    }

    /// Three splits, so the buffered path is entered with a partial block
    /// already held — the case a two-split test can miss.
    #[test]
    fn three_way_splits_agree_with_one_shot() {
        let data: alloc::vec::Vec<u8> = (0..160u32).map(|index| (index * 7) as u8).collect();
        let expected = digest(&data);
        for first in [1usize, 31, 63, 64, 65, 100] {
            for second in [1usize, 2, 63, 64] {
                if first + second > data.len() {
                    continue;
                }
                let mut hasher = Sha256::new();
                hasher.update(&data[..first]);
                hasher.update(&data[first..first + second]);
                hasher.update(&data[first + second..]);
                assert_eq!(hasher.finalize(), expected, "splits {first}/{second}");
            }
        }
    }

    /// The padding boundary. At 55 bytes the length field still fits the first
    /// block; at 56 it does not and `finalize` must emit a second block. Off by
    /// one here produces a plausible-looking digest for most inputs, so the
    /// lengths either side of the seam are asserted against known values.
    #[test]
    fn the_padding_boundary_is_handled_at_every_adjacent_length() {
        let expected = [
            // 55 bytes: padding and length share one block.
            (
                55usize,
                "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
            ),
            // 56 bytes: 0x80 lands at index 56, so a second block is required.
            (
                56,
                "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
            ),
            // Exactly one block, so `finalize` starts with an empty buffer.
            (
                64,
                "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
            ),
            // One block plus one byte.
            (
                65,
                "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
            ),
        ];
        for (length, want) in expected {
            let data = alloc::vec![b'a'; length];
            assert_eq!(hex(digest(&data)), want, "length {length}");
        }
    }

    /// An empty `update` must not disturb the state, or a caller feeding an
    /// empty slice between real ones would silently change the digest.
    #[test]
    fn empty_updates_are_transparent() {
        let mut hasher = Sha256::new();
        hasher.update(b"");
        hasher.update(b"ab");
        hasher.update(b"");
        hasher.update(b"c");
        hasher.update(b"");
        assert_eq!(hasher.finalize(), digest(b"abc"));
    }

    /// `Default` is derived from `new`, and both must produce the initial state
    /// rather than a zeroed one.
    #[test]
    fn default_matches_new() {
        let from_default = {
            let mut hasher = Sha256::default();
            hasher.update(b"abc");
            hasher.finalize()
        };
        assert_eq!(from_default, digest(b"abc"));
    }

    /// Digests must actually distinguish inputs, including ones differing only
    /// in a single bit or in length.
    #[test]
    fn distinct_inputs_give_distinct_digests() {
        assert_ne!(digest(b"abc"), digest(b"abc\x01"));
        assert_ne!(digest(b"abc"), digest(b"cba"));
        assert_ne!(digest(b"abc"), digest(b"abc\0"));
        assert_ne!(digest(b""), digest(b"\0"));
    }
}
