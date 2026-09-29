"""HMAC-DRBG over SHA-256 (NIST SP 800-90A Rev. 1, section 10.1.2).

The host's independent reference for the entropy service's seeded holders: a
seeded holder's stream must equal this instantiation from generation data
alone. Only the operations the service uses are provided: no additional input,
no prediction resistance, and no reseed of a seeded instance.
"""

from __future__ import annotations

import hashlib
import hmac

OUTLEN = 32
PERSONALIZATION_PREFIX = b"slime-entropy/v1:"


class HmacDrbg:
    def __init__(self, entropy: bytes, nonce: bytes = b"", personalization: bytes = b"") -> None:
        if len(entropy) < 32:
            raise ValueError("HMAC-DRBG(SHA-256) needs at least 256 bits of entropy input")
        self._key = b"\x00" * OUTLEN
        self._value = b"\x01" * OUTLEN
        self._update(entropy + nonce + personalization)

    def _hmac(self, data: bytes) -> bytes:
        return hmac.new(self._key, data, hashlib.sha256).digest()

    def _update(self, provided: bytes) -> None:
        self._key = self._hmac(self._value + b"\x00" + provided)
        self._value = self._hmac(self._value)
        if provided:
            self._key = self._hmac(self._value + b"\x01" + provided)
            self._value = self._hmac(self._value)

    def generate(self, length: int) -> bytes:
        if not 0 < length <= 7500:
            raise ValueError("HMAC-DRBG request outside 1..7500 bytes")
        output = b""
        while len(output) < length:
            self._value = self._hmac(self._value)
            output += self._value
        self._update(b"")
        return output[:length]


def seeded_stream(seed: bytes, holder: str, draws: int, size: int = 32) -> list[bytes]:
    """The blocks a seeded holder receives for `draws` requests of `size` bytes."""
    if len(seed) != 32:
        raise ValueError("a seeded holder's seed is exactly 32 bytes")
    drbg = HmacDrbg(seed, b"", PERSONALIZATION_PREFIX + holder.encode("ascii"))
    return [drbg.generate(size) for _ in range(draws)]


def self_test() -> None:
    """NIST CAVP HMAC_DRBG.rsp, [SHA-256], no prediction resistance, COUNT = 0."""
    drbg = HmacDrbg(
        bytes.fromhex("ca851911349384bffe89de1cbdc46e6831e44d34a4fb935ee285dd14b71a7488"),
        bytes.fromhex("659ba96c601dc69fc902940805ec0ca8"),
    )
    drbg.generate(128)
    expected = bytes.fromhex(
        "e528e9abf2dece54d47c7e75e5fe302149f817ea9fb4bee6f4199697d04d5b89"
        "d54fbb978a15b5c443c9ec21036d2460b6f73ebad0dc2aba6e624abf07745bc1"
        "07694bb7547bb0995f70de25d6b29e2d3011bb19d27676c07162c8b5ccde0668"
        "961df86803482cb37ed6d5c0bb8d50cf1f50d476aa0458bdaba806f48be9dcb8"
    )
    if drbg.generate(128) != expected:
        raise ValueError("HMAC-DRBG reference disagrees with the NIST CAVP vector")
