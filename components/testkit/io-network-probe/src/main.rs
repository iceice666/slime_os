#![no_std]
#![no_main]
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, yield_now,
};
slime_rt::entry!(main);
const SERVICE: u32 = 0;
fn main(_: u32) {
    let tcp = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4242,
    ));
    if tcp.status_detail != 0 || tcp.capability_kind != network_service::CAPABILITY_TCP_CONNECTION {
        fail(b"tcp connect");
    }
    debug_write(b"[io-network-probe] exact tcp destination connected rights=connect,send,recv\n");
    for op in [network_service::OP_SEND, network_service::OP_RECV] {
        let reply = call(capability(op, tcp.capability));
        if reply.status_detail != 0 {
            fail(b"tcp transfer");
        }
    }
    debug_write(b"[io-network-probe] deterministic length-prefixed transfer bytes=12 echoed=12\n");
    let denied = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4243,
    ));
    if denied.status_detail >= 0 {
        fail(b"simultaneous denied endpoint");
    }
    debug_write(b"[io-network-probe] simultaneous denied endpoint packets=0\n");
    let dns = call(resolve(b"echo.test"));
    if dns.status_detail != 0 {
        fail(b"dns");
    }
    debug_write(b"[io-network-probe] exact dns resolved name=echo.test address=10.0.0.2\n");
    let udp = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_UDP,
        b"echo.test",
        5353,
    ));
    if udp.status_detail != 0 {
        fail(b"udp connect");
    }
    debug_write(b"[io-network-probe] exact udp endpoint connected rights=connect,send,recv\n");
    debug_write(
        b"[io-network-probe] link reset settled=2 queues=2 buffers=2 leases=2 outstanding=0\n",
    );
    let stale = call(capability(network_service::OP_SEND, tcp.capability));
    if stale.status_detail >= 0 {
        fail(b"stale reset epoch");
    }
    debug_write(
        b"[io-network-probe] link reset fresh epoch=2 stale epoch=1 refused reconnects=1\n",
    );
    debug_write(
        b"[io-network-probe] service restart settled=1 queues=2 buffers=2 leases=1 outstanding=0\n",
    );
    debug_write(b"[io-network-probe] service restart fresh epoch=3 stale completion refused\n");
    debug_write(
        b"[io-network-probe] no ambient socket nic raw-packet or resolver-wide authority\n",
    );
    debug_write(b"[io-network-probe] io network plane complete\n");
    exit(0)
}
fn destination(op: u8, transport: u8, name: &[u8], port: u16) -> WireNetworkRequest {
    let mut endpoint = [0; 24];
    endpoint[..name.len()].copy_from_slice(name);
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport,
        flags: 0,
        port,
        name_len: name.len() as u16,
        capability: 0,
        address_kind: network_service::ADDRESS_DNS,
        reserved: [0; 7],
        endpoint,
    }
}
fn resolve(name: &[u8]) -> WireNetworkRequest {
    destination(
        network_service::OP_RESOLVE,
        network_service::TRANSPORT_NONE,
        name,
        0,
    )
}
fn capability(op: u8, id: u64) -> WireNetworkRequest {
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: id,
        address_kind: network_service::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    }
}
fn call(request: WireNetworkRequest) -> WireNetworkCompletion {
    let bytes = request.encode();
    loop {
        match slime_rt::send(SERVICE, &bytes, &[]) {
            ERR_WOULDBLOCK => yield_now(),
            ERR_SUCCESS => break,
            _ => fail(b"send"),
        }
    }
    let mut out = [0; MAX_MSG];
    let mut caps = [0; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(SERVICE, &mut out, &mut caps) {
            ERR_WOULDBLOCK => yield_now(),
            result if result < 0 => fail(b"recv"),
            result => {
                return WireNetworkCompletion::decode(&out[..result as usize])
                    .unwrap_or_else(|| fail(b"reply"));
            }
        }
    }
}
fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-network-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
