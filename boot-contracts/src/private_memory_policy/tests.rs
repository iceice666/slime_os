extern crate std;
use super::*;
use ledger::{Class, Incarnation, Ledger, Placement, Plan, Range, Resources};
use std::vec::Vec;

fn bytes(
    mut entitlements: Vec<Entitlement>,
    mut subjects: Vec<Subject>,
    reserve: Resources,
) -> Vec<u8> {
    entitlements.sort_by_key(|entry| entry.identity);
    subjects.sort_by_key(|entry| entry.identity);
    let header = Header {
        magic: MAGIC,
        format_version: FORMAT_VERSION,
        header_size: HEADER_BYTES as u32,
        required_flags: 0,
        entitlement_count: entitlements.len() as u32,
        subject_count: subjects.len() as u32,
        total_len: (HEADER_BYTES
            + entitlements.len() * ENTITLEMENT_BYTES
            + subjects.len() * SUBJECT_BYTES) as u32,
        reserved: 0,
        reserve_bytes: reserve.bytes,
        reserve_slots: reserve.slots,
        reserve_descriptors: reserve.descriptors,
        reserve_extents: reserve.extents,
        reserve_tables: reserve.tables,
    };
    let mut output = header.encode().to_vec();
    for entry in entitlements {
        output.extend(entry.encode());
    }
    for entry in subjects {
        output.extend(entry.encode());
    }
    output
}
fn entitlement(name: &str, guarantee: u64) -> Entitlement {
    Entitlement {
        identity: entitlement_identity(name),
        subtree_root: [0; 32],
        guarantee_pages: guarantee,
        maximum_pages: 0,
        maximum_mode: POOL,
        reserved: 0,
    }
}
fn subject(name: &str, group: &str) -> Subject {
    Subject {
        identity: subject_identity(name),
        entitlement: entitlement_identity(group),
        maximum_pages: 0,
        maximum_mode: POOL,
        reserved: 0,
    }
}
fn resources(pages: u64) -> Resources {
    Resources {
        bytes: pages * PAGE_BYTES,
        slots: pages,
        descriptors: pages,
        extents: pages,
        tables: pages,
    }
}
/// `pages` of guaranteed payload with no leaf tables, the shape a witness
/// may redeem page for page.
fn payload(pages: u64) -> Resources {
    Resources {
        tables: 0,
        ..resources(pages)
    }
}
fn plan(pages: u64) -> Plan {
    let placements: Vec<_> = (0..pages)
        .map(|page| Placement {
            start: page * PAGE_BYTES,
            size_bits: 12,
        })
        .collect();
    Plan::validate(
        &[Range {
            start: 0,
            bytes: pages * PAGE_BYTES,
            class: Class::OrdinaryTail,
        }],
        &placements,
        resources(pages),
    )
    .unwrap()
}
fn instances(policy: Policy<'_>) -> Vec<Instance> {
    (0..policy.subject_count())
        .map(|index| Instance {
            identity: policy.subject(index).unwrap().identity,
            owner: None,
        })
        .collect()
}

fn run_contention(policy: Policy<'_>) -> (Resources, Resources, Incarnation) {
    let guarantees: Vec<_> = (0..policy.entitlement_count())
        .map(|index| resources(policy.entitlement(index).unwrap().guarantee_pages))
        .collect();
    let mut ledger = Ledger::admit(policy, &instances(policy), resources(8), &guarantees).unwrap();
    let initial = ledger.available();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    let guaranteed = ledger.bind(&subject_identity("guaranteed")).unwrap();
    assert_eq!(ledger.available(), initial);
    assert_eq!(
        ledger.bind(&subject_identity("a")),
        Err(ledger::Error::Incarnation)
    );
    assert_eq!(
        ledger.bind(&subject_identity("denied")),
        Err(ledger::Error::Denied)
    );
    ledger.begin(b, 5, plan(5)).unwrap();
    ledger.commit(b).unwrap();
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Unavailable));
    ledger.begin(guaranteed, 2, plan(2)).unwrap();
    ledger.commit(guaranteed).unwrap();
    assert_eq!(ledger.pages(b), Ok(5));
    assert_eq!(ledger.retire(b, false), Err(ledger::Error::Cleanup));
    assert_eq!(
        ledger.bind(&subject_identity("b")),
        Err(ledger::Error::Incarnation)
    );
    assert_eq!(ledger.begin(b, 0, plan(0)), Err(ledger::Error::Cleanup));
    ledger.retire(b, true).unwrap();
    ledger.begin(a, 5, plan(5)).unwrap();
    ledger.commit(a).unwrap();
    assert_eq!(ledger.retire(b, true), Err(ledger::Error::Incarnation));
    let restarted = ledger.bind(&subject_identity("b")).unwrap();
    ledger.retire(a, true).unwrap();
    ledger.retire(guaranteed, true).unwrap();
    ledger.retire(restarted, true).unwrap();
    (initial, ledger.available(), restarted)
}

#[test]
fn unbegun_cleanup_charges_once_without_payload_and_refunds_only_after_revoke() {
    let data = bytes(
        std::vec![entitlement("shared", 1)],
        std::vec![subject("a", "shared")],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[resources(1)]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 1, plan(1)).unwrap();
    ledger.commit(a).unwrap();
    let demand = resources(2);
    let before = ledger.available();
    ledger.can_fund_common(demand).unwrap();
    let root = Resources {
        bytes: before.bytes - demand.bytes,
        ..Resources::ZERO
    };
    ledger.reserve_root(root).unwrap();
    ledger.can_fund_common(demand).unwrap();
    let retained = Resources {
        bytes: PAGE_BYTES,
        slots: 1,
        extents: 1,
        ..Resources::ZERO
    };
    assert_eq!(ledger.ensure_growable(a), Ok(()));
    ledger.retain_unbegun(a, retained).unwrap();
    assert_eq!(ledger.ensure_growable(a), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.ensure_growable(a), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.pages(a), Ok(1));
    assert_eq!(
        ledger.entitlement_pages(ledger.entitlement_of(a).unwrap()),
        1
    );
    assert_eq!(ledger.common_held(), retained);
    assert_eq!(ledger.guaranteed_held(a), Ok(resources(1)));
    let charged = ledger.available();
    assert_eq!(
        ledger.retain_unbegun(a, retained),
        Err(ledger::Error::Cleanup)
    );
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.retire(a, false), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.available(), charged);
    ledger.settle_root(root).unwrap();
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), before.subtract(root).unwrap());
    assert_eq!(ledger.root_owned(), root);
    assert_eq!(ledger.common_held(), Resources::ZERO);
    assert_eq!(ledger.retire(a, true), Err(ledger::Error::Incarnation));
    assert_eq!(ledger.ensure_growable(a), Err(ledger::Error::Incarnation));
    let replacement = ledger.bind(&subject_identity("a")).unwrap();
    assert_eq!(ledger.ensure_growable(replacement), Ok(()));
    assert_eq!(ledger.ensure_growable(a), Err(ledger::Error::Incarnation));
    assert_eq!(
        ledger.retain_unbegun(a, retained),
        Err(ledger::Error::Incarnation)
    );
}

