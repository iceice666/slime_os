use crate::run::{KERNEL_TESTS, TestResult};
use crate::{print, println};
use linkme::distributed_slice;

#[distributed_slice(KERNEL_TESTS)]
fn trivial_assertion() -> TestResult {
    print!("trivial assertion... ");
    assert_eq!(1, 1);
    println!("[ok]");

    Ok(())
}
