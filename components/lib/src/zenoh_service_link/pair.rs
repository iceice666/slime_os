//! The scripted byte streams and the pair of links the service-over-a-link exam drives.
use crate::zenoh_link::{ByteLink, LinkError, LinkOf};
use crate::zenoh_profile0::session::{Config, Params, Role};
use crate::zenoh_service::exam::params;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Default)]
pub struct Pipe {
    bytes: VecDeque<u8>,
}

pub struct Io {
    pub rx: Rc<RefCell<Pipe>>,
    pub tx: Rc<RefCell<Pipe>>,
}
impl ByteLink for Io {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        self.tx.borrow_mut().bytes.extend(buf.iter());
        Ok(buf.len())
    }
    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        let mut pipe = self.rx.borrow_mut();
        let mut n = 0;
        while n < buf.len() {
            match pipe.bytes.pop_front() {
                Some(b) => {
                    buf[n] = b;
                    n += 1;
                }
                None => break,
            }
        }
        Ok(n)
    }
    fn eof(&self) -> bool {
        false
    }
}

// The service's link: 5 slots (3 own + the peer's 2), 12-byte payload, 8 notices.
pub type Svc = LinkOf<Io, 514, 1, 5, 129, 12, 8, 4>;
// The peer's link: it takes 3 of the service's and declares 2 of its own.
pub type Far = LinkOf<Io, 514, 1, 5, 129, 12, 8, 4>;

pub fn cfg(zid: &'static [u8], cookie: &'static [u8], sn: u32) -> Config<'static> {
    Config {
        zid,
        lease_ms: 10_000,
        initial_sn: sn,
        cookie,
    }
}

pub fn pair() -> (
    Svc,
    Far,
    crate::zenoh_link::LinkHandle,
    crate::zenoh_link::LinkHandle,
) {
    let ab = Rc::new(RefCell::new(Pipe::default()));
    let ba = Rc::new(RefCell::new(Pipe::default()));
    let service_params = params();
    let peer_params = Params {
        local: 2,
        per_side: 3,
        ..params()
    };
    let mut svc = Svc::with_params(
        Io {
            rx: ab.clone(),
            tx: ba.clone(),
        },
        Role::Listener,
        &cfg(&[0xb2; 8], &[0xc0; 32], 7000),
        service_params,
        1,
    )
    .expect("service");
    let mut far = Far::with_params(
        Io { rx: ba, tx: ab },
        Role::Connector,
        &cfg(&[0xa1; 8], b"", 9000),
        peer_params,
        1,
    )
    .expect("peer");
    let (hs, hf) = (svc.handle(), far.handle());
    far.open(hf, 0).expect("open");
    for t in 1..=12u64 {
        far.pump(hf, t).expect("pump far");
        svc.pump(hs, t).expect("pump svc");
        if far.is_open() && svc.is_open() {
            break;
        }
    }
    assert!(far.is_open() && svc.is_open());
    (svc, far, hs, hf)
}

/// A stream that accepts every byte the link offers, and fails on the next read once armed.
pub struct FailAfterSend {
    pub inner: Io,
    pub armed: Rc<RefCell<bool>>,
    pub sent_since_armed: bool,
}

impl ByteLink for FailAfterSend {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        if *self.armed.borrow() {
            self.sent_since_armed = true;
        }
        self.inner.send(buf)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        if self.sent_since_armed {
            return Err(LinkError::Reset);
        }
        self.inner.recv(buf)
    }

    fn eof(&self) -> bool {
        false
    }
}

/// A stream that takes no byte while `stall` is set, so a batch the link queued stays in its send slot.
pub struct Stalling {
    pub inner: Io,
    pub stall: Rc<RefCell<bool>>,
}

impl ByteLink for Stalling {
    fn send(&mut self, buf: &[u8]) -> Result<usize, LinkError> {
        if *self.stall.borrow() {
            return Ok(0);
        }
        self.inner.send(buf)
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, LinkError> {
        self.inner.recv(buf)
    }

    fn eof(&self) -> bool {
        false
    }
}