#[test]
fn common_preflight_and_unbegun_retention_check_every_component_atomically() {
    let data = bytes(
        std::vec![entitlement("shared", 1)],
        std::vec![subject("a", "shared")],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(3), &[resources(1)]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    for excess in [
        Resources {
            bytes: 1,
            ..Resources::ZERO
        },
        Resources {
            slots: 1,
            ..Resources::ZERO
        },
        Resources {
            descriptors: 1,
            ..Resources::ZERO
        },
        Resources {
            extents: 1,
            ..Resources::ZERO
        },
        Resources {
            tables: 1,
            ..Resources::ZERO
        },
    ] {
        let too_much = resources(1).checked_add(excess).unwrap();
        assert_eq!(
            ledger.can_fund_common(too_much),
            Err(ledger::Error::Unavailable)
        );
        assert_eq!(
            ledger.retain_unbegun(a, too_much),
            Err(ledger::Error::Unavailable)
        );
        assert_eq!(ledger.available(), resources(1));
        assert_eq!(ledger.common_held(), Resources::ZERO);
        assert_eq!(
            ledger.guaranteed_available(&entitlement_identity("shared")),
            Ok(resources(1))
        );
    }
    ledger.begin(a, 1, plan(1)).unwrap();
    assert_eq!(
        ledger.retain_unbegun(a, Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    ledger.abort(a, Resources::ZERO, true).unwrap();
    ledger.retain_unbegun(a, Resources::ZERO).unwrap();
    assert_eq!(ledger.pages(a), Ok(0));
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Cleanup));
}

#[test]
fn unbegun_retention_after_begin_refusal_does_not_grant_payload() {
    let mut member = subject("a", "shared");
    member.maximum_mode = FIXED;
    member.maximum_pages = 1;
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![member],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(4), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.can_fund_common(resources(2)).unwrap();
    assert_eq!(ledger.begin(a, 2, plan(2)), Err(ledger::Error::Maximum));
    ledger.retain_unbegun(a, resources(1)).unwrap();
    assert_eq!(ledger.pages(a), Ok(0));
    assert_eq!(ledger.entitlement_pages(0), 0);
    assert_eq!(ledger.available(), resources(3));
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), resources(4));
}

#[test]
fn system_reconciliation_replaces_exact_census_and_preserves_other_owners() {
    let data = bytes(
        std::vec![entitlement("shared", 2)],
        std::vec![subject("a", "shared")],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(12), &[resources(2)]).unwrap();
    ledger.reserve_root(resources(1)).unwrap();
    ledger.settle_root(resources(1)).unwrap();
    let initial = ledger.available();
    assert_eq!(ledger.system_owned(), Resources::ZERO);
    for census in [resources(2), resources(4), resources(1)] {
        ledger.reconcile_system(census).unwrap();
        assert_eq!(ledger.system_owned(), census);
        assert_eq!(ledger.available(), initial.subtract(census).unwrap());
        assert_eq!(ledger.root_owned(), resources(1));
        assert_eq!(ledger.operational_reserve(), resources(1));
        assert_eq!(
            ledger.guaranteed_available(&entitlement_identity("shared")),
            Ok(resources(2))
        );
    }
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 3, plan(3)).unwrap();
    ledger.commit(a).unwrap();
    assert_eq!(ledger.common_held(), resources(1));
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), initial.subtract(resources(1)).unwrap());
    assert_eq!(ledger.system_owned(), resources(1));
    assert_eq!(ledger.root_owned(), resources(1));
    ledger.reconcile_system(Resources::ZERO).unwrap();
    assert_eq!(ledger.available(), initial);
}

#[test]
fn operational_censuses_spend_componentwise_and_cleanup_restores_reserve() {
    let reserve = resources(3);
    let data = bytes(std::vec![], std::vec![], reserve);
    let policy = Policy::decode(&data).unwrap();
    let mut ledger = Ledger::admit(policy, &[], resources(10), &[]).unwrap();
    assert_eq!(ledger.operational_spent(), Resources::ZERO);
    assert_eq!(ledger.remaining_operational_reserve(), reserve);
    let root = resources(2);
    let system = Resources {
        bytes: 7 * PAGE_BYTES,
        slots: 6,
        descriptors: 5,
        extents: 4,
        tables: 8,
    };
    ledger.reconcile_ownership(root, system).unwrap();
    let remaining = Resources {
        bytes: PAGE_BYTES,
        slots: 2,
        descriptors: 3,
        extents: 3,
        tables: 0,
    };
    assert_eq!(ledger.root_owned(), root);
    assert_eq!(ledger.system_owned(), system);
    assert_eq!(ledger.remaining_operational_reserve(), remaining);
    assert_eq!(
        ledger.operational_spent(),
        reserve.subtract(remaining).unwrap()
    );
    assert_eq!(
        ledger.available(),
        Resources {
            extents: 1,
            ..Resources::ZERO
        }
    );
    let debt = ledger.operational_spent();
    assert_eq!(
        ledger.reserve_root(resources(1)),
        Err(ledger::Error::Unavailable)
    );
    assert_eq!(
        ledger.can_fund_common(resources(1)),
        Err(ledger::Error::Unavailable)
    );
    ledger.reconcile_ownership(root, system).unwrap();
    assert_eq!(ledger.operational_spent(), debt);
    ledger.reconcile_system(Resources::ZERO).unwrap();
    assert_eq!(ledger.operational_spent(), Resources::ZERO);
    assert_eq!(ledger.remaining_operational_reserve(), reserve);
    assert_eq!(ledger.available(), resources(5));
    assert_eq!(ledger.root_owned(), root);
}

