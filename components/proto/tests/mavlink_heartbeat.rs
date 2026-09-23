use slime_proto::{
    mavlink::{encode_heartbeat, x25_crc, x25_crc_update},
    mavlink_heartbeat::{
        CHECKSUM_LEN, CRC_EXTRA, FRAME_LEN, HEADER_LEN, MAVLINK_VERSION, PAYLOAD_LEN, STX,
        VECTOR_SEQ_0, VECTOR_SEQ_1, VECTOR_SEQ_255, WireHeartbeat,
    },
};

#[test]
fn the_checksum_matches_the_published_check_value() {
    assert_eq!(x25_crc(b"123456789"), 0x6F91);
}

#[test]
fn the_encoder_reproduces_every_pinned_vector() {
    assert_eq!(encode_heartbeat(0), VECTOR_SEQ_0);
    assert_eq!(encode_heartbeat(1), VECTOR_SEQ_1);
    assert_eq!(encode_heartbeat(255), VECTOR_SEQ_255);
}

#[test]
fn a_frame_round_trips_through_the_generated_codec() {
    for seq in [0u8, 1, 127, 254, 255] {
        let bytes = encode_heartbeat(seq);
        let frame = WireHeartbeat::decode(&bytes).expect("a whole frame decodes");
        assert_eq!(frame.stx, STX);
        assert_eq!(frame.payload_len as usize, PAYLOAD_LEN);
        assert_eq!(frame.seq, seq);
        assert_eq!(frame.mavlink_version, MAVLINK_VERSION);
        assert_eq!(frame.encode(), bytes);
    }
}

#[test]
fn the_layout_is_header_payload_checksum_and_a_short_buffer_is_refused() {
    assert_eq!(FRAME_LEN, HEADER_LEN + PAYLOAD_LEN + CHECKSUM_LEN);
    assert!(WireHeartbeat::decode(&VECTOR_SEQ_0[..FRAME_LEN - 1]).is_none());
}

#[test]
fn the_checksum_covers_the_payload_and_the_extra_byte() {
    let frame = encode_heartbeat(7);
    let sent = u16::from_le_bytes([frame[FRAME_LEN - 2], frame[FRAME_LEN - 1]]);
    let covered = x25_crc(&frame[1..FRAME_LEN - CHECKSUM_LEN]);
    assert_eq!(x25_crc_update(covered, &[CRC_EXTRA]), sent);
    assert_ne!(covered, sent, "the extra byte must be folded in");

    let mut altered = frame;
    altered[HEADER_LEN] ^= 1;
    let altered_covered = x25_crc(&altered[1..FRAME_LEN - CHECKSUM_LEN]);
    assert_ne!(x25_crc_update(altered_covered, &[CRC_EXTRA]), sent);
}
