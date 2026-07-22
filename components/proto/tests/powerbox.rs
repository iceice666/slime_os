use slime_proto::{
    powerbox::{self, WirePowerboxReply, WirePowerboxRequest},
    valid_powerbox_reply, valid_powerbox_request,
};

const RIGHT_DIRECTORY_READ: u32 = 1 << 19;

fn request() -> WirePowerboxRequest {
    let purpose = b"Open selected note";
    let mut encoded_purpose = [0u8; powerbox::MAX_PURPOSE_BYTES];
    encoded_purpose[..purpose.len()].copy_from_slice(purpose);
    WirePowerboxRequest {
        magic: powerbox::POWERBOX_MAGIC,
        version: powerbox::FORMAT_VERSION,
        object_kind: powerbox::OBJECT_KIND_FILE,
        reserved0: 0,
        purpose_len: purpose.len() as u16,
        requested_rights: RIGHT_DIRECTORY_READ,
        purpose: encoded_purpose,
        reserved: [0; 8],
    }
}

#[test]
fn request_round_trips_byte_identically() {
    let request = request();
    assert!(valid_powerbox_request(&request));
    let encoded = request.encode();
    assert_eq!(WirePowerboxRequest::decode(&encoded), Some(request));
    assert_eq!(
        WirePowerboxRequest::decode(&encoded).unwrap().encode(),
        encoded
    );
    assert!(WirePowerboxRequest::decode(&encoded[..powerbox::REQUEST_LEN - 1]).is_none());
}

#[test]
fn request_validation_rejects_unknown_or_ambiguous_authority() {
    let base = request();
    assert!(!valid_powerbox_request(&WirePowerboxRequest {
        version: powerbox::FORMAT_VERSION + 1,
        ..base
    }));
    assert!(!valid_powerbox_request(&WirePowerboxRequest {
        object_kind: 9,
        ..base
    }));
    assert!(!valid_powerbox_request(&WirePowerboxRequest {
        requested_rights: 0,
        ..base
    }));
    assert!(!valid_powerbox_request(&WirePowerboxRequest {
        purpose_len: 0,
        ..base
    }));
    let mut trailing = base;
    trailing.purpose[trailing.purpose_len as usize] = b'x';
    assert!(!valid_powerbox_request(&trailing));
}

#[test]
fn replies_distinguish_selection_cancellation_and_failure() {
    let mut selected_path = [0u8; 16];
    selected_path[..4].copy_from_slice(b"note");
    let mut purpose = [0u8; 16];
    purpose[..4].copy_from_slice(b"open");
    let selected = WirePowerboxReply {
        magic: powerbox::POWERBOX_MAGIC,
        version: powerbox::FORMAT_VERSION,
        status: 0,
        flags: powerbox::REPLY_FLAG_SELECTED,
        object_kind: powerbox::OBJECT_KIND_FILE,
        purpose_len: 4,
        reserved0: 0,
        granted_rights: RIGHT_DIRECTORY_READ,
        event_id: 1,
        selected_path,
        purpose,
    };
    assert!(valid_powerbox_reply(&selected));
    assert_eq!(
        WirePowerboxReply::decode(&selected.encode()),
        Some(selected)
    );

    let cancelled = WirePowerboxReply {
        flags: powerbox::REPLY_FLAG_CANCELLED,
        granted_rights: 0,
        event_id: 0,
        selected_path: [0; 16],
        ..selected
    };
    assert!(valid_powerbox_reply(&cancelled));
    assert!(!valid_powerbox_reply(&WirePowerboxReply {
        granted_rights: RIGHT_DIRECTORY_READ,
        ..cancelled
    }));

    let failed = WirePowerboxReply {
        status: -1,
        flags: 0,
        granted_rights: 0,
        event_id: 0,
        selected_path: [0; 16],
        ..selected
    };
    assert!(valid_powerbox_reply(&failed));
    assert!(WirePowerboxReply::decode(&failed.encode()[..powerbox::REPLY_LEN - 1]).is_none());
}