#[test]
fn guarantees_redeem_and_refund_separately_while_common_retirement_repays_debt() {
    let data = bytes(
        std::vec![entitlement("common", 0), entitlement("protected", 2)],
        std::vec![subject("a", "common"), subject("b", "protected")],
        resources(3),
    );
    let policy = Policy::decode(&data).unwrap();
    let guarantees: Vec<_> = (0..policy.entitlement_count())
        .map(|index| resources(policy.entitlement(index).unwrap().guarantee_pages))
        .collect();
    let mut ledger = Ledger::admit(policy, &instances(policy), resources(12), &guarantees).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    ledger.begin(a, 4, plan(4)).unwrap();
    ledger.commit(a).unwrap();
    ledger
        .reconcile_ownership(resources(1), resources(4))
        .unwrap();
    assert_eq!(ledger.available(), Resources::ZERO);
    assert_eq!(ledger.operational_spent(), resources(2));
    assert_eq!(ledger.common_held(), resources(4));
    ledger.begin(b, 2, plan(2)).unwrap();
    ledger.abort(b, Resources::ZERO, true).unwrap();
    assert_eq!(ledger.operational_spent(), resources(2));
    ledger.begin(b, 2, plan(2)).unwrap();
    ledger.commit(b).unwrap();
    assert_eq!(ledger.guaranteed_held(b), Ok(resources(2)));
    ledger.retire(b, true).unwrap();
    assert_eq!(
        ledger.guaranteed_available(&entitlement_identity("protected")),
        Ok(resources(2))
    );
    assert_eq!(ledger.available(), Resources::ZERO);
    assert_eq!(ledger.operational_spent(), resources(2));
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), resources(2));
    assert_eq!(ledger.operational_spent(), Resources::ZERO);
    assert_eq!(ledger.remaining_operational_reserve(), resources(3));
    assert_eq!(ledger.root_owned(), resources(1));
    assert_eq!(ledger.system_owned(), resources(4));
}

