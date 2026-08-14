const INITIAL: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    length: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub const fn new() -> Self {
        Self {
            state: INITIAL,
            buffer: [0; 64],
            buffered: 0,
            length: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.length = self.length.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let take = (64 - self.buffered).min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered == 64 {
                compress(&mut self.state, &self.buffer);
                self.buffered = 0;
            }
            if data.is_empty() {
                return;
            }
        }
        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            compress(
                &mut self.state,
                chunk.try_into().expect("SHA-256 chunk size"),
            );
        }
        let remainder = chunks.remainder();
        self.buffer[..remainder.len()].copy_from_slice(remainder);
        self.buffered = remainder.len();
    }

    pub fn finalize(mut self) -> [u8; 32] {
        let bit_len = self.length.wrapping_mul(8);
        self.buffer[self.buffered] = 0x80;
        self.buffered += 1;
        if self.buffered > 56 {
            self.buffer[self.buffered..].fill(0);
            compress(&mut self.state, &self.buffer);
            self.buffer = [0; 64];
        } else {
            self.buffer[self.buffered..56].fill(0);
        }
        self.buffer[56..].copy_from_slice(&bit_len.to_be_bytes());
        compress(&mut self.state, &self.buffer);
        let mut result = [0u8; 32];
        for (dst, word) in result.chunks_exact_mut(4).zip(self.state) {
            dst.copy_from_slice(&word.to_be_bytes());
        }
        result
    }
}

pub fn digest(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize()
}

fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    for (word, bytes) in w.iter_mut().take(16).zip(block.chunks_exact(4)) {
        *word = u32::from_be_bytes(bytes.try_into().expect("SHA-256 word size"));
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = *state;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let choice = (e & f) ^ ((!e) & g);
        let temp1 = h
            .wrapping_add(s1)
            .wrapping_add(choice)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let majority = (a & b) ^ (a & c) ^ (b & c);
        let temp2 = s0.wrapping_add(majority);
        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(temp1);
        d = c;
        c = b;
        b = a;
        a = temp1.wrapping_add(temp2);
    }
    for (dst, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
        *dst = dst.wrapping_add(value);
    }
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
