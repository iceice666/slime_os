//! HMAC-DRBG over SHA-256 (NIST SP 800-90A Rev. 1, section 10.1.2).
//!
//! Byte-for-byte the construction `scripts/lib/hmac_drbg.py` computes on the
//! host: a seeded holder's stream is judged against that reference, so the two
//! must not diverge. No additional input and no prediction resistance; reseed
//! is the specification's update with fresh entropy.

use boot_contracts::sha256::Sha256;

pub const OUTLEN: usize = 32;
/// The minimum entropy input: SHA-256's 256-bit security strength.
pub const MIN_ENTROPY: usize = 32;
/// The largest single request SP 800-90A permits for this construction.
pub const MAX_REQUEST: usize = 7500;
/// Prefixed to a holder's name to form its personalization string.
pub const PERSONALIZATION_PREFIX: &[u8] = b"slime-entropy/v1:";

const BLOCK: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DrbgError {
    ShortEntropy,
    BadLength,
}

pub struct HmacDrbg {
    key: [u8; OUTLEN],
    value: [u8; OUTLEN],
}

impl HmacDrbg {
    pub fn instantiate(
        entropy: &[u8],
        nonce: &[u8],
        personalization: &[u8],
    ) -> Result<Self, DrbgError> {
        if entropy.len() < MIN_ENTROPY {
            return Err(DrbgError::ShortEntropy);
        }
        let mut drbg = Self {
            key: [0; OUTLEN],
            value: [1; OUTLEN],
        };
        drbg.update([entropy, nonce, personalization]);
        Ok(drbg)
    }

    pub fn reseed(&mut self, entropy: &[u8]) -> Result<(), DrbgError> {
        if entropy.len() < MIN_ENTROPY {
            return Err(DrbgError::ShortEntropy);
        }
        self.update([entropy, &[], &[]]);
        Ok(())
    }

    /// Fill `out` (1..=`MAX_REQUEST` bytes) and advance the state.
    pub fn generate(&mut self, out: &mut [u8]) -> Result<(), DrbgError> {
        if out.is_empty() || out.len() > MAX_REQUEST {
            return Err(DrbgError::BadLength);
        }
        for chunk in out.chunks_mut(OUTLEN) {
            self.value = hmac(&self.key, &[&self.value]);
            chunk.copy_from_slice(&self.value[..chunk.len()]);
        }
        self.update([&[]; 3]);
        Ok(())
    }

    /// `provided` is the concatenation of its parts; empty when every part is.
    fn update(&mut self, provided: [&[u8]; 3]) {
        let empty = provided.iter().all(|part| part.is_empty());
        for tag in [[0x00u8], [0x01]] {
            if tag[0] == 0x01 && empty {
                break;
            }
            let value = self.value;
            self.key = hmac(
                &self.key,
                &[&value, &tag, provided[0], provided[1], provided[2]],
            );
            self.value = hmac(&self.key, &[&self.value]);
        }
    }
}

