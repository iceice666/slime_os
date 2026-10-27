use slime_proto::fabric_call::{
    CALL_MAGIC, FLAG_NON_IDEMPOTENT, FORMAT_VERSION, KIND_HELD, KIND_REPLY, STATUS_CANCELLED,
    STATUS_SUCCESS, WireCallEnvelope,
};
use slime_proto::valid_call_envelope;

const TYPE: u64 = 0x5eed;

fn held() -> WireCallEnvelope {
    WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_HELD,
        flags: 0,
        session: 7,
        request_id: 4,
        type_identity: TYPE,
        status: STATUS_SUCCESS,
        payload_len: 0,
        payload: [0; 16],
    }
}

#[test]
fn a_held_record_settles_nothing_and_carries_nothing() {
    assert!(valid_call_envelope(&held(), TYPE));
    let encoded = held().encode();
    assert_eq!(WireCallEnvelope::decode(&encoded), Some(held()));

    let mut outcome = held();
    outcome.status = STATUS_CANCELLED;
    assert!(!valid_call_envelope(&outcome, TYPE));

    let mut payload = held();
    payload.payload_len = 1;
    payload.payload[0] = 9;
    assert!(!valid_call_envelope(&payload, TYPE));

    let mut flagged = held();
    flagged.flags = FLAG_NON_IDEMPOTENT;
    assert!(!valid_call_envelope(&flagged, TYPE));

    // A reply with the same fields is still a reply, not a hold.
    let mut reply = held();
    reply.kind = KIND_REPLY;
    assert!(valid_call_envelope(&reply, TYPE));
}
