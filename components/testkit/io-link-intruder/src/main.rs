#![no_std]
#![no_main]

use slime_rt::{debug_write, exit, resolve_binding};

slime_rt::entry!(main);

const UNGRANTED: [&[u8]; 8] = [
    b"io-link-peer",
    b"notification:io-link-tx-request-ready+signal",
    b"notification:io-link-rx-request-ready+signal",
    b"notification:io-link-tx-completion-ready+wait",
    b"notification:io-link-rx-completion-ready+wait",
    b"notification:io-link-state-changed+wait",
    b"virtio-net-device",
    b"virtio-net-dma",
];

fn main(_startup_arg: u32) {
    let mut refused = 0;
    for name in UNGRANTED {
        if resolve_binding(name).is_ok() {
            fail(b"ungranted raw link binding resolved");
        }
        refused += 1;
    }
    write_number(b"[io-link-intruder] binding-resolution refused=", refused);
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

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-link-intruder] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
