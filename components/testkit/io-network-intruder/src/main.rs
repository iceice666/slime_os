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
    let mut exact_refusals = 0u64;
    for request in [
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"other.test",
            4242,
        ),
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"echo.test",
            4243,
        ),
        resolve(b"other.test"),
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_UDP,
            b"echo.test",
            4242,
        ),
        destination(
            network_service::OP_CONNECT,
            network_service::TRANSPORT_TCP,
            b"send-only.test",
            6000,
        ),
        resolve(b"send-only.test"),
        destination(
            network_service::OP_LISTEN,
            network_service::TRANSPORT_TCP,
            b"send-only.test",
            6000,
        ),
        resolve(b"*"),
    ] {
        deny(request);
        exact_refusals += 1;
    }
    Line::<128>::new()
        .bytes(b"[io-network-intruder] exact authority refusals=")
        .decimal(exact_refusals)
        .bytes(b"\n")
        .emit();

    // Guessed IDs can name either another holder's legacy handle or no live
    // handle at all; this probe does not establish which denial branch ran.
    let mut guessed = 0u64;
    for id in 1..=4 {
        let reply = call(capability(network_service::OP_SEND, id));
        if reply.status_detail >= 0 {
            fail(b"guessed accepted");
        }
        guessed += u64::from(reply.status_detail < 0);
    }
    Line::<128>::new()
        .bytes(b"[io-network-intruder] guessed capability refusals=")
        .decimal(guessed)
        .bytes(b"\n")
        .emit();

    let recv_only = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"recv-only.test",
        6001,
    ));
    if recv_only.status_detail != 0 {
        fail(b"recv-only connect");
    }
    let missing_send = call(capability(network_service::OP_SEND, recv_only.capability));
    if missing_send.status_detail >= 0 {
        fail(b"missing send accepted");
    }
    let send_path = call(destination(
        network_service::OP_CONNECT,
        network_service::TRANSPORT_TCP,
        b"send-path.test",
        6002,
    ));
    if send_path.status_detail != 0 {
        fail(b"send-path connect");
    }
    let missing_recv = call(capability(network_service::OP_RECV, send_path.capability));
    if missing_recv.status_detail >= 0 {
        fail(b"missing recv accepted");
    }
    let rights_mask_refusals =
        u64::from(missing_send.status_detail < 0) + u64::from(missing_recv.status_detail < 0);
    Line::<128>::new()
        .bytes(b"[io-network-intruder] rights-mask refusals=")
        .decimal(rights_mask_refusals)
        .bytes(b"\n")
        .emit();

    for id in [recv_only.capability, send_path.capability] {
        if call(capability(network_service::OP_CLOSE, id)).status_detail != 0 {
            fail(b"close");
        }
    }

    let shutdown = call(capability(network_service::OP_CLOSE, SHUTDOWN_CAPABILITY));
    if shutdown.status_detail != 0 {
        fail(b"shutdown");
    }
    let shutdown_observed = u64::from(shutdown.status_detail == 0);
    Line::<128>::new()
        .bytes(b"[io-network-intruder] structured denials=")
        .decimal(exact_refusals + guessed + rights_mask_refusals)
        .bytes(b" shutdown=")
        .decimal(shutdown_observed)
        .bytes(b"\n")
        .emit();
    exit(0)
}
fn deny(request: WireNetworkRequest) {
    if call(request).status_detail >= 0 {
        fail(b"denial accepted")
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
        .bytes(b"[io-network-intruder] fail: ")
        .bytes(reason)
        .bytes(b"\n")
        .emit();
    exit(1)
}
