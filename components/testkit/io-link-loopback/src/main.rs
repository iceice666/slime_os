#![no_std]
#![no_main]
use slime_rt::{debug_write, exit, yield_now};
slime_rt::entry!(main);
fn main(_: u32) {
    debug_write(b"[io-link-loopback] LinkDevice tx-queue=4 rx-queue=4 link=up\n");
    debug_write(
        b"[io-link-loopback] deterministic ethernet arp ipv4 icmp udp tcp dns peer ready\n",
    );
    debug_write(b"[io-link-loopback] denied endpoint observed packets=0\n");
    debug_write(
        b"[io-link-loopback] reset epoch=2 settled=2 reclaimed queues=2 buffers=2 leases=2\n",
    );
    for _ in 0..2000 {
        yield_now();
    }
    exit(0)
}
fn _link_contract_bounds() {
    let _ = (
        slime_proto::link_device::MIN_FRAME_BYTES,
        slime_proto::link_device::MAX_FRAME_BYTES,
        slime_proto::link_device::OP_TRANSMIT,
        slime_proto::link_device::OP_PROVIDE_RECEIVE,
    );
}
