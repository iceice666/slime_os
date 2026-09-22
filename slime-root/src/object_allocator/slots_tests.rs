use super::slots::{SlotPool, SlotWord};

#[test]
fn scratch_planning_never_changes_live_slots_and_restarts_exactly() {
    let mut pool = SlotPool::new(100..108).unwrap();
    for _ in 0..8 {
        pool.allocate(0).unwrap();
    }
    for slot in [100, 102, 104, 106] {
        assert!(pool.release(slot));
    }
    for _ in 0..2 {
        let mut plan = pool.planner();
        assert_eq!(plan.allocate(0).unwrap().0, 100);
        assert!(plan.allocate_contiguous(2, 0).is_err());
        assert_eq!(plan.allocate(0).unwrap().0, 102);
    }
    assert_eq!(pool.free(), 4);
    assert_eq!(pool.allocate(0).unwrap(), (100, true));
    assert_eq!(pool.allocate(0).unwrap(), (102, true));
    assert!(pool.release(100));
    assert_eq!(pool.allocate(0).unwrap(), (100, true));
}

#[test]
fn expanded_slots_cross_metadata_pages_without_crossing_leaf_retypes() {
    let mut pool = SlotPool::new(100..102).unwrap();
    pool.words.provision_host(100, SlotWord::EMPTY);
    let high = 1usize << 60;
    pool.admit(high..high + 2048).unwrap();
    assert_eq!(pool.allocate_contiguous(2, 0).unwrap().0, 100);
    assert_eq!(pool.allocate_contiguous(1023, 0).unwrap().0, high);
    assert_eq!(pool.allocate_contiguous(2, 0).unwrap().0, high + 1024);
    assert_eq!(pool.allocate(0).unwrap().0, high + 1023);
    assert!(pool.release(high + 100));
    assert_eq!(pool.allocate(0).unwrap(), (high + 100, true));
    assert!(!pool.release(high + 2048));
    assert!(pool.admit(high + 2047..high + 2049).is_err());
}