#[test]
fn ownership_shortage_root_decrease_and_pending_are_atomic_with_debt() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared")],
        resources(3),
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger = Ledger::admit(
        policy,
        &instances(policy),
        resources(10),
        &[Resources::ZERO],
    )
    .unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger
        .reconcile_ownership(resources(2), resources(7))
        .unwrap();
    for component in 0..5 {
        let mut lower = resources(2);
        let field = match component {
            0 => &mut lower.bytes,
            1 => &mut lower.slots,
            2 => &mut lower.descriptors,
            3 => &mut lower.extents,
            _ => &mut lower.tables,
        };
        *field -= 1;
        assert_eq!(
            ledger.reconcile_ownership(lower, Resources::ZERO),
            Err(ledger::Error::Unavailable)
        );
        let mut excess = resources(8);
        let field = match component {
            0 => &mut excess.bytes,
            1 => &mut excess.slots,
            2 => &mut excess.descriptors,
            3 => &mut excess.extents,
            _ => &mut excess.tables,
        };
        *field += 1;
        assert_eq!(
            ledger.reconcile_ownership(resources(2), excess),
            Err(ledger::Error::Unavailable)
        );
        assert_eq!(ledger.root_owned(), resources(2));
        assert_eq!(ledger.system_owned(), resources(7));
        assert_eq!(ledger.operational_spent(), resources(2));
        assert_eq!(ledger.remaining_operational_reserve(), resources(1));
        assert_eq!(ledger.available(), Resources::ZERO);
    }
    ledger.reserve_root(Resources::ZERO).unwrap();
    assert_eq!(
        ledger.reconcile_ownership(resources(2), Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    ledger.settle_root(Resources::ZERO).unwrap();
    ledger.begin(a, 0, plan(0)).unwrap();
    assert_eq!(
        ledger.reconcile_ownership(resources(2), Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    ledger.abort(a, Resources::ZERO, true).unwrap();
    assert_eq!(ledger.root_owned(), resources(2));
    assert_eq!(ledger.system_owned(), resources(7));
    assert_eq!(ledger.operational_spent(), resources(2));
    assert_eq!(ledger.available(), Resources::ZERO);
}

#[test]
fn system_shortage_is_atomic_for_every_resource_component() {
    let data = bytes(std::vec![], std::vec![], Resources::ZERO);
    let policy = Policy::decode(&data).unwrap();
    let mut ledger = Ledger::admit(policy, &[], resources(8), &[]).unwrap();
    ledger.reconcile_system(resources(2)).unwrap();
    for excess in [
        Resources {
            bytes: 1,
            ..Resources::ZERO
        },
        Resources {
            slots: 1,
            ..Resources::ZERO
        },
        Resources {
            descriptors: 1,
            ..Resources::ZERO
        },
        Resources {
            extents: 1,
            ..Resources::ZERO
        },
        Resources {
            tables: 1,
            ..Resources::ZERO
        },
    ] {
        assert_eq!(
            ledger.reconcile_system(resources(8).checked_add(excess).unwrap()),
            Err(ledger::Error::Unavailable)
        );
        assert_eq!(ledger.available(), resources(6));
        assert_eq!(ledger.system_owned(), resources(2));
    }
}

#[test]
fn system_reconciliation_refuses_root_and_holder_pending_funding() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[Resources::ZERO]).unwrap();
    ledger.reconcile_system(resources(1)).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.reserve_root(resources(1)).unwrap();
    assert_eq!(ledger.common_held(), Resources::ZERO);
    assert_eq!(
        ledger.reconcile_system(Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    assert_eq!(ledger.available(), resources(6));
    assert_eq!(ledger.system_owned(), resources(1));
    ledger.begin(a, 2, plan(2)).unwrap();
    assert_eq!(ledger.common_held(), resources(2));
    ledger.settle_root(resources(1)).unwrap();
    assert_eq!(
        ledger.reconcile_system(Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    assert_eq!(ledger.available(), resources(4));
    assert_eq!(ledger.system_owned(), resources(1));
    ledger.commit(a).unwrap();
    assert_eq!(ledger.common_held(), resources(2));
    ledger.reconcile_system(Resources::ZERO).unwrap();
    assert_eq!(ledger.available(), resources(5));
}

#[test]
fn system_reconciliation_at_integer_limits_never_wraps() {
    let data = bytes(std::vec![], std::vec![], Resources::ZERO);
    let policy = Policy::decode(&data).unwrap();
    let maximum = Resources {
        bytes: u64::MAX,
        slots: u64::MAX,
        descriptors: u64::MAX,
        extents: u64::MAX,
        tables: u64::MAX,
    };
    let one = Resources {
        bytes: 1,
        slots: 1,
        descriptors: 1,
        extents: 1,
        tables: 1,
    };
    let mut ledger = Ledger::admit(policy, &[], maximum, &[]).unwrap();
    for census in [maximum, one, maximum, Resources::ZERO] {
        ledger.reconcile_system(census).unwrap();
        assert_eq!(ledger.available(), maximum.subtract(census).unwrap());
        assert_eq!(ledger.system_owned(), census);
    }
    // Public transitions conserve the admitted tuple, so refunding an existing
    // census cannot overflow even at the largest representable inventory.
    assert_eq!(maximum.checked_add(one), Err(ledger::Error::Overflow));
    ledger.reserve_root(one).unwrap();
    ledger.settle_root(one).unwrap();
    assert_eq!(
        ledger.reconcile_system(maximum),
        Err(ledger::Error::Unavailable)
    );
    assert_eq!(ledger.available(), maximum.subtract(one).unwrap());
    assert_eq!(ledger.system_owned(), Resources::ZERO);
}

#[test]
fn common_held_counts_pending_retained_and_quarantined_elastic_without_guarantees() {
    let data = bytes(
        std::vec![entitlement("shared", 1)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(12), &[resources(1)]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    assert_eq!(ledger.common_held(), Resources::ZERO);
    ledger.begin(a, 2, plan(2)).unwrap();
    assert_eq!(ledger.common_held(), resources(1));
    ledger.abort(a, Resources::ZERO, false).unwrap();
    assert_eq!(ledger.common_held(), resources(1));
    assert_eq!(ledger.pages(a), Ok(0));
    let table = Resources {
        extents: 0,
        ..resources(1)
    };
    ledger.begin(b, 2, plan(2)).unwrap();
    assert_eq!(ledger.common_held(), resources(3));
    ledger.abort(b, table, true).unwrap();
    assert_eq!(
        ledger.common_held(),
        resources(1).checked_add(table).unwrap()
    );
    ledger.begin(b, 1, plan(1)).unwrap();
    assert_eq!(
        ledger.common_held(),
        resources(2).checked_add(table).unwrap()
    );
    ledger.commit(b).unwrap();
    assert_eq!(
        ledger.common_held(),
        resources(2).checked_add(table).unwrap()
    );
    assert_eq!(ledger.retire(a, false), Err(ledger::Error::Cleanup));
    assert_eq!(
        ledger.common_held(),
        resources(2).checked_add(table).unwrap()
    );
    ledger.retire(a, true).unwrap();
    ledger.retire(b, true).unwrap();
    assert_eq!(ledger.common_held(), Resources::ZERO);
    assert_eq!(ledger.available(), resources(11));
}

#[test]
fn root_funding_preserves_protected_resources_and_refuses_each_unaffordable_component() {
    let data = bytes(
        std::vec![entitlement("shared", 2)],
        std::vec![subject("a", "shared")],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[resources(2)]).unwrap();
    let initial = resources(5);
    for requested in [
        Resources {
            bytes: initial.bytes + 1,
            ..Resources::ZERO
        },
        Resources {
            slots: initial.slots + 1,
            ..Resources::ZERO
        },
        Resources {
            descriptors: initial.descriptors + 1,
            ..Resources::ZERO
        },
        Resources {
            extents: initial.extents + 1,
            ..Resources::ZERO
        },
        Resources {
            tables: initial.tables + 1,
            ..Resources::ZERO
        },
    ] {
        assert_eq!(
            ledger.reserve_root(requested),
            Err(ledger::Error::Unavailable)
        );
        assert_eq!(ledger.available(), initial);
        assert_eq!(ledger.root_owned(), Resources::ZERO);
    }
    ledger.reserve_root(initial).unwrap();
    assert_eq!(ledger.available(), Resources::ZERO);
    assert_eq!(ledger.root_owned(), Resources::ZERO);
    assert_eq!(
        ledger.reserve_root(Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
    assert_eq!(
        ledger.guaranteed_available(&entitlement_identity("shared")),
        Ok(resources(2))
    );
    ledger.settle_root(Resources::ZERO).unwrap();
    assert_eq!(ledger.available(), initial);
    assert_eq!(
        ledger.settle_root(Resources::ZERO),
        Err(ledger::Error::Transaction)
    );
}

#[test]
fn root_partial_settlement_checks_every_component_and_keeps_invalid_funding() {
    let data = bytes(std::vec![], std::vec![], Resources::ZERO);
    let policy = Policy::decode(&data).unwrap();
    let mut ledger = Ledger::admit(policy, &[], resources(8), &[]).unwrap();
    let reserved = resources(4);
    ledger.reserve_root(reserved).unwrap();
    for used in [
        Resources {
            bytes: reserved.bytes + 1,
            ..Resources::ZERO
        },
        Resources {
            slots: reserved.slots + 1,
            ..Resources::ZERO
        },
        Resources {
            descriptors: reserved.descriptors + 1,
            ..Resources::ZERO
        },
        Resources {
            extents: reserved.extents + 1,
            ..Resources::ZERO
        },
        Resources {
            tables: reserved.tables + 1,
            ..Resources::ZERO
        },
    ] {
        assert_eq!(ledger.settle_root(used), Err(ledger::Error::Unavailable));
        assert_eq!(ledger.available(), resources(4));
        assert_eq!(ledger.root_owned(), Resources::ZERO);
        assert_eq!(
            ledger.reserve_root(Resources::ZERO),
            Err(ledger::Error::Transaction)
        );
    }
    let used = Resources {
        bytes: PAGE_BYTES,
        slots: 2,
        descriptors: 3,
        extents: 4,
        tables: 0,
    };
    ledger.settle_root(used).unwrap();
    assert_eq!(ledger.root_owned(), used);
    assert_eq!(ledger.available(), resources(8).subtract(used).unwrap());
    assert_eq!(ledger.settle_root(used), Err(ledger::Error::Transaction));
}

#[test]
fn root_funding_at_integer_limits_never_wraps_or_partially_charges() {
    let data = bytes(std::vec![], std::vec![], Resources::ZERO);
    let policy = Policy::decode(&data).unwrap();
    let maximum = Resources {
        bytes: u64::MAX,
        slots: u64::MAX,
        descriptors: u64::MAX,
        extents: u64::MAX,
        tables: u64::MAX,
    };
    let one = Resources {
        bytes: 1,
        slots: 1,
        descriptors: 1,
        extents: 1,
        tables: 1,
    };
    for extra in [
        Resources {
            bytes: 1,
            ..Resources::ZERO
        },
        Resources {
            slots: 1,
            ..Resources::ZERO
        },
        Resources {
            descriptors: 1,
            ..Resources::ZERO
        },
        Resources {
            extents: 1,
            ..Resources::ZERO
        },
        Resources {
            tables: 1,
            ..Resources::ZERO
        },
    ] {
        assert_eq!(maximum.checked_add(extra), Err(ledger::Error::Overflow));
    }
    let mut ledger = Ledger::admit(policy, &[], maximum, &[]).unwrap();
    ledger.reserve_root(maximum).unwrap();
    ledger.settle_root(maximum.subtract(one).unwrap()).unwrap();
    assert_eq!(ledger.available(), one);
    assert_eq!(
        ledger.reserve_root(resources(1)),
        Err(ledger::Error::Unavailable)
    );
    assert_eq!(ledger.available(), one);
    assert_eq!(ledger.root_owned(), maximum.subtract(one).unwrap());
    ledger.reserve_root(one).unwrap();
    ledger.settle_root(one).unwrap();
    assert_eq!(ledger.root_owned(), maximum);
    assert_eq!(ledger.reserve_root(one), Err(ledger::Error::Unavailable));
    assert_eq!(ledger.available(), Resources::ZERO);
    assert_eq!(ledger.root_owned(), maximum);
}

#[test]
fn holder_transactions_and_retirement_never_refund_root_and_reuse_is_not_charged() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 2, plan(2)).unwrap();
    ledger.reserve_root(resources(3)).unwrap();
    ledger.abort(a, Resources::ZERO, true).unwrap();
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), resources(5));
    ledger.settle_root(resources(2)).unwrap();
    assert_eq!(ledger.available(), resources(6));
    assert_eq!(ledger.root_owned(), resources(2));
    for _ in 0..20 {
        let a = ledger.bind(&subject_identity("a")).unwrap();
        // Mapping reuses the root backing already owned: no new acquisition.
        ledger.reserve_root(Resources::ZERO).unwrap();
        ledger.begin(a, 2, plan(2)).unwrap();
        ledger.settle_root(Resources::ZERO).unwrap();
        ledger.commit(a).unwrap();
        assert_eq!(ledger.available(), resources(4));
        assert_eq!(ledger.retire(a, false), Err(ledger::Error::Cleanup));
        assert_eq!(ledger.available(), resources(4));
        ledger.retire(a, true).unwrap();
        assert_eq!(ledger.available(), resources(6));
        assert_eq!(ledger.root_owned(), resources(2));
    }
}

#[test]
fn binding_reports_fixed_and_pool_address_maxima_and_quarantines_duplicates() {
    let mut fixed = subject("fixed", "shared");
    fixed.maximum_mode = FIXED;
    fixed.maximum_pages = 3;
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![fixed, subject("pooled", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[Resources::ZERO]).unwrap();

    let fixed = ledger
        .bind_with_maximum(&subject_identity("fixed"))
        .unwrap();
    assert_eq!(fixed.maximum_pages, 3);
    assert_eq!(
        ledger.bind_with_maximum(&subject_identity("fixed")),
        Err(ledger::Error::Incarnation)
    );

    let pooled = ledger
        .bind_with_maximum(&subject_identity("pooled"))
        .unwrap();
    assert_eq!(pooled.maximum_pages, 8);
    ledger.quarantine(pooled.token).unwrap();
    assert_eq!(
        ledger.begin(pooled.token, 0, plan(0)),
        Err(ledger::Error::Cleanup)
    );
    assert_eq!(
        ledger.bind_with_maximum(&subject_identity("pooled")),
        Err(ledger::Error::Incarnation)
    );

    ledger.retire(fixed.token, true).unwrap();
    ledger.retire(pooled.token, true).unwrap();
    assert!(ledger.bind(&subject_identity("pooled")).is_ok());
}

#[test]
fn a_pool_relative_maximum_is_the_admitted_pool_not_the_free_share() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 6, plan(6)).unwrap();
    ledger.commit(a).unwrap();
    assert!(ledger.available().bytes < 8 * PAGE_BYTES);

    // Bound while a peer holds most of the pool: the permission is still the
    // whole admitted pool, so capacity the peer returns stays reachable.
    let b = ledger.bind_with_maximum(&subject_identity("b")).unwrap();
    assert_eq!(b.maximum_pages, 8);
    assert_eq!(ledger.pool_pages(), 8);
    ledger.retire(a, true).unwrap();
    ledger.begin(b.token, 8, plan(8)).unwrap();
    ledger.commit(b.token).unwrap();
}

#[test]
fn shared_guarantee_and_operational_reserve_survive_elastic_contention_and_restart() {
    let data = bytes(
        std::vec![entitlement("shared", 0), entitlement("reserved", 2)],
        std::vec![
            subject("a", "shared"),
            subject("b", "shared"),
            subject("guaranteed", "reserved")
        ],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    let first = run_contention(policy);
    assert_eq!(first.0, resources(5));
    assert_eq!(first.0, first.1);
    assert_eq!(first, run_contention(policy));
}

#[test]
fn failed_growth_retains_only_owned_resources_and_quarantine_is_not_free() {
    let data = bytes(
        std::vec![entitlement("shared", 1)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(6), &[resources(1)]).unwrap();
    let initial = ledger.available();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 2, plan(2)).unwrap();
    let table = Resources {
        bytes: PAGE_BYTES,
        slots: 1,
        descriptors: 1,
        extents: 0,
        tables: 1,
    };
    ledger.abort(a, table, true).unwrap();
    assert_eq!(ledger.pages(a), Ok(0));
    assert_eq!(ledger.available(), initial.subtract(table).unwrap());
    assert_eq!(
        ledger.guaranteed_available(&entitlement_identity("shared")),
        Ok(resources(1))
    );
    ledger.begin(a, 2, plan(2)).unwrap();
    ledger.abort(a, Resources::ZERO, false).unwrap();
    assert_eq!(ledger.available(), resources(4).subtract(table).unwrap());
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Cleanup));
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), initial);
    assert_eq!(
        ledger.guaranteed_available(&entitlement_identity("shared")),
        Ok(resources(1))
    );
}

#[test]
fn shared_members_have_one_guarantee_and_distinct_subject_limits() {
    let mut group = entitlement("shared", 2);
    group.maximum_mode = FIXED;
    group.maximum_pages = 3;
    let mut a = subject("a", "shared");
    a.maximum_mode = FIXED;
    a.maximum_pages = 1;
    let data = bytes(
        std::vec![group],
        std::vec![a, subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[resources(2)]).unwrap();
    assert_eq!(ledger.available(), resources(6));
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    assert_eq!(ledger.begin(a, 2, plan(2)), Err(ledger::Error::Maximum));
    ledger.begin(a, 1, plan(1)).unwrap();
    ledger.commit(a).unwrap();
    ledger.begin(b, 2, plan(2)).unwrap();
    ledger.commit(b).unwrap();
    assert_eq!(ledger.begin(b, 1, plan(1)), Err(ledger::Error::Maximum));
    ledger.retire(a, true).unwrap();
    ledger.retire(b, true).unwrap();
    assert_eq!(ledger.available(), resources(6));
}

#[test]
fn exact_resource_tuple_fit_refuses_each_independent_shortage_atomically() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    for lacking in [
        Resources {
            slots: 0,
            ..resources(1)
        },
        Resources {
            descriptors: 0,
            ..resources(1)
        },
        Resources {
            extents: 0,
            ..resources(1)
        },
        Resources {
            tables: 0,
            ..resources(1)
        },
    ] {
        let mut ledger =
            Ledger::admit(policy, &instances(policy), lacking, &[Resources::ZERO]).unwrap();
        let a = ledger.bind(&subject_identity("a")).unwrap();
        assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Unavailable));
        assert_eq!(ledger.available(), lacking);
        assert_eq!(ledger.pages(a), Ok(0));
    }
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(1), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    ledger.begin(a, 1, plan(1)).unwrap();
    ledger.commit(a).unwrap();
    assert_eq!(ledger.available(), Resources::ZERO);
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Maximum));
    assert_eq!(
        Resources {
            bytes: u64::MAX,
            ..Resources::ZERO
        }
        .checked_add(resources(1)),
        Err(ledger::Error::Overflow)
    );
}

