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
    assert_eq!(ledger.abort(a, table, true), Err(ledger::Error::Cleanup));
    ledger.abort(a, Resources::ZERO, true).unwrap();
    ledger.retire(a, true).unwrap();
    assert_eq!(ledger.available(), resources(4));
}
