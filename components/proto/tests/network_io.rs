//! Host regression of the actual adapter with IO0 queues and no kernel calls.
extern crate self as slime_rt;

#[allow(dead_code)]
#[path = "../../lib/src/network_io.rs"]
mod network_io;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_WOULDBLOCK: i64 = -2;
pub const MAX_CAPS_PER_MSG: usize = 4;
pub const MAX_MSG: usize = 128;

#[derive(Debug)]
pub struct UnexpectedSyscall;

pub mod wait_set {
    pub enum Kind {
        Stream,
    }
}

pub struct WaitSet;

impl WaitSet {
    pub fn declared(_: u32) -> Result<Self, UnexpectedSyscall> {
        panic!("unexpected wait-set declaration")
    }
    pub fn declarations(&self) -> &[()] {
        panic!("unexpected wait-set declarations")
    }
    pub fn register_slot(&mut self, _: wait_set::Kind, _: u32) -> Result<(), UnexpectedSyscall> {
        panic!("unexpected wait-set registration")
    }
    pub fn register_timer(&mut self) -> Result<(), UnexpectedSyscall> {
        panic!("unexpected timer registration")
    }
    pub fn wait(&mut self) -> Result<(), UnexpectedSyscall> {
        panic!("unexpected wait")
    }
    pub fn next_ready(&mut self) -> Option<()> {
        panic!("unexpected readiness consumption")
    }
    pub fn wakes(&self) -> usize {
        panic!("unexpected wake count")
    }
}

pub fn notification_signal(_: u32) -> i64 {
    panic!("unexpected signal")
}
pub fn monotonic_read() -> Result<u64, UnexpectedSyscall> {
    panic!("unexpected clock read")
}
pub fn timer_cancel(_: u64) -> i64 {
    panic!("unexpected timer cancellation")
}
pub fn timer_arm(_: u64) -> Result<u64, UnexpectedSyscall> {
    panic!("unexpected timer arm")
}
pub fn recv(_: u32, _: &mut [u8], _: &mut [u64]) -> i64 {
    panic!("unexpected IPC receive")
}
pub fn send(_: u32, _: &[u8], _: &[u64]) -> i64 {
    panic!("unexpected IPC send")
}
pub fn yield_now() {}
pub fn capability_import() -> Result<u32, UnexpectedSyscall> {
    panic!("unexpected capability import")
}
pub fn shared_buffer_loan_map(_: u32, _: u64, _: u64, _: u64) -> i64 {
    panic!("unexpected loan mapping")
}
pub fn shared_buffer_return(slot: u32) -> i64 {
    assert!(matches!(slot, 1 | 3));
    ERR_SUCCESS
}
