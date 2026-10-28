//! What the link-facing operations promise and the exam's transcript does not print. The judge fixes the
//! transcript, so these tests assert and print nothing.

use super::pair::{Far, Io, Pipe, Svc, cfg, pair};
use super::{declare_all, payload, peer_declares, step};
use crate::zenoh_profile0::session::{Params, Role};
use crate::zenoh_service::exam::{Exam, GID, KEYS, params, rows};
use crate::zenoh_service::{ServiceError, Settled};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn a_publication_that_finds_the_slot_taken_is_busy_whatever_else_is_wrong_with_it() {
    let (mut svc, mut far, hs, hf) = pair();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut now = 20u64;
    declare_all(&mut core, &mut svc, &mut far, hs, hf, &mut now, false);
    peer_declares(&mut svc, &mut far, hs, hf, &mut now);
    assert!(
        core.publish_over(&mut svc, hs, 0, KEYS[0], 100, 1100, &payload(100))
            .expect("granted")
            .is_none()
    );
    let before = (
        core.held(0).unwrap(),
        core.held(1).unwrap(),
        core.held(2).unwrap(),
    );
    let unknown = core.publish_over(&mut svc, hs, 3, KEYS[0], 7, 1007, &payload(7));
    let no_right = core.publish_over(&mut svc, hs, 1, KEYS[0], 7, 1007, &payload(7));
    let no_row = core.publish_over(&mut svc, hs, 2, KEYS[2], 7, 1007, &payload(7));
    let oversized = core.publish_over(&mut svc, hs, 0, KEYS[0], 7, 1007, &[0u8; 13]);
    for outcome in [unknown, no_right, no_row, oversized] {
        assert_eq!(outcome.expect_err("the slot is taken"), ServiceError::Busy);
    }
    assert_eq!(
        before,
        (
            core.held(0).unwrap(),
            core.held(1).unwrap(),
            core.held(2).unwrap()
        )
    );
    step(&mut svc, &mut far, hs, hf, &mut now);
    assert!(matches!(core.settle(&svc), Ok(Settled::Published { .. })));
    // With the slot free again the same invalid calls are refused for what is wrong with them.
    assert_eq!(
        core.publish_over(&mut svc, hs, 3, KEYS[0], 7, 1007, &payload(7))
            .expect_err("unknown"),
        ServiceError::UnknownHolder
    );
    assert_eq!(
        core.publish_over(&mut svc, hs, 1, KEYS[0], 7, 1007, &payload(7))
            .expect_err("no right"),
        ServiceError::NotGranted
    );
    assert_eq!(
        core.publish_over(&mut svc, hs, 0, KEYS[0], 7, 1007, &[0u8; 13])
            .expect_err("oversized"),
        ServiceError::TooLarge
    );
}

#[test]
fn a_link_declared_with_other_parameters_is_refused_before_anything_is_queued() {
    let ab = Rc::new(RefCell::new(Pipe::default()));
    let ba = Rc::new(RefCell::new(Pipe::default()));
    let other = Params {
        local: 1,
        per_side: 4,
        ..params()
    };
    let peer = Params {
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
        other,
        1,
    )
    .expect("service");
    let mut far = Far::with_params(
        Io { rx: ba, tx: ab },
        Role::Connector,
        &cfg(&[0xa1; 8], b"", 9000),
        peer,
        1,
    )
    .expect("peer");
    let (hs, hf) = (svc.handle(), far.handle());
    far.open(hf, 0).expect("open");
    let mut now = 0u64;
    for _ in 0..12 {
        step(&mut svc, &mut far, hs, hf, &mut now);
        if svc.is_open() && far.is_open() {
            break;
        }
    }
    assert!(svc.is_open() && far.is_open());
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    assert_eq!(
        core.declare_next_over(&mut svc, hs)
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
    assert_eq!(
        core.publish_over(&mut svc, hs, 0, KEYS[0], 7, 1007, &payload(7))
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
    assert!(!svc.send_pending());
    assert_eq!(core.declared(), 0);
}
