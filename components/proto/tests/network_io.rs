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
std::thread_local! {
    static IPC: std::cell::RefCell<Option<(u8, usize, usize)>> = const { std::cell::RefCell::new(None) };
    static RETURNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CLOCK: std::cell::Cell<Option<(u64, u64)>> = const { std::cell::Cell::new(None) };
}

pub fn test_clock(now: Option<(u64, u64)>) {
    CLOCK.with(|clock| clock.set(now));
}

pub fn monotonic_read() -> Result<u64, UnexpectedSyscall> {
    CLOCK.with(|clock| {
        let (now, step) = clock.get().expect("unexpected clock read");
        clock.set(Some((now + step, step)));
        Ok(now)
    })
}
pub fn timer_cancel(_: u64) -> i64 {
    panic!("unexpected timer cancellation")
}
pub fn timer_arm(_: u64) -> Result<u64, UnexpectedSyscall> {
    panic!("unexpected timer arm")
}
pub fn test_ipc(op: u8, pending: usize) {
    IPC.with(|ipc| *ipc.borrow_mut() = Some((op, pending, 0)));
    RETURNS.with(|count| count.set(0));
}
pub fn test_ipc_counts() -> (usize, usize) {
    (
        IPC.with(|ipc| ipc.borrow().as_ref().unwrap().2),
        RETURNS.with(|count| count.get()),
    )
}
pub fn recv(_: u32, bytes: &mut [u8], _: &mut [u64]) -> i64 {
    IPC.with(|ipc| {
        let mut ipc = ipc.borrow_mut();
        let (op, pending, _) = ipc.as_mut().expect("unexpected IPC receive");
        if *pending > 0 {
            *pending -= 1;
            return ERR_WOULDBLOCK;
        }
        use slime_proto::network_service as net;
        let reply = net::WireNetworkCompletion {
            magic: net::NETWORK_MAGIC,
            version: net::FORMAT_VERSION,
            op: *op,
            capability_kind: net::CAPABILITY_NONE,
            status_detail: net::STATUS_SUCCESS,
            flags: 0,
            capability: 0,
        }
        .encode();
        bytes[..reply.len()].copy_from_slice(&reply);
        reply.len() as i64
    })
}
pub fn send(_: u32, bytes: &[u8], _: &[u64]) -> i64 {
    IPC.with(|ipc| {
        let mut ipc = ipc.borrow_mut();
        let (op, _, sent) = ipc.as_mut().expect("unexpected IPC send");
        let request = slime_proto::network_service::WireNetworkRequest::decode(bytes).unwrap();
        assert_eq!(request.op, *op);
        assert_eq!(request.capability, u64::MAX);
        *sent += 1;
        ERR_SUCCESS
    })
}
pub fn yield_now() {}
pub fn capability_import() -> Result<u32, UnexpectedSyscall> {
    panic!("unexpected capability import")
}
pub fn shared_buffer_loan_map(_: u32, _: u64, _: u64, _: u64) -> i64 {
    panic!("unexpected loan mapping")
}
pub fn shared_buffer_return(slot: u32) -> i64 {
    RETURNS.with(|count| count.set(count.get() + 1));
    assert!(matches!(slot, 1 | 3));
    ERR_SUCCESS
}