#[test]
fn inventory_partition_and_exact_alignment_do_not_count_parent_bytes_twice() {
    let ordinary = [Range {
        start: 0x200000,
        bytes: 4 * PAGE_BYTES,
        class: Class::OrdinaryTail,
    }];
    let partition = [
        Range {
            start: 0x200000,
            bytes: PAGE_BYTES,
            class: Class::Metadata,
        },
        Range {
            start: 0x201000,
            bytes: PAGE_BYTES,
            class: Class::PreservedLeaf,
        },
        Range {
            start: 0x202000,
            bytes: PAGE_BYTES,
            class: Class::SharedDma,
        },
        Range {
            start: 0x203000,
            bytes: PAGE_BYTES,
            class: Class::Reusable,
        },
    ];
    assert_eq!(
        ledger::inventory_available(&ordinary, &partition),
        Ok(2 * PAGE_BYTES)
    );
    assert_eq!(
        ledger::inventory_available(&ordinary, &partition[..3]),
        Err(ledger::Error::Inventory)
    );
    let mut overlap = partition;
    overlap[1].start = overlap[0].start;
    assert_eq!(
        ledger::inventory_available(&ordinary, &overlap),
        Err(ledger::Error::Inventory)
    );
    assert_eq!(
        Plan::validate(
            &partition,
            &[Placement {
                start: 0x200000,
                size_bits: 21
            }],
            resources(512)
        ),
        Err(ledger::Error::Placement)
    );
    assert!(
        Plan::validate(
            &partition,
            &[
                Placement {
                    start: 0x201000,
                    size_bits: 12
                },
                Placement {
                    start: 0x203000,
                    size_bits: 12
                }
            ],
            resources(2)
        )
        .is_ok()
    );
    assert_eq!(
        Plan::validate(
            &partition,
            &[Placement {
                start: 0x201001,
                size_bits: 12
            }],
            resources(1)
        ),
        Err(ledger::Error::Placement)
    );
}

