//! Encoding the MAVLink v2 HEARTBEAT frame.
//!
//! The contract in `contracts/mavlink-heartbeat/v1/` owns the frame's layout,
//! identity, and pinned vectors; this module owns the checksum arithmetic and
//! the one frame a Slime heartbeat sends, which is not a format and so is not
//! rendered. `scripts/lib/mavlink.py` implements the same arithmetic for the
//! checkers, and both must reproduce the contract's vectors.

use crate::mavlink_heartbeat::{
    AUTOPILOT, BASE_MODE, CHECKSUM_LEN, COMP_ID, CRC_EXTRA, CRC_INIT, CUSTOM_MODE, FRAME_LEN,
    MAV_TYPE, MAVLINK_VERSION, MSG_ID, PAYLOAD_LEN, STX, SYS_ID, SYSTEM_STATUS, WireHeartbeat,
};

/// Fold `bytes` into a running CRC-16/MCRF4XX (MAVLink's X.25 checksum).
///
/// Start from [`CRC_INIT`]; no final inversion.
pub const fn x25_crc_update(mut crc: u16, bytes: &[u8]) -> u16 {
    let mut index = 0;
    while index < bytes.len() {
        let mut scratch = bytes[index] ^ (crc as u8);
        scratch ^= scratch << 4;
        let scratch = scratch as u16;
        crc = (crc >> 8) ^ (scratch << 8) ^ (scratch << 3) ^ (scratch >> 4);
        index += 1;
    }
    crc
}

/// CRC-16/MCRF4XX over `bytes` from [`CRC_INIT`].
pub const fn x25_crc(bytes: &[u8]) -> u16 {
    x25_crc_update(CRC_INIT, bytes)
}

/// The declared heartbeat at sequence `seq`, ready for the wire.
pub fn encode_heartbeat(seq: u8) -> [u8; FRAME_LEN] {
    let msgid = MSG_ID.to_le_bytes();
    let mut frame = WireHeartbeat {
        stx: STX,
        payload_len: PAYLOAD_LEN as u8,
        incompat_flags: 0,
        compat_flags: 0,
        seq,
        sysid: SYS_ID,
        compid: COMP_ID,
        msgid: [msgid[0], msgid[1], msgid[2]],
        custom_mode: CUSTOM_MODE,
        mav_type: MAV_TYPE,
        autopilot: AUTOPILOT,
        base_mode: BASE_MODE,
        system_status: SYSTEM_STATUS,
        mavlink_version: MAVLINK_VERSION,
        checksum: 0,
    }
    .encode();
    let covered = x25_crc(&frame[1..FRAME_LEN - CHECKSUM_LEN]);
    let checksum = x25_crc_update(covered, &[CRC_EXTRA]).to_le_bytes();
    frame[FRAME_LEN - CHECKSUM_LEN..].copy_from_slice(&checksum);
    frame
}
