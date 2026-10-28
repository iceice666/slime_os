//! What the link-facing operations promise and the exam's transcript does not print. The judge fixes the
//! transcript, so these tests assert and print nothing.

use super::pair::{Far, Io, Pipe, Svc, cfg, pair};
use super::{declare_all, payload, peer_declares, step};
use crate::zenoh_profile0::session::{Params, Role};
use crate::zenoh_service::exam::{Exam, GID, KEYS, params, rows};
use crate::zenoh_service::{OverLink, ServiceError, Settled};
use std::cell::RefCell;
use std::rc::Rc;

#[test]
fn a_publication_that_finds_the_slot_taken_is_busy_whatever_else_is_wrong_with_it() {
    let (mut svc, mut far, hs, hf) = pair();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut over = OverLink::<12>::new();
    let mut now = 20u64;
    declare_all(
        &mut core, &mut over, &mut svc, &mut far, hs, hf, &mut now, false,
    );
    peer_declares(&mut svc, &mut far, hs, hf, &mut now);
    assert!(
        over.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            100,
            1100,
            &payload(100)
        )
        .expect("granted")
        .is_none()
    );
    let before = (
        core.held(0).unwrap(),
        core.held(1).unwrap(),
        core.held(2).unwrap(),
    );
    let unknown = over.publish(&mut core, &mut svc, hs, 3, KEYS[0], 7, 1007, &payload(7));
    let no_right = over.publish(&mut core, &mut svc, hs, 1, KEYS[0], 7, 1007, &payload(7));
    let no_row = over.publish(&mut core, &mut svc, hs, 2, KEYS[2], 7, 1007, &payload(7));
    let oversized = over.publish(&mut core, &mut svc, hs, 0, KEYS[0], 7, 1007, &[0u8; 13]);
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
    assert!(matches!(
        over.settle(&mut core, &svc),
        Ok(Settled::Published { .. })
    ));
    // With the slot free again the same invalid calls are refused for what is wrong with them.
    assert_eq!(
        over.publish(&mut core, &mut svc, hs, 3, KEYS[0], 7, 1007, &payload(7))
            .expect_err("unknown"),
        ServiceError::UnknownHolder
    );
    assert_eq!(
        over.publish(&mut core, &mut svc, hs, 1, KEYS[0], 7, 1007, &payload(7))
            .expect_err("no right"),
        ServiceError::NotGranted
    );
    assert_eq!(
        over.publish(&mut core, &mut svc, hs, 0, KEYS[0], 7, 1007, &[0u8; 13])
            .expect_err("oversized"),
        ServiceError::TooLarge
    );
}

/// An open pair whose service side was declared with other parameters than the core's.
fn pair_of_other_parameters() -> (
    Svc,
    Far,
    crate::zenoh_link::LinkHandle,
    crate::zenoh_link::LinkHandle,
) {
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
    (svc, far, hs, hf)
}

