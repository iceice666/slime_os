#![no_std]
#![no_main]
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_rt::{
    ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, debug_write, exit, yield_now,
};
slime_rt::entry!(main);
const SERVICE: u32 = 0;
fn main(_: u32) {
    deny(
        "alternate address",
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"other.test",
            4242,
        ),
    );
    deny(
        "alternate port",
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"echo.test",
            4243,
        ),
    );
    deny("alternate dns name", resolve(b"other.test"));
    deny(
        "wrong transport",
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_UDP,
            b"echo.test",
            4242,
        ),
    );
    deny(
        "missing CONNECT",
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"send-only.test",
            6000,
        ),
    );
    deny("missing SEND", capability(network_service::OP_SEND, 999));
    deny("missing RECV", capability(network_service::OP_RECV, 999));
    deny(
        "missing LISTEN",
        destination(
            network_service::OP_LISTEN,
            network_service::TRANSPORT_TCP,
            b"echo.test",
            4242,
        ),
    );
    deny("raw-packet attempt", malformed_raw());
    deny("resolver-wide lookup", resolve(b"*"));
    deny(
        "listen without LISTEN",
        destination(
            network_service::OP_LISTEN,
            network_service::TRANSPORT_TCP,
            b"send-only.test",
            6000,
        ),
    );
    debug_write(b"[io-network-intruder] every denial structured packets=0\n");
    exit(0)
}
fn deny(name: &str, request: WireNetworkRequest) {
    let reply = call(request);
    if reply.status_detail >= 0 {
        fail(b"denial accepted")
    }
    debug_write(b"[io-network-intruder] denied ");
    debug_write(name.as_bytes());
    debug_write(b" packets=0\n");
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
fn malformed_raw() -> WireNetworkRequest {
    WireNetworkRequest {
        magic: network_service::NETWORK_MAGIC,
        version: network_service::FORMAT_VERSION,
        op: 99,
        transport: network_service::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: 0,
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
    debug_write(b"[io-network-intruder] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
