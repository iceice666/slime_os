//! Pure encoding helpers for the native seL4 transport.
//!
//! A Slime operation crosses the root service endpoint as a message label (the
//! operation number) plus at most [`FAST_REGISTERS`] fast message registers.
//! Bytes and capability slots that do not fit in those registers travel through
//! the caller's startup transfer window; they are never silently truncated.
//! These helpers do the packing and hold no transport state, so they can be
//! reasoned about — and tested — on their own.

/// Fast message registers available to one request or one reply.
pub const FAST_REGISTERS: usize = 4;

/// Payload bytes carried inline by the two payload registers (`MR2`, `MR3`).
pub const INLINE_BYTES: usize = 16;

/// Payload bytes ride in `MR2`/`MR3`. Only valid when no capability crosses.
pub const FORM_INLINE: u64 = 0;
/// Payload bytes and capability slots ride in the bound transfer window.
pub const FORM_WINDOW: u64 = 1;

/// Largest byte count a transfer descriptor can name.
pub const MAX_DESCRIPTOR_LEN: usize = 0xffff;

/// Largest capability count a staged transfer may name. The descriptor field is
/// wider, but the transport contract admits only [`super::MAX_CAPS_PER_MSG`].
pub const MAX_DESCRIPTOR_CAPS: usize = super::MAX_CAPS_PER_MSG;

/// Builds the transfer descriptor register: payload byte count, capability
/// count, which carrier holds them, and the sending thread's window index.
///
/// The thread index is invocation metadata, not authority. The root already
/// authenticated the process from the endpoint badge and uses this field only
/// to select one of the windows it mapped for that process (B47/B46).
pub const fn descriptor(len: usize, caps: usize, form: u64, thread: usize) -> u64 {
    debug_assert!(len <= MAX_DESCRIPTOR_LEN);
    debug_assert!(caps <= MAX_DESCRIPTOR_CAPS);
    (len as u64) | ((caps as u64) << 16) | (form << 24) | ((thread as u64) << 32)
}

/// Payload byte count named by a transfer descriptor.
pub const fn descriptor_len(descriptor: u64) -> usize {
    (descriptor & 0xffff) as usize
}

/// Capability count named by a transfer descriptor.
pub const fn descriptor_caps(descriptor: u64) -> usize {
    ((descriptor >> 16) & 0xff) as usize
}

/// Payload carrier named by a transfer descriptor.
pub const fn descriptor_form(descriptor: u64) -> u64 {
    (descriptor >> 24) & 0xff
}

#[cfg(test)]
/// Thread index whose transfer window carries this frame.
pub const fn descriptor_thread(descriptor: u64) -> usize {
    (descriptor >> 32) as usize
}

/// True when `len` bytes and `caps` capabilities fit in the fast registers.
pub const fn fits_inline(len: usize, caps: usize) -> bool {
    len <= INLINE_BYTES && caps == 0
}

/// Packs two capability slots into one register, so operations naming a source
/// and a destination still fit their arguments in the fast registers.
pub const fn slot_pair(first: u32, second: u32) -> u64 {
    (first as u64) | ((second as u64) << 32)
}

/// Packs a capability slot with one boolean flag, for the operations whose
/// argument list is one word wider than the fast registers allow.
pub const fn slot_with_flag(slot: u32, flag: bool) -> u64 {
    (slot as u64) | ((flag as u64) << 32)
}

/// Byte offset of the capability-slot vector within a transfer-window frame
/// whose payload is `len` bytes. Slots are word-aligned so the root service
/// reads them without an unaligned access.
pub const fn frame_caps_offset(len: usize) -> usize {
    len.next_multiple_of(8)
}

/// Total transfer-window bytes a frame of `len` payload bytes and `caps`
/// capability slots occupies.
pub const fn frame_len(len: usize, caps: usize) -> usize {
    frame_caps_offset(len) + caps * 8
}

/// Packs at most [`INLINE_BYTES`] payload bytes into the two payload registers,
/// zero-padding the tail. Longer input never reaches here: the caller has
/// already routed it to the transfer window.
pub fn pack_bytes(bytes: &[u8]) -> [u64; 2] {
    debug_assert!(bytes.len() <= INLINE_BYTES);
    let mut padded = [0u8; INLINE_BYTES];
    let taken = bytes.len().min(INLINE_BYTES);
    padded[..taken].copy_from_slice(&bytes[..taken]);
    let (low, high) = padded.split_at(8);
    [
        u64::from_le_bytes(low.try_into().unwrap()),
        u64::from_le_bytes(high.try_into().unwrap()),
    ]
}

/// Clears the capability slots a reply did not name, so a short reply cannot
/// leave a stale handle visible to the caller.
pub fn clear_unnamed_slots(slots: &mut [u64], named: usize) {
    for slot in slots.iter_mut().skip(named) {
        *slot = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_cap_bound_matches_transport_contract() {
        assert_eq!(MAX_DESCRIPTOR_CAPS, crate::MAX_CAPS_PER_MSG);
        let encoded = descriptor(7, MAX_DESCRIPTOR_CAPS, FORM_WINDOW, 1);
        assert_eq!(descriptor_len(encoded), 7);
        assert_eq!(descriptor_caps(encoded), crate::MAX_CAPS_PER_MSG);
        assert_eq!(descriptor_thread(encoded), 1);
    }

    #[test]
    fn inline_fit_never_accepts_capabilities() {
        assert!(fits_inline(INLINE_BYTES, 0));
        assert!(!fits_inline(0, 1));
        assert!(!fits_inline(INLINE_BYTES + 1, 0));
    }
}