#[test]
fn a_link_declared_with_other_parameters_is_refused_before_anything_is_queued() {
    let (mut svc, _far, hs, _hf) = pair_of_other_parameters();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut over = OverLink::<12>::new();
    assert_eq!(
        over.declare_next(&mut core, &mut svc, hs)
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
    assert_eq!(
        over.publish(&mut core, &mut svc, hs, 0, KEYS[0], 7, 1007, &payload(7))
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
    assert!(!svc.send_pending());
    assert_eq!(core.declared(), 0);
    // Nothing is left queued: the same calls are refused for the same reason and not as busy.
    assert!(over.is_idle());
    assert_eq!(
        over.declare_next(&mut core, &mut svc, hs)
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
    assert_eq!(
        over.publish(&mut core, &mut svc, hs, 0, KEYS[0], 7, 1007, &payload(7))
            .expect_err("other parameters"),
        ServiceError::WrongSession
    );
}

#[test]
fn settle_and_drain_refuse_a_link_of_other_parameters_and_change_nothing() {
    let (mut a, mut far, ha, hf) = pair();
    let (mut other, _far_other, _ho, _hfo) = pair_of_other_parameters();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut over = OverLink::<12>::new();
    let mut now = 20u64;
    declare_all(
        &mut core, &mut over, &mut a, &mut far, ha, hf, &mut now, false,
    );
    peer_declares(&mut a, &mut far, ha, hf, &mut now);
    assert!(
        over.publish(&mut core, &mut a, ha, 0, KEYS[0], 100, 1100, &payload(100))
            .expect("granted")
            .is_none()
    );
    let before = (core.held(0).unwrap(), core.held(1).unwrap());
    // The other link's slot is empty. Settling against it must not commit the copies of the frame on `a`.
    assert_eq!(
        over.settle(&mut core, &other),
        Err(ServiceError::WrongSession)
    );
    assert_eq!(
        core.drain_notices(&mut other),
        Err(ServiceError::WrongSession)
    );
    assert_eq!(before, (core.held(0).unwrap(), core.held(1).unwrap()));
    // The frame is still waiting on its own link, and settles there once the stream took it.
    assert_eq!(over.settle(&mut core, &a), Ok(Settled::Waiting));
    step(&mut a, &mut far, ha, hf, &mut now);
    assert_eq!(
        over.settle(&mut core, &a),
        Ok(Settled::Published { reached: 2 })
    );
}

#[test]
fn a_closed_link_drops_what_waited_even_when_a_later_pump_emptied_the_slot() {
    use super::Link;
    use super::pair::FailAfterSend;
    use crate::zenoh_link::Error as LinkError;
    type Failing = Link<FailAfterSend>;
    let ab = Rc::new(RefCell::new(Pipe::default()));
    let ba = Rc::new(RefCell::new(Pipe::default()));
    let armed = Rc::new(RefCell::new(false));
    let peer = Params {
        local: 2,
        per_side: 3,
        ..params()
    };
    let mut svc = Failing::with_params(
        FailAfterSend {
            inner: Io {
                rx: ab.clone(),
                tx: ba.clone(),
            },
            armed: armed.clone(),
            sent_since_armed: false,
        },
        Role::Listener,
        &cfg(&[0xb2; 8], &[0xc0; 32], 7000),
        params(),
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
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut over = OverLink::<12>::new();
    declare_all(
        &mut core, &mut over, &mut svc, &mut far, hs, hf, &mut now, false,
    );
    far.declare_subscriber(hf, 1, KEYS[0])
        .expect("the peer declares");
    for _ in 0..3 {
        step(&mut svc, &mut far, hs, hf, &mut now);
    }
    while svc.next_notice().is_some() {}
    *armed.borrow_mut() = true;
    assert!(
        over.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            100,
            1100,
            &payload(100)
        )
        .expect("granted")
        .is_none()
    );
    now += 1;
    assert_eq!(
        svc.pump(hs, now).expect_err("the stream resets"),
        LinkError::Link(crate::zenoh_link::LinkError::Reset)
    );
    assert!(svc.is_closed());
    // A second pump on the closed link flushes the CLOSE it queued, and the slot reads empty.
    now += 1;
    svc.pump(hs, now)
        .expect("a pump on a closed link only flushes");
    assert!(svc.is_closed() && !svc.send_pending());
    assert_eq!(
        over.settle(&mut core, &svc),
        Err(ServiceError::Link(LinkError::SendAfterClose))
    );
    assert_eq!(
        (core.held(0).unwrap(), core.held(1).unwrap()),
        ((0, 0), (0, 0))
    );
    // The waiting publication is gone, so the core is idle and the next call finds the closed link.
    assert_eq!(over.settle(&mut core, &svc), Ok(Settled::Idle));
}

#[test]
fn a_core_at_the_largest_fixed_profile_still_fits_its_declared_budget() {
    // `limits::LARGE` carries 1024-byte payloads. The link-facing state lives outside the core, so the
    // core's fixed size is the same with or without a link and stays within the contract's bound.
    use crate::zenoh_service::Core;
    let large = Core::<3, 4, 1024, 5>::sizes();
    let small = Core::<3, 4, 12, 5>::sizes();
    assert_eq!(large.fixed, small.fixed);
    assert!(large.fixed <= slime_proto::zenoh_service_grants::SERVICE_FIXED_BYTES as usize);
    // `Core::new` evaluates the budget at compile time, so this fails to compile if the core grows. The
    // refusal for the session's payload bound comes after that evaluation, at run time.
    let built = Core::<3, 4, 1024, 5>::new(&rows(), GID, &params());
    assert_eq!(built.err(), Some(ServiceError::InvalidGrants));
    let _over = OverLink::<1024>::new();
}

#[test]
fn a_second_over_link_finds_the_core_busy_while_a_frame_is_in_flight() {
    let (mut svc, mut far, hs, hf) = pair();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut one = OverLink::<12>::new();
    let mut two = OverLink::<12>::new();
    let mut now = 20u64;
    declare_all(
        &mut core, &mut one, &mut svc, &mut far, hs, hf, &mut now, false,
    );
    peer_declares(&mut svc, &mut far, hs, hf, &mut now);
    assert!(
        one.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            100,
            1100,
            &payload(100)
        )
        .expect("granted")
        .is_none()
    );
    // While the frame waits, and after the stream took it and before `one` settled, `two` finds the core busy.
    let before = (core.held(0).unwrap(), core.held(1).unwrap());
    assert_eq!(
        two.publish(
            &mut core,
            &mut svc,
            hs,
            1,
            KEYS[2],
            101,
            1101,
            &payload(101)
        )
        .expect_err("in flight"),
        ServiceError::Busy
    );
    step(&mut svc, &mut far, hs, hf, &mut now);
    assert!(!svc.send_pending());
    assert_eq!(
        two.publish(
            &mut core,
            &mut svc,
            hs,
            1,
            KEYS[2],
            101,
            1101,
            &payload(101)
        )
        .expect_err("in flight"),
        ServiceError::Busy
    );
    assert_eq!(
        two.declare_next(&mut core, &mut svc, hs)
            .expect_err("in flight"),
        ServiceError::Busy
    );
    assert_eq!(before, (core.held(0).unwrap(), core.held(1).unwrap()));
    // Once `one` settled, `two` may queue.
    assert!(matches!(
        one.settle(&mut core, &svc),
        Ok(Settled::Published { .. })
    ));
    assert!(
        two.publish(
            &mut core,
            &mut svc,
            hs,
            1,
            KEYS[2],
            101,
            1101,
            &payload(101)
        )
        .expect("granted")
        .is_none()
    );
}

#[test]
fn polling_for_the_next_declaration_at_the_last_subscription_id_returns_false() {
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    core.pretend_declared(u16::MAX, u16::MAX);
    let (mut svc, _far, hs, _hf) = pair();
    let mut over = OverLink::<12>::new();
    assert_eq!(over.declare_next(&mut core, &mut svc, hs), Ok(false));
    assert!(!svc.send_pending());
}

#[test]
fn a_second_over_link_finds_the_core_busy_while_a_declaration_is_in_flight() {
    let (mut svc, mut far, hs, hf) = pair();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut one = OverLink::<12>::new();
    let mut two = OverLink::<12>::new();
    let mut now = 20u64;
    assert!(one.declare_next(&mut core, &mut svc, hs).expect("queues"));
    // After the stream took the frame and before `one` settled, `two` finds the core busy.
    step(&mut svc, &mut far, hs, hf, &mut now);
    assert!(!svc.send_pending());
    assert_eq!(
        two.declare_next(&mut core, &mut svc, hs)
            .expect_err("in flight"),
        ServiceError::Busy
    );
    assert_eq!(core.declared(), 0);
    assert_eq!(one.settle(&mut core, &svc), Ok(Settled::Declared { id: 1 }));
    assert!(
        two.declare_next(&mut core, &mut svc, hs)
            .expect("the next one")
    );
}

#[test]
fn a_closed_link_frees_the_core_for_another_over_link() {
    use super::Link;
    use super::pair::FailAfterSend;
    use crate::zenoh_link::Error as LinkError;
    type Failing = Link<FailAfterSend>;
    let ab = Rc::new(RefCell::new(Pipe::default()));
    let ba = Rc::new(RefCell::new(Pipe::default()));
    let armed = Rc::new(RefCell::new(false));
    let peer = Params {
        local: 2,
        per_side: 3,
        ..params()
    };
    let mut svc = Failing::with_params(
        FailAfterSend {
            inner: Io {
                rx: ab.clone(),
                tx: ba.clone(),
            },
            armed: armed.clone(),
            sent_since_armed: false,
        },
        Role::Listener,
        &cfg(&[0xb2; 8], &[0xc0; 32], 7000),
        params(),
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
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut one = OverLink::<12>::new();
    let mut two = OverLink::<12>::new();
    declare_all(
        &mut core, &mut one, &mut svc, &mut far, hs, hf, &mut now, false,
    );
    far.declare_subscriber(hf, 1, KEYS[0])
        .expect("the peer declares");
    for _ in 0..3 {
        step(&mut svc, &mut far, hs, hf, &mut now);
    }
    while svc.next_notice().is_some() {}
    *armed.borrow_mut() = true;
    assert!(
        one.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            100,
            1100,
            &payload(100)
        )
        .expect("granted")
        .is_none()
    );
    now += 1;
    assert_eq!(
        svc.pump(hs, now).expect_err("the stream resets"),
        LinkError::Link(crate::zenoh_link::LinkError::Reset)
    );
    now += 1;
    svc.pump(hs, now)
        .expect("a pump on a closed link only flushes");
    // The link closed with a frame in flight. `settle` drops it and frees the core.
    assert_eq!(
        one.settle(&mut core, &svc),
        Err(ServiceError::Link(LinkError::SendAfterClose))
    );
    // Another `OverLink` over the same core is not told the core is busy. It finds the closed link.
    assert_eq!(
        two.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            101,
            1101,
            &payload(101)
        )
        .expect_err("closed"),
        ServiceError::Link(LinkError::SendAfterClose)
    );
    // That refusal queued nothing. A second call is refused as closed again and not as busy.
    assert!(two.is_idle());
    assert_eq!(
        two.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            102,
            1102,
            &payload(102)
        )
        .expect_err("closed"),
        ServiceError::Link(LinkError::SendAfterClose)
    );
}