/// HMAC-SHA-256 with a 32-byte key over the concatenation of `parts`.
fn hmac(key: &[u8; OUTLEN], parts: &[&[u8]]) -> [u8; OUTLEN] {
    let mut pad = [0u8; BLOCK];
    pad[..OUTLEN].copy_from_slice(key);
    let mut inner = Sha256::new();
    inner.update(&pad.map(|byte| byte ^ 0x36));
    for part in parts {
        inner.update(part);
    }
    let inner = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(&pad.map(|byte| byte ^ 0x5c));
    outer.update(&inner);
    outer.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
        }
        out
    }

    fn personalization(holder: &str) -> ([u8; 64], usize) {
        let mut out = [0u8; 64];
        let len = PERSONALIZATION_PREFIX.len() + holder.len();
        out[..PERSONALIZATION_PREFIX.len()].copy_from_slice(PERSONALIZATION_PREFIX);
        out[PERSONALIZATION_PREFIX.len()..len].copy_from_slice(holder.as_bytes());
        (out, len)
    }

    /// NIST CAVP HMAC_DRBG.rsp, [SHA-256], no prediction resistance, COUNT = 0:
    /// the vector `scripts/lib/hmac_drbg.py` self-tests against.
    #[test]
    fn the_nist_cavp_vector_is_reproduced() {
        let entropy = hex("ca851911349384bffe89de1cbdc46e6831e44d34a4fb935ee285dd14b71a7488");
        let nonce = [
            0x65, 0x9b, 0xa9, 0x6c, 0x60, 0x1d, 0xc6, 0x9f, 0xc9, 0x02, 0x94, 0x08, 0x05, 0xec,
            0x0c, 0xa8,
        ];
        let mut drbg = HmacDrbg::instantiate(&entropy, &nonce, &[]).unwrap();
        let mut out = [0u8; 128];
        drbg.generate(&mut out).unwrap();
        drbg.generate(&mut out).unwrap();
        let expected = [
            hex("e528e9abf2dece54d47c7e75e5fe302149f817ea9fb4bee6f4199697d04d5b89"),
            hex("d54fbb978a15b5c443c9ec21036d2460b6f73ebad0dc2aba6e624abf07745bc1"),
            hex("07694bb7547bb0995f70de25d6b29e2d3011bb19d27676c07162c8b5ccde0668"),
            hex("961df86803482cb37ed6d5c0bb8d50cf1f50d476aa0458bdaba806f48be9dcb8"),
        ];
        for (chunk, expected) in out.chunks(32).zip(expected) {
            assert_eq!(chunk, expected);
        }
    }

    /// `hmac_drbg.seeded_stream(bytes(range(32)), "entropy-seeded", 3, 32)`.
    #[test]
    fn a_seeded_stream_matches_the_host_reference() {
        let seed: [u8; 32] = core::array::from_fn(|index| index as u8);
        let (personalization, len) = personalization("entropy-seeded");
        let mut drbg = HmacDrbg::instantiate(&seed, &[], &personalization[..len]).unwrap();
        for expected in [
            "37725b4330ccc627332bd71638fc8be7bfe7bc9059a48d98af0a720036161375",
            "12a724d0b3ed600999cc34a20eb5b788b5959c68a5250b26431caf408837deab",
            "8e0ba73803821de6e12acd74666f976796eea5c176196724bcb82cc586338807",
        ] {
            let mut block = [0u8; 32];
            drbg.generate(&mut block).unwrap();
            assert_eq!(block, hex(expected));
        }
    }

    /// The reference's `HmacDrbg(entropy, nonce, personalization)`, one
    /// generate, `_update(0xaa * 32)`, and a second generate.
    #[test]
    fn a_nonce_and_a_reseed_match_the_host_reference() {
        let entropy: [u8; 32] = core::array::from_fn(|index| index as u8);
        let nonce: [u8; 16] = core::array::from_fn(|index| 100 + index as u8);
        let (personalization, len) = personalization("entropy-hw-a");
        let mut drbg = HmacDrbg::instantiate(&entropy, &nonce, &personalization[..len]).unwrap();
        let mut block = [0u8; 32];
        drbg.generate(&mut block).unwrap();
        assert_eq!(
            block,
            hex("3f0debdc94928c71e647cdbd405106aa3780b41d1176e459c76cc5e2d5f1cc6c")
        );
        drbg.reseed(&[0xaa; 32]).unwrap();
        drbg.generate(&mut block).unwrap();
        assert_eq!(
            block,
            hex("2bbc04dfb88c71a1afeff93f27e7636f90eeebf88a407b7d2163b5f6cbfc58a9")
        );
    }

    #[test]
    fn short_entropy_and_out_of_range_requests_are_refused() {
        assert_eq!(
            HmacDrbg::instantiate(&[0; 31], &[], &[]).err(),
            Some(DrbgError::ShortEntropy)
        );
        let mut drbg = HmacDrbg::instantiate(&[0; 32], &[], &[]).unwrap();
        assert_eq!(drbg.reseed(&[0; 16]), Err(DrbgError::ShortEntropy));
        assert_eq!(drbg.generate(&mut []), Err(DrbgError::BadLength));
        assert_eq!(
            drbg.generate(&mut [0u8; MAX_REQUEST + 1]),
            Err(DrbgError::BadLength)
        );
    }
}
