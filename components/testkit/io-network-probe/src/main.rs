#![no_std]
#![no_main]
use slime_proto::network_service::{self, WireNetworkCompletion, WireNetworkRequest};
use slime_proto::valid_network_completion;
use slime_rt::console_line::Line;
use slime_rt::{ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG, exit, yield_now};
slime_rt::entry!(main);
const SERVICE: u32 = 0;
const SHUTDOWN_CAPABILITY: u64 = u64::MAX;
fn main(_: u32) {
    let mut tcp_capabilities = 0u64;
    let mut successful_transfers = 0u64;
    let tcp = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4242,
    ));
    if tcp.status_detail != 0 || tcp.capability_kind != network_service::CAPABILITY_TCP_CONNECTION {
        fail(b"tcp connect");
    }
    tcp_capabilities += 1;
    Line::<256>::new()
        .bytes(b"[io-network-probe] tcp capabilities=")
        .decimal(tcp_capabilities)
        .bytes(b" rights=connect,send,recv\n")
        .emit();
    for op in [network_service::OP_SEND, network_service::OP_RECV] {
        let reply = call(capability(op, tcp.capability));
        if reply.status_detail != 0 {
            fail(b"tcp transfer");
        }
        successful_transfers += 1;
    }
    Line::<256>::new()
        .bytes(b"[io-network-probe] successful capability operations=")
        .decimal(successful_transfers)
        .bytes(b"\n")
        .emit();

    let wrong_port = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4243,
    ));
    require_denied(wrong_port, b"wrong port");
    let exact_refusals = u64::from(wrong_port.status_detail < 0);
    Line::<256>::new()
        .bytes(b"[io-network-probe] exact destination refusals=")
        .decimal(exact_refusals)
        .bytes(b"\n")
        .emit();

    let dns = call(resolve(b"echo.test"));
    if dns.status_detail != 0 || dns.capability_kind != network_service::CAPABILITY_DNS_RECORD {
        fail(b"dns");
    }
    let dns_exhausted = call(resolve(b"echo.test"));
    require_denied(dns_exhausted, b"dns budget");
    let dns_records = u64::from(dns.status_detail == 0);
    let dns_budget_refusals = u64::from(dns_exhausted.status_detail < 0);
    Line::<256>::new()
        .bytes(b"[io-network-probe] dns records=")
        .decimal(dns_records)
        .bytes(b" budget_refusals=")
        .decimal(dns_budget_refusals)
        .bytes(b"\n")
        .emit();

    let udp = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_UDP,
        b"echo.test",
        5353,
    ));
    if udp.status_detail != 0 || udp.capability_kind != network_service::CAPABILITY_UDP_ENDPOINT {
        fail(b"udp connect");
    }
    let tcp_second = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4242,
    ));
    if tcp_second.status_detail != 0 {
        fail(b"second tcp connect");
    }
    let socket_exhausted = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"echo.test",
        4242,
    ));
    require_denied(socket_exhausted, b"socket budget");
    let socket_charges =
        u64::from(tcp.status_detail == 0) + u64::from(tcp_second.status_detail == 0);
    let socket_budget_refusals = u64::from(socket_exhausted.status_detail < 0);
    Line::<256>::new()
        .bytes(b"[io-network-probe] socket charges=")
        .decimal(socket_charges)
        .bytes(b" budget_refusals=")
        .decimal(socket_budget_refusals)
        .bytes(b"\n")
        .emit();

    let mut closed_capabilities = 0u64;
    for id in [
        tcp.capability,
        dns.capability,
        udp.capability,
        tcp_second.capability,
    ] {
        let closed = call(capability(network_service::OP_CLOSE, id));
        if closed.status_detail != 0 {
            fail(b"close");
        }
        closed_capabilities += 1;
    }
    let shutdown = call(capability(network_service::OP_CLOSE, SHUTDOWN_CAPABILITY));
    if shutdown.status_detail != 0 {
        fail(b"shutdown");
    }
    Line::<256>::new()
        .bytes(b"[io-network-probe] closed capabilities=")
        .decimal(closed_capabilities)
        .bytes(b" shutdown=")
        .decimal(u64::from(shutdown.status_detail == 0))
        .bytes(b"\n")
        .emit();
    exit(0)
}
fn require_denied(reply: WireNetworkCompletion, reason: &[u8]) {
    if reply.status_detail >= 0 {
        fail(reason);
    }
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
                let reply = WireNetworkCompletion::decode(&out[..result as usize])
                    .unwrap_or_else(|| fail(b"reply"));
                if !valid_network_completion(&reply) {
                    fail(b"invalid completion");
                }
                return reply;
            }
        }
    }
}
fn fail(reason: &[u8]) -> ! {
    Line::<256>::new()
        .bytes(b"[io-network-probe] fail: ")
        .bytes(reason)
        .bytes(b"\n")
        .emit();
    exit(1)
}
