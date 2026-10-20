#![no_std]
#![no_main]

//! The subscriber of the `sel4-zenoh-external` composition. That composition's subscriber instance
//! holds no network authority and the listener is a stock peer on the host, so there is nothing
//! for it to do; it exits at once.

slime_rt::entry!(main);

fn main(_: u32) {
    slime_rt::exit(0)
}
