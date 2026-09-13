#![no_std]
#![no_main]
use slime_rt::{debug_write, exit};
slime_rt::entry!(main);
fn main(_: u32) {
    let endpoint_bindings =
        u64::from(slime_rt::resolve_binding(b"network-service-link-device").is_ok());
    let protocol_operations = 0u64;
    write_number(
        b"[io-link-loopback] declared endpoint bindings=",
        endpoint_bindings,
    );
    write_number(b" protocol operations=", protocol_operations);
    debug_write(b"\n");
    exit(0)
}
fn write_number(prefix: &[u8], mut value: u64) {
    let mut digits = [0u8; 20];
    let mut offset = digits.len();
    loop {
        offset -= 1;
        digits[offset] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    debug_write(prefix);
    debug_write(&digits[offset..]);
}
