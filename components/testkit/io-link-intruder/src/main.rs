#![no_std]
#![no_main]

use slime_rt::console_line::Line;
use slime_rt::{exit, resolve_binding};

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
    Line::<96>::new()
        .bytes(b"[io-link-intruder] binding-resolution refused=")
        .decimal(refused)
        .bytes(b"\n")
        .emit();
    exit(0)
}

fn fail(reason: &[u8]) -> ! {
    Line::<256>::new()
        .bytes(b"[io-link-intruder] fail: ")
        .bytes(reason)
        .bytes(b"\n")
        .emit();
    exit(1)
}
