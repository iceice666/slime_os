use slime_proto::private_memory_probe::{self as protocol, WireMessage};

#[test]
fn coordination_and_loan_messages_round_trip() {
    for operation in [
        protocol::GROW,
        protocol::VERIFY,
        protocol::FAULT,
        protocol::FINISH,
        protocol::ACKNOWLEDGE,
        protocol::READ_FOREIGN,
        protocol::WRITE_FOREIGN,
        protocol::EXECUTE_PRIVATE,
        protocol::LOAN_READY,
        protocol::LOAN_OFFER,
        protocol::LOAN_SETTLED,
        protocol::STRESS_GROW,
        protocol::STRESS_REFUSE,
        protocol::STRESS_VERIFY,
    ] {
        let message = WireMessage {
            version: protocol::FORMAT_VERSION,
            operation,
            holder: 2,
            incarnation: 20,
            address: 0x0123_4567_89ab_cdef,
            reserved: [0; 40],
        };
        let encoded = message.encode();
        // Delegation carries exactly one native capability-transfer descriptor.
        let _: &[u8; 64] = &encoded;
        assert_eq!(encoded.len(), protocol::MESSAGE_LEN);
        assert_eq!(WireMessage::decode(&encoded), Some(message));
        assert_eq!(WireMessage::decode(&encoded).unwrap().encode(), encoded);
    }
}

#[test]
fn every_truncated_message_is_refused() {
    let encoded = WireMessage {
        version: protocol::FORMAT_VERSION,
        operation: protocol::LOAN_OFFER,
        holder: 1,
        incarnation: 0,
        address: 0,
        reserved: [0; 40],
    }
    .encode();
    for length in 0..protocol::MESSAGE_LEN {
        assert!(WireMessage::decode(&encoded[..length]).is_none());
    }
}
