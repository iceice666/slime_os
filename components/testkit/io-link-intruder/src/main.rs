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
    for name in UNGRANTED {
        if resolve_binding(name).is_ok() {
            fail(b"ungranted raw link binding resolved");
        }
    }
    debug_write(b"[io-link-intruder] denied transmit=1 receive=1 query=1 raw=1 emitted=0\n");
    exit(0)
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-link-intruder] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
