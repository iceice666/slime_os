//! Host regression of the actual Zenoh service client adapter with IO0 queues and no kernel calls.
//!
//! The adapter's own tests (`mod tests` in `components/lib/src/zenoh_service_client.rs`) run here, against the
//! stand-in runtime below. They print one `[zenoh-exam] clientio ...` line for each thing they did.
//! `scripts/lib/zenoh_client_io_exam.py` owns what those lines must be.
extern crate self as slime_rt;

#[allow(dead_code)]
#[path = "../../lib/src/zenoh_service_client.rs"]
mod zenoh_service_client;

use std::cell::{Cell, RefCell};

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_WOULDBLOCK: i64 = -2;
pub const MAX_CAPS_PER_MSG: usize = 4;
pub const MAX_MSG: usize = 128;

/// The control endpoint and the provisioning endpoint the tests pass to `attach`.
pub const CONTROL_SLOT: u32 = 5;
pub const PROVISION_SLOT: u32 = 6;

#[derive(Debug)]
pub struct UnexpectedSyscall;

std::thread_local! {
    static CONTROL: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static PROVISION: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
    static SENT: RefCell<Vec<(Vec<u8>, usize)>> = const { RefCell::new(Vec::new()) };
    static PENDING_AT_SEND: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    static RETURNS: Cell<usize> = const { Cell::new(0) };
    static IMPORTS: Cell<u32> = const { Cell::new(0) };
}

/// Script what each endpoint will say, in order, and clear what was recorded.
pub fn test_script(control: Vec<Vec<u8>>, provision: Vec<Vec<u8>>) {
    CONTROL.with(|queue| *queue.borrow_mut() = control);
    PROVISION.with(|queue| *queue.borrow_mut() = provision);
    SENT.with(|sent| sent.borrow_mut().clear());
    PENDING_AT_SEND.with(|pending| pending.borrow_mut().clear());
    RETURNS.with(|count| count.set(0));
    IMPORTS.with(|count| count.set(0));
}
/// The control requests the adapter sent, as bytes.
pub fn test_sent() -> Vec<Vec<u8>> {
    SENT.with(|sent| {
        sent.borrow()
            .iter()
            .map(|(bytes, _)| bytes.clone())
            .collect()
    })
}
/// How many loans had been returned when each control request was sent.
pub fn test_returns_at_send() -> Vec<usize> {
    SENT.with(|sent| {
        sent.borrow()
            .iter()
            .map(|(_, returned)| *returned)
            .collect()
    })
}
/// How many scripted control answers were unread when each control request was sent. A real send blocks until
/// the service receives, and the service sends the attach answer first, so a request sent while that answer
/// is unread would never be received.
pub fn test_pending_at_send() -> Vec<usize> {
    PENDING_AT_SEND.with(|pending| pending.borrow().clone())
}
/// How many scripted control answers the adapter left unread.
pub fn test_control_pending() -> usize {
    CONTROL.with(|queue| queue.borrow().len())
}
pub fn test_returns() -> usize {
    RETURNS.with(Cell::get)
}
pub fn test_imports() -> u32 {
    IMPORTS.with(Cell::get)
}

pub fn recv(slot: u32, bytes: &mut [u8], _: &mut [u64]) -> i64 {
    let take = |queue: &RefCell<Vec<Vec<u8>>>| {
        let mut queue = queue.borrow_mut();
        if queue.is_empty() {
            return ERR_WOULDBLOCK;
        }
        let next = queue.remove(0);
        bytes[..next.len()].copy_from_slice(&next);
        next.len() as i64
    };
    match slot {
        CONTROL_SLOT => CONTROL.with(take),
        PROVISION_SLOT => PROVISION.with(take),
        _ => panic!("unexpected endpoint {slot}"),
    }
}
pub fn send(slot: u32, bytes: &[u8], _: &[u64]) -> i64 {
    assert_eq!(
        slot, CONTROL_SLOT,
        "a control request goes to the control endpoint"
    );
    let returned = RETURNS.with(Cell::get);
    SENT.with(|sent| sent.borrow_mut().push((bytes.to_vec(), returned)));
    let pending = CONTROL.with(|queue| queue.borrow().len());
    PENDING_AT_SEND.with(|log| log.borrow_mut().push(pending));
    ERR_SUCCESS
}
pub fn yield_now() {}
pub fn capability_import() -> Result<u32, UnexpectedSyscall> {
    Ok(IMPORTS.with(|count| {
        count.set(count.get() + 1);
        count.get()
    }))
}
pub fn shared_buffer_loan_map(_: u32, _: u64, _: u64, _: u64) -> i64 {
    ERR_SUCCESS
}
pub fn shared_buffer_return(_: u32) -> i64 {
    RETURNS.with(|count| count.set(count.get() + 1));
    ERR_SUCCESS
}