#[test]
fn a_refused_publication_leaves_nothing_queued() {
    let (mut svc, mut far, hs, hf) = pair();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let mut over = OverLink::<12>::new();
    let mut now = 20u64;
    declare_all(
        &mut core, &mut over, &mut svc, &mut far, hs, hf, &mut now, false,
    );
    peer_declares(&mut svc, &mut far, hs, hf, &mut now);
    let before = (
        core.held(0).unwrap(),
        core.held(1).unwrap(),
        core.held(2).unwrap(),
    );
    let bytes = [0u8; 13];
    // (holder, key, payload length, why it is refused)
    let refusals = [
        (3usize, KEYS[0], 3usize, ServiceError::UnknownHolder),
        (1, KEYS[0], 3, ServiceError::NotGranted),
        (2, KEYS[2], 3, ServiceError::NotGranted),
        (0, KEYS[0], 13, ServiceError::TooLarge),
    ];
    for (holder, key, len, why) in refusals {
        for _ in 0..2 {
            // A second call is refused for the same reason. A refusal that left the core or the OverLink
            // waiting would turn it into `Busy`.
            assert_eq!(
                over.publish(&mut core, &mut svc, hs, holder, key, 7, 1007, &bytes[..len])
                    .expect_err("refused"),
                why
            );
            assert!(over.is_idle());
            assert!(!svc.send_pending());
        }
    }
    assert_eq!(
        before,
        (
            core.held(0).unwrap(),
            core.held(1).unwrap(),
            core.held(2).unwrap()
        )
    );
    // A valid publication is queued and then settles, which it could not if a refusal had left the core in flight.
    assert!(
        over.publish(
            &mut core,
            &mut svc,
            hs,
            0,
            KEYS[0],
            100,
            1100,
            &payload(100)
        )
        .expect("granted")
        .is_none()
    );
    step(&mut svc, &mut far, hs, hf, &mut now);
    assert_eq!(
        over.settle(&mut core, &svc),
        Ok(Settled::Published { reached: 2 })
    );
}