#[test]
fn explicit_subtree_membership_rejects_wrong_owner_and_cycles() {
    let mut group = entitlement("shared", 0);
    group.subtree_root = subject_identity("owner");
    let data = bytes(
        std::vec![group],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let root = Instance {
        identity: subject_identity("owner"),
        owner: None,
    };
    let member = Instance {
        identity: subject_identity("a"),
        owner: Some(root.identity),
    };
    assert_eq!(policy.validate_instances(&[root, member]), Ok(()));
    assert_eq!(
        policy.validate_instances(&[
            root,
            Instance {
                owner: None,
                ..member
            }
        ]),
        Err(Error::Topology)
    );
    assert_eq!(
        policy.validate_instances(&[
            Instance {
                owner: Some(member.identity),
                ..root
            },
            member
        ]),
        Err(Error::Topology)
    );
    assert_eq!(
        policy.subject_index(&subject_identity("undeclared-child")),
        None
    );
}

#[test]
fn decoder_refuses_malformed_policy_and_v1_reader_rejects_v2() {
    let data = bytes(
        std::vec![entitlement("shared", 1)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    assert!(Policy::decode(&data).is_ok());
    assert!(matches!(
        crate::private_memory_budget::PrivateMemoryBudget::decode(&data),
        Err(crate::private_memory_budget::DecodeError::UnsupportedVersion)
    ));
    for offset in [
        OFF_HEADER_FORMAT_VERSION,
        OFF_HEADER_REQUIRED_FLAGS,
        OFF_HEADER_TOTAL_LEN,
        HEADER_BYTES + OFF_ENTITLEMENT_RESERVED,
        HEADER_BYTES + ENTITLEMENT_BYTES + OFF_SUBJECT_ENTITLEMENT,
    ] {
        let mut invalid = data.clone();
        invalid[offset] ^= 0x40;
        assert!(Policy::decode(&invalid).is_err(), "offset {offset}");
    }
    let mut excessive = entitlement("shared", 2);
    excessive.maximum_mode = FIXED;
    excessive.maximum_pages = 1;
    assert!(
        Policy::decode(&bytes(
            std::vec![excessive],
            std::vec![subject("a", "shared")],
            Resources::ZERO
        ))
        .is_err()
    );
    assert!(
        Policy::decode(&bytes(
            std::vec![entitlement("shared", 1)],
            std::vec![subject("a", "shared"), subject("a", "shared")],
            Resources::ZERO
        ))
        .is_err()
    );
}

#[test]
fn guarantees_and_reserves_are_simultaneously_affordable_or_admission_fails() {
    let data = bytes(
        std::vec![entitlement("shared", 2)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        resources(1),
    );
    let policy = Policy::decode(&data).unwrap();
    assert!(matches!(
        Ledger::admit(policy, &instances(policy), resources(2), &[resources(2)]),
        Err(ledger::Error::Unavailable)
    ));
    assert!(matches!(
        Ledger::admit(policy, &instances(policy), resources(3), &[resources(1)]),
        Err(ledger::Error::Guarantee)
    ));
    let admitted =
        Ledger::admit(policy, &instances(policy), resources(3), &[resources(2)]).unwrap();
    assert_eq!(admitted.available(), Resources::ZERO);
    assert_eq!(
        admitted.guaranteed_available(&entitlement_identity("shared")),
        Ok(resources(2))
    );
}

#[test]
fn zero_payload_and_partial_redemption_cannot_drain_peer_guarantees() {
    let data = bytes(
        std::vec![entitlement("shared", 2)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(2), &[resources(2)]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    let slots = Plan::validate(
        &[],
        &[],
        Resources {
            slots: 2,
            ..Resources::ZERO
        },
    )
    .unwrap();
    assert_eq!(ledger.begin(a, 0, slots), Err(ledger::Error::Placement));
    assert_eq!(ledger.begin(a, 1, plan(2)), Err(ledger::Error::Unavailable));
    ledger.begin(a, 1, plan(1)).unwrap();
    let retained = Resources {
        tables: 1,
        ..Resources::ZERO
    };
    assert_eq!(ledger.abort(a, retained, true), Err(ledger::Error::Cleanup));
    ledger.abort(a, Resources::ZERO, true).unwrap();
    ledger.begin(b, 2, plan(2)).unwrap();
    ledger.commit(b).unwrap();
    assert_eq!(ledger.pages(b), Ok(2));
}

#[test]
fn failed_payload_cannot_evade_shared_maximum_through_abort() {
    let mut group = entitlement("shared", 0);
    group.maximum_mode = FIXED;
    group.maximum_pages = 1;
    let data = bytes(
        std::vec![group],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(4), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    ledger.begin(a, 1, plan(1)).unwrap();
    assert_eq!(
        ledger.abort(a, resources(1), true),
        Err(ledger::Error::Cleanup)
    );
    ledger.abort(a, Resources::ZERO, false).unwrap();
    assert_eq!(ledger.begin(b, 1, plan(1)), Err(ledger::Error::Maximum));
    ledger.retire(a, true).unwrap();
    ledger.begin(b, 1, plan(1)).unwrap();
    ledger.commit(b).unwrap();
    assert_eq!(ledger.pages(b), Ok(1));
}

#[test]
fn repeated_rollback_cannot_accumulate_tables_beyond_the_attempted_window() {
    let mut member = subject("a", "shared");
    member.maximum_mode = FIXED;
    member.maximum_pages = 1;
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![member],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    assert!(matches!(
        Ledger::admit(policy, &instances(policy), resources(4), &[resources(1)]),
        Err(ledger::Error::Guarantee)
    ));
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(4), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let table = Resources {
        bytes: PAGE_BYTES,
        slots: 1,
        descriptors: 1,
        extents: 0,
        tables: 1,
    };
    ledger.begin(a, 1, plan(1)).unwrap();
    ledger.abort(a, table, true).unwrap();
    ledger.begin(a, 1, plan(1)).unwrap();
    // A one-page window has one span, so a second retained table is refused,
    // and the refusal leaves the transaction open for a settling abort.
    assert_eq!(ledger.abort(a, table, true), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Transaction));
    ledger.abort(a, Resources::ZERO, true).unwrap();
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), resources(4));
}

/// One protected range and one ordinary range, and the same placements
/// certified three ways: refused outright without a witness, accepted with a
/// witness that matches what physically landed in the protected range, and
/// refused again when the witness overstates either its bytes or the payload
/// pages it claims to redeem.
#[test]
fn a_protected_placement_is_certified_only_by_a_matching_witness() {
    const PROTECTED: u64 = 1 << 20;
    let sources = std::vec![
        Range {
            start: 0,
            bytes: 4 * PAGE_BYTES,
            class: Class::OrdinaryTail,
        },
        Range {
            start: PROTECTED,
            bytes: 4 * PAGE_BYTES,
            class: Class::Guaranteed,
        },
    ];
    let protected: Vec<_> = (0..2)
        .map(|page| Placement {
            start: PROTECTED + page * PAGE_BYTES,
            size_bits: 12,
        })
        .collect();
    // Payload only: a guaranteed table would be protected bytes that no
    // redeemed page may claim.
    let charged = payload(2);

    // Reservation-owned backing is not free memory, and pointing a placement
    // at it does not make it so.
    assert_eq!(
        Plan::validate(&sources, &protected, charged),
        Err(ledger::Error::Placement)
    );

    let witness = ledger::Witness {
        guaranteed: charged,
        guarantee_pages: 2,
    };
    let plan = Plan::validate_reserved(&sources, &protected, charged, witness).unwrap();
    assert_eq!(plan.resources(), charged);
    assert_eq!(plan.guaranteed(), charged);
    assert_eq!(plan.guarantee_pages(), 2);

    // A witness claiming more protected bytes than landed, and one claiming
    // more redeemed pages than its own bytes hold, are both refused.
    for overstated in [
        ledger::Witness {
            guaranteed: payload(3),
            guarantee_pages: 2,
        },
        ledger::Witness {
            guaranteed: charged,
            guarantee_pages: 3,
        },
    ] {
        assert_eq!(
            Plan::validate_reserved(&sources, &protected, charged, overstated),
            Err(ledger::Error::Guarantee)
        );
    }

    // An ordinary transaction is unchanged, with or without the new path: it
    // draws from free ranges and redeems no guarantee.
    let elastic: Vec<_> = (0..2)
        .map(|page| Placement {
            start: page * PAGE_BYTES,
            size_bits: 12,
        })
        .collect();
    let plan = Plan::validate(&sources, &elastic, charged).unwrap();
    assert_eq!(plan.guaranteed(), Resources::ZERO);
    assert_eq!(plan.guarantee_pages(), 0);
    assert_eq!(
        Plan::validate_reserved(
            &sources,
            &elastic,
            charged,
            ledger::Witness {
                guaranteed: Resources::ZERO,
                guarantee_pages: 0,
            }
        )
        .map(Plan::resources),
        Ok(charged)
    );
    // A mixed transaction certifies each half against its own source.
    let mixed = std::vec![protected[0], elastic[0]];
    let plan = Plan::validate_reserved(
        &sources,
        &mixed,
        charged,
        ledger::Witness {
            guaranteed: payload(1),
            guarantee_pages: 1,
        },
    )
    .unwrap();
    assert_eq!(plan.guaranteed(), payload(1));
    assert_eq!(plan.resources().subtract(plan.guaranteed()), Ok(payload(1)));
}

/// A certified protected transaction is charged to its entitlement rather
/// than to the common pool, and it cannot redeem more than the entitlement
/// promises.
#[test]
fn a_certified_guarantee_is_charged_to_its_entitlement_and_not_to_the_pool() {
    const PROTECTED: u64 = 1 << 20;
    let data = bytes(
        std::vec![entitlement("shared", 2)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[resources(2)]).unwrap();
    let pool = ledger.available();
    let a = ledger.bind(&subject_identity("a")).unwrap();

    let sources = std::vec![Range {
        start: PROTECTED,
        bytes: 2 * PAGE_BYTES,
        class: Class::Guaranteed,
    }];
    let placements: Vec<_> = (0..2)
        .map(|page| Placement {
            start: PROTECTED + page * PAGE_BYTES,
            size_bits: 12,
        })
        .collect();
    let witness = ledger::Witness {
        guaranteed: payload(2),
        guarantee_pages: 2,
    };
    let plan = Plan::validate_reserved(&sources, &placements, payload(2), witness).unwrap();
    ledger.begin(a, 2, plan).unwrap();
    // The pool is untouched: every byte came from the entitlement's own
    // protected backing.
    assert_eq!(ledger.available(), pool);
    // Only the unspent table envelope is left.
    assert_eq!(
        ledger.guaranteed_available(&entitlement_identity("shared")),
        resources(2).subtract(payload(2))
    );
    ledger.commit(a).unwrap();
    assert_eq!(ledger.pages(a), Ok(2));

    // The guarantee is spent, so a further certified claim against it is
    // refused rather than silently funded from the pool.
    let plan = Plan::validate_reserved(
        &sources[..],
        &placements[..1],
        payload(1),
        ledger::Witness {
            guaranteed: payload(1),
            guarantee_pages: 1,
        },
    )
    .unwrap();
    assert_eq!(ledger.begin(a, 1, plan), Err(ledger::Error::Guarantee));
}

/// A leaf table serves a 2 MiB span, not a payload page, so a failed growth
/// may keep a table for a span past every committed page. Two such rollbacks in
/// distinct spans are both legal and must both settle: a refusal would leave
/// the transaction pending and refuse every later growth of every subject.
#[test]
fn rollback_retains_one_table_per_touched_span_not_per_page() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared"), subject("b", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger = Ledger::admit(
        policy,
        &instances(policy),
        resources(2048),
        &[Resources::ZERO],
    )
    .unwrap();
    let initial = ledger.available();
    let a = ledger.bind(&subject_identity("a")).unwrap();
    let b = ledger.bind(&subject_identity("b")).unwrap();
    let table = Resources {
        bytes: PAGE_BYTES,
        slots: 1,
        descriptors: 1,
        extents: 0,
        tables: 1,
    };
    // A large frame, then one base page and its table in the next span.
    let large_and_page = Plan::validate(
        &[Range {
            start: 0,
            bytes: 4 << 20,
            class: Class::OrdinaryTail,
        }],
        &[
            Placement {
                start: 0,
                size_bits: 21,
            },
            Placement {
                start: 2 << 20,
                size_bits: 12,
            },
            Placement {
                start: (2 << 20) + PAGE_BYTES,
                size_bits: 12,
            },
        ],
        Resources {
            bytes: 514 * PAGE_BYTES,
            slots: 6,
            descriptors: 3,
            extents: 3,
            tables: 1,
        },
    )
    .unwrap();
    ledger.begin(a, 513, large_and_page).unwrap();
    ledger.abort(a, table, true).unwrap();
    // One base page and its table in the first span.
    let page_and_table = Plan::validate(
        &[Range {
            start: 0,
            bytes: 1 << 20,
            class: Class::OrdinaryTail,
        }],
        &[
            Placement {
                start: 0,
                size_bits: 12,
            },
            Placement {
                start: PAGE_BYTES,
                size_bits: 12,
            },
        ],
        Resources {
            bytes: 2 * PAGE_BYTES,
            slots: 4,
            descriptors: 2,
            extents: 2,
            tables: 1,
        },
    )
    .unwrap();
    ledger.begin(a, 1, page_and_table).unwrap();
    assert_eq!(ledger.abort(a, table, true), Ok(()));
    assert_eq!(ledger.pages(a), Ok(0));
    // Both tables stay charged to their holder, and nothing is pending.
    assert_eq!(
        ledger.available(),
        initial.subtract(table).unwrap().subtract(table).unwrap()
    );
    ledger.begin(b, 1, plan(1)).unwrap();
    ledger.commit(b).unwrap();
    ledger.retire(a, true).unwrap();
    ledger.retire(b, true).unwrap();
    assert_eq!(ledger.available(), initial);
}

/// Quarantine taken while a transaction is open survives its abort and
/// refuses its commit: a member whose cleanup is in doubt never gains pages.
#[test]
fn quarantine_is_sticky_across_an_open_transaction() {
    let data = bytes(
        std::vec![entitlement("shared", 0)],
        std::vec![subject("a", "shared")],
        Resources::ZERO,
    );
    let policy = Policy::decode(&data).unwrap();
    let mut ledger =
        Ledger::admit(policy, &instances(policy), resources(8), &[Resources::ZERO]).unwrap();
    let a = ledger.bind(&subject_identity("a")).unwrap();

    ledger.begin(a, 1, plan(1)).unwrap();
    ledger.quarantine(a).unwrap();
    assert_eq!(ledger.commit(a), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.pages(a), Ok(0));
    ledger.abort(a, Resources::ZERO, true).unwrap();
    assert_eq!(ledger.begin(a, 1, plan(1)), Err(ledger::Error::Cleanup));
    assert_eq!(ledger.available(), resources(8));
}

/// A guaranteed leaf table is protected backing but never a redeemed page:
/// a witness may not count table bytes as guarantee pages.
#[test]
fn a_witness_cannot_redeem_its_table_bytes_as_pages() {
    const PROTECTED: u64 = 1 << 40;
    let sources = [
        Range {
            start: PROTECTED,
            bytes: 2 * PAGE_BYTES,
            class: Class::Guaranteed,
        },
        Range {
            start: 0,
            bytes: PAGE_BYTES,
            class: Class::OrdinaryTail,
        },
    ];
    let runs = [
        ledger::Run {
            start: PROTECTED,
            size_bits: 12,
            count: 2,
        },
        ledger::Run {
            start: 0,
            size_bits: 12,
            count: 1,
        },
    ];
    // One guaranteed page and its guaranteed table, one pooled page.
    let guaranteed = Resources {
        bytes: 2 * PAGE_BYTES,
        slots: 2,
        descriptors: 2,
        extents: 0,
        tables: 1,
    };
    let total = guaranteed
        .checked_add(Resources {
            bytes: PAGE_BYTES,
            slots: 2,
            descriptors: 1,
            extents: 1,
            tables: 0,
        })
        .unwrap();
    let certify = |guarantee_pages| {
        Plan::validate_runs(
            &sources,
            &runs,
            total,
            ledger::Witness {
                guaranteed,
                guarantee_pages,
            },
        )
    };
    assert_eq!(certify(2), Err(ledger::Error::Guarantee));
    assert_eq!(certify(1).map(Plan::guarantee_pages), Ok(1));
}
