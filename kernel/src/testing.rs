#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}

#[test_case]
fn panic_assertion() {
    assert_eq!(1,0);
}