//! What the core does that the exam's pinned transcript does not show. Each test fails against the
//! mistake it names, so the claim in the core's documentation is checked and not only written.

use super::exam::{Exam, GID, KEYS, open, params, rows};
use super::{Core, Row, ServiceError};
use crate::zenoh_profile0::session::{Config, Params, Role, SessionError, SessionOf};
use crate::zenoh_profile0::{MAX_BATCH_BYTES, MAX_KEY_BYTES};

#[test]
fn a_publication_on_a_session_that_is_not_open_is_not_open_and_not_unsubscribed() {
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    let config = Config {
        zid: &[0xb2; 8],
        lease_ms: 10_000,
        initial_sn: 7000,
        cookie: &[0xc0; 32],
    };
    let mut idle =
        SessionOf::<1, 5, MAX_KEY_BYTES, 4>::with_params(Role::Listener, &config, params())
            .expect("idle");
    let mut out = [0u8; MAX_BATCH_BYTES];
    let refused = core.publish(&mut idle, 0, KEYS[0], 1, 1, &[1, 2, 3], &mut out);
    assert_eq!(
        refused.err(),
        Some(ServiceError::Session(SessionError::NotOpen))
    );
    assert_eq!(
        core.held(0).unwrap(),
        (0, 0),
        "a refused publication reaches no ring"
    );
    assert_eq!(core.held(1).unwrap(), (0, 0));
}

#[test]
fn a_session_declared_with_other_parameters_is_refused_before_anything_changes() {
    let (mut service, _peer) = open();
    let mut core = Exam::new(&rows(), GID, &params()).expect("the honest grants");
    // The same table size, so the type is the same, with a different count on each side.
    let config = Config {
        zid: &[0xb2; 8],
        lease_ms: 10_000,
        initial_sn: 7000,
        cookie: &[0xc0; 32],
    };
    let other = Params {
        local: 4,
        per_side: 1,
        ..params()
    };
    let mut different =
        SessionOf::<1, 5, MAX_KEY_BYTES, 4>::with_params(Role::Listener, &config, other)
            .expect("different");
    let mut out = [0u8; MAX_BATCH_BYTES];
    assert_eq!(
        core.declare_next(&mut different, &mut out).err(),
        Some(ServiceError::WrongSession)
    );
    assert_eq!(
        core.publish(&mut different, 0, KEYS[0], 1, 1, &[1, 2, 3], &mut out)
            .err(),
        Some(ServiceError::WrongSession)
    );
    assert_eq!(
        core.on_batch(&mut different, &[0x01], 1, &mut out).err(),
        Some(ServiceError::WrongSession)
    );
    assert_eq!(core.declared(), 0, "a refused declaration declares nothing");
    // The session the core was declared with still works.
    assert!(
        core.declare_next(&mut service, &mut out)
            .expect("declares")
            .is_some()
    );
}

#[test]
fn a_row_with_an_empty_key_or_a_key_over_the_session_bound_is_invalid() {
    let honest = rows();
    let mut empty = honest;
    empty[0] = Row {
        key: "",
        ..honest[0]
    };
    assert_eq!(
        Exam::new(&empty, GID, &params()).err(),
        Some(ServiceError::InvalidGrants)
    );
    let long = "k".repeat(MAX_KEY_BYTES + 1);
    let mut over = honest;
    over[0] = Row {
        key: &long,
        ..honest[0]
    };
    assert_eq!(
        Exam::new(&over, GID, &params()).err(),
        Some(ServiceError::InvalidGrants)
    );
    let exact = "k".repeat(MAX_KEY_BYTES);
    let mut at_bound = honest;
    at_bound[0] = Row {
        key: &exact,
        ..honest[0]
    };
    assert!(
        Exam::new(&at_bound, GID, &params()).is_ok(),
        "a key of exactly the bound is valid"
    );
}

#[test]
fn a_session_that_reassembles_fragments_is_refused() {
    let heap = Params {
        fragments: true,
        ..params()
    };
    assert_eq!(
        Exam::new(&rows(), GID, &heap).err(),
        Some(ServiceError::InvalidGrants)
    );
    let _ = Core::<3, 4, 12, 5>::sizes();
}
