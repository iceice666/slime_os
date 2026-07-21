use alloc::collections::VecDeque;
use alloc::sync::{Arc, Weak};
use core::sync::atomic::AtomicBool;
use spin::Mutex;

use crate::capability::{Capability, CapabilityTable};

pub const MAX_MSG: usize = 64;
pub const MAX_CAPS_PER_MSG: usize = 4;
pub const CHANNEL_QUEUE: usize = 16;

pub const ERR_SUCCESS: i64 = 0;
pub const ERR_BAD_CAP: i64 = -1;
pub const ERR_PEER_DEAD: i64 = -2;
pub const ERR_WOULDBLOCK: i64 = -3;
pub const ERR_INVALID_ARG: i64 = -4;
pub const ERR_OUT_OF_MEMORY: i64 = -5;

#[derive(Clone)]
pub struct Message {
    pub bytes: [u8; MAX_MSG],
    pub len: usize,
    pub caps: [Option<Capability>; MAX_CAPS_PER_MSG],
}

pub struct EndpointInner {
    pub queue: Arc<Mutex<VecDeque<Message>>>,
    pub peer_queue: Weak<Mutex<VecDeque<Message>>>,
    pub owner_alive: Arc<AtomicBool>,
    pub peer_owner_alive: Weak<AtomicBool>,
}

pub struct Endpoint {
    inner: Arc<EndpointInner>,
    owner_alive: Arc<AtomicBool>,
}

impl Endpoint {
    pub fn inner(&self) -> &EndpointInner {
        &self.inner
    }
}

impl Clone for Endpoint {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            owner_alive: self.owner_alive.clone(),
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        if Arc::strong_count(&self.owner_alive) == 2 {
            self.owner_alive
                .store(false, core::sync::atomic::Ordering::Release);
        }
    }
}

pub fn channel() -> (Endpoint, Endpoint) {
    let a_alive = Arc::new(AtomicBool::new(true));
    let b_alive = Arc::new(AtomicBool::new(true));
    let a_queue = Arc::new(Mutex::new(VecDeque::new()));
    let b_queue = Arc::new(Mutex::new(VecDeque::new()));

    let a = Endpoint {
        inner: Arc::new(EndpointInner {
            queue: a_queue.clone(),
            peer_queue: Arc::downgrade(&b_queue),
            owner_alive: a_alive.clone(),
            peer_owner_alive: Arc::downgrade(&b_alive),
        }),
        owner_alive: a_alive.clone(),
    };
    let b = Endpoint {
        inner: Arc::new(EndpointInner {
            queue: b_queue,
            peer_queue: Arc::downgrade(&a_queue),
            owner_alive: b_alive.clone(),
            peer_owner_alive: Arc::downgrade(&a_alive),
        }),
        owner_alive: b_alive.clone(),
    };
    (a, b)
}

pub fn send(ep: &Endpoint, bytes: &[u8], caps: &mut [Option<Capability>; MAX_CAPS_PER_MSG]) -> i64 {
    let ep = ep.inner();
    if ep.peer_owner_alive.upgrade().is_none() {
        return ERR_PEER_DEAD;
    }
    let Some(peer_queue) = ep.peer_queue.upgrade() else {
        return ERR_PEER_DEAD;
    };

    let mut queue = peer_queue.lock();
    if queue.len() >= CHANNEL_QUEUE {
        return ERR_WOULDBLOCK;
    }

    let len = bytes.len().min(MAX_MSG);
    let mut msg = Message {
        bytes: [0; MAX_MSG],
        len,
        caps: core::array::from_fn(|_| None),
    };
    msg.bytes[..len].copy_from_slice(&bytes[..len]);
    for (dst, src) in msg.caps.iter_mut().zip(caps.iter_mut()) {
        *dst = src.take();
    }
    queue.push_back(msg);
    ERR_SUCCESS
}

pub fn recv(
    ep: &Endpoint,
    buf: &mut [u8],
    cap_out: &mut [u64; MAX_CAPS_PER_MSG],
    caps: &mut CapabilityTable,
) -> i64 {
    let ep = ep.inner();
    let mut queue = ep.queue.lock();
    if let Some(msg) = queue.front() {
        let cap_count = msg.caps.iter().filter(|cap| cap.is_some()).count();
        if caps.available_slots() < cap_count {
            return ERR_OUT_OF_MEMORY;
        }

        let mut msg = queue.pop_front().expect("front message disappeared");
        let len = msg.len.min(buf.len());
        buf[..len].copy_from_slice(&msg.bytes[..len]);
        for (i, cap) in msg.caps.iter_mut().enumerate() {
            cap_out[i] = 0;
            if let Some(cap) = cap.take() {
                cap_out[i] = caps
                    .insert(cap)
                    .expect("cap-table capacity changed after preflight")
                    as u64;
            }
        }
        return len as i64;
    }
    drop(queue);

    if ep.peer_owner_alive.upgrade().is_none() {
        ERR_PEER_DEAD
    } else {
        ERR_WOULDBLOCK
    }
}
