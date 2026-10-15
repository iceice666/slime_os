//! Host tests for the slot tables.

use super::*;
use std::vec::Vec;

fn storage(slots: usize, key: usize) -> Vec<u8> {
    std::vec![0u8; table_bytes(slots, key)]
}

#[test]
fn a_table_is_the_size_the_contract_counts() {
    assert_eq!(table_bytes(2, 129), 2 * (129 + 16));
    assert_eq!(table_bytes(65_535, 129), 65_535 * 145);
}

#[test]
fn a_slot_holds_its_id_and_key() {
    let mut bytes = storage(4, 20);
    let mut table = TableMut::new(&mut bytes, 4, 20);
    let at = table.insert(7, b"demo/key").expect("insert");
    assert_eq!(table.view().find(7), Some(at));
    assert_eq!(table.view().get(at), Some((7, &b"demo/key"[..])));
    assert_eq!(table.view().find(8), None);
    assert_eq!(table.view().first(), Some((7, &b"demo/key"[..])));
}

#[test]
fn the_last_slot_is_taken_and_the_next_is_refused() {
    for slots in [1usize, 4, 9, 255, 256, 257] {
        let mut bytes = storage(slots, 8);
        let mut table = TableMut::new(&mut bytes, slots, 8);
        for id in 0..slots as u32 {
            table.insert(id + 1, b"k").expect("room");
        }
        assert_eq!(table.insert(slots as u32 + 1, b"k"), Err(InsertError::Full), "{slots}");
        for id in 0..slots as u32 {
            assert!(table.view().find(id + 1).is_some(), "{slots}: {id}");
        }
    }
}

#[test]
fn an_id_already_held_is_a_duplicate_and_a_key_past_the_slot_is_refused() {
    let mut bytes = storage(4, 4);
    let mut table = TableMut::new(&mut bytes, 4, 4);
    table.insert(1, b"abcd").expect("insert");
    assert_eq!(table.insert(1, b"x"), Err(InsertError::Duplicate));
    assert_eq!(table.insert(2, b"abcde"), Err(InsertError::KeyTooLong));
}

#[test]
fn ids_that_collide_are_each_found_and_removal_leaves_the_chain_intact() {
    let mut bytes = storage(4, 4);
    let mut table = TableMut::new(&mut bytes, 4, 4);
    // 1, 5 and 9 all start at slot 1.
    let first = table.insert(1, b"a").expect("1");
    table.insert(5, b"b").expect("5");
    table.insert(9, b"c").expect("9");
    table.remove(first);
    assert_eq!(table.view().find(1), None);
    assert!(table.view().find(5).is_some(), "5 is behind the removed slot");
    assert!(table.view().find(9).is_some(), "9 is behind it too");
    let again = table.insert(13, b"d").expect("reuses the removed slot");
    assert_eq!(again, first);
    assert_eq!(table.view().find(13), Some(first));
}

#[test]
fn a_removed_slot_makes_room_in_a_full_table() {
    let mut bytes = storage(3, 4);
    let mut table = TableMut::new(&mut bytes, 3, 4);
    let slots: Vec<usize> = (1..=3).map(|id| table.insert(id, b"k").expect("room")).collect();
    assert_eq!(table.insert(4, b"k"), Err(InsertError::Full));
    table.remove(slots[1]);
    table.insert(4, b"k").expect("one slot is free");
    assert_eq!(table.insert(5, b"k"), Err(InsertError::Full));
}

#[test]
fn clear_empties_every_slot() {
    let mut bytes = storage(5, 4);
    let mut table = TableMut::new(&mut bytes, 5, 4);
    for id in 1..=5 {
        table.insert(id, b"k").expect("room");
    }
    table.clear();
    assert_eq!(table.view().first(), None);
    assert_eq!(table.view().find(3), None);
    table.insert(3, b"k").expect("room again");
}

#[test]
fn ids_use_all_32_bits() {
    let mut bytes = storage(4, 4);
    let mut table = TableMut::new(&mut bytes, 4, 4);
    let ids = [0u32, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF];
    for id in ids {
        table.insert(id, b"k").expect("insert");
    }
    for id in ids {
        let at = table.view().find(id).expect("found");
        assert_eq!(table.view().get(at).map(|(held, _)| held), Some(id));
    }
}

#[test]
fn every_slot_of_a_table_of_65535_is_taken_found_and_freed() {
    let slots = 65_535usize;
    let mut bytes = storage(slots, 16);
    let mut table = TableMut::new(&mut bytes, slots, 16);
    for id in 1..=slots as u32 {
        table.insert(id, &id.to_le_bytes()).expect("room");
    }
    assert_eq!(table.insert(65_536, b"k"), Err(InsertError::Full));
    for id in [1u32, 255, 256, 257, 32_768, 65_534, 65_535] {
        let at = table.view().find(id).expect("found");
        assert_eq!(table.view().get(at), Some((id, &id.to_le_bytes()[..])));
    }
    for id in 1..=slots as u32 {
        let at = table.view().find(id).expect("found");
        table.remove(at);
    }
    assert_eq!(table.view().first(), None);
}
