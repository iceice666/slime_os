#[test_case]
fn trivial_assertion() {
    assert_eq!([1, 1].len(), 2);
}

#[test_case]
fn nonzero_assertion() {
    assert_ne!(1, 0);
}
