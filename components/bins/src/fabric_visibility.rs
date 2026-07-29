//! Blocking helpers shared by the C8.8 visibility-profile components.
//!
//! The service answers exactly one fixed-size record per cursor request. A
//! caller therefore owns no graph-sized buffer and cannot make the service grow
//! a response queue by refusing to read later pages.

use slime_proto::fabric_visibility::{
    FORMAT_VERSION, RECORD_LEN, STATUS_END, VISIBILITY_QOS_MAGIC, VISIBILITY_REQUEST_MAGIC,
    VISIBILITY_ROUTE_MAGIC, WireVisibilityQosRecord, WireVisibilityRequest,
    WireVisibilityRouteRecord,
};
use slime_proto::{
    valid_visibility_qos_record, valid_visibility_request, valid_visibility_route_record,
};
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, WaitSource};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Transport,
    InvalidRecord,
}

#[derive(Clone, Copy)]
pub enum ViewPage {
    Route(WireVisibilityRouteRecord),
    Qos(WireVisibilityQosRecord),
    End(WireVisibilityRouteRecord),
}

pub fn request_page(control_slot: u32, cursor: u8) -> Result<ViewPage, Error> {
    let request = WireVisibilityRequest {
        magic: VISIBILITY_REQUEST_MAGIC,
        version: FORMAT_VERSION,
        cursor,
        flags: 0,
        reserved: [0; 56],
    };
    if !valid_visibility_request(&request) {
        return Err(Error::InvalidRecord);
    }
    send(control_slot, &request.encode())?;

    let mut message = [0u8; MAX_MSG];
    let mut received = [0u64; MAX_CAPS_PER_MSG];
    let length = loop {
        match slime_rt::recv(control_slot, &mut message, &mut received) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(control_slot)]),
            n if n < 0 => return Err(Error::Transport),
            n => break n as usize,
        }
    };
    if length != RECORD_LEN || received.iter().any(|slot| *slot != 0) {
        for slot in received.into_iter().filter(|slot| *slot != 0) {
            let _ = slime_rt::cap_drop(slot as u32);
        }
        return Err(Error::InvalidRecord);
    }
    let magic = u32::from_le_bytes(message[..4].try_into().map_err(|_| Error::InvalidRecord)?);
    match magic {
        VISIBILITY_ROUTE_MAGIC => {
            let record = WireVisibilityRouteRecord::decode(&message).ok_or(Error::InvalidRecord)?;
            if !valid_visibility_route_record(&record) {
                return Err(Error::InvalidRecord);
            }
            if record.status == STATUS_END {
                Ok(ViewPage::End(record))
            } else {
                Ok(ViewPage::Route(record))
            }
        }
        VISIBILITY_QOS_MAGIC => {
            let record = WireVisibilityQosRecord::decode(&message).ok_or(Error::InvalidRecord)?;
            valid_visibility_qos_record(&record)
                .then_some(ViewPage::Qos(record))
                .ok_or(Error::InvalidRecord)
        }
        _ => Err(Error::InvalidRecord),
    }
}

pub fn send(control_slot: u32, message: &[u8; RECORD_LEN]) -> Result<(), Error> {
    loop {
        match slime_rt::send(control_slot, message, &[]) {
            ERR_SUCCESS => return Ok(()),
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(control_slot)]),
            _ => return Err(Error::Transport),
        }
    }
}

const _: () = assert!(RECORD_LEN == MAX_MSG);
