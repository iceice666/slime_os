//! B22: native endpoint authority crossing a spawn boundary.
//!
//! Extracted from `init.rs` by B65: 21 plane launchers in one 2286-line
//! binary meant every plane's edit shared a file with every other plane's.
//! Holds this plane and the helpers only it uses.
//!
//! Init's slot numbers arrive by `include!` of the generated per-generation
//! boot layout into `init.rs`'s scope, so anything from it is reached through
//! `super` — there is no path naming that layout independently of its binary.

use super::{CROSSING_PEER_SLOT, RIGHT_SEND};
use slime_rt::CapabilityDisposition;

/// More rendezvous exchanges than the retired logical lifetime bound of 48.
/// The direct endpoint is static, so this proves transport stays live without
/// depending on root-mediated channel allocation or sweeping.
const CHANNEL_LOOP_PAIRS: u32 = 49;
/// Drive the direct endpoint crossing plane with one-cap narrowed copy
/// delegation and sustained native request/reply rendezvous.
pub fn drive_crossing_plane() {
    const CARRIER_SLOT: u32 = 2;
    const GATE_SLOT: u32 = 3;
    let peer = slime_rt::spawn(CROSSING_PEER_SLOT, &[])
        .unwrap_or_else(|_| fail_crossing(b"crossing peer"));
    let descriptor = slime_proto::capability_transfer::WireCapabilityTransfer {
        magic: slime_proto::capability_transfer::CAPABILITY_TRANSFER_MAGIC,
        version: slime_proto::capability_transfer::FORMAT_VERSION,
        status: 0,
        flags: slime_proto::capability_transfer::FLAG_RETAIN_TRANSFER,
        object_kind: slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        direction: 0,
        rights_mask: RIGHT_SEND,
        route_identity: [0u8; 32],
    };
    if slime_rt::capability_delegate(
        CARRIER_SLOT,
        GATE_SLOT,
        CapabilityDisposition::Retain,
        slime_proto::capability_transfer::OBJECT_KIND_ENDPOINT,
        RIGHT_SEND,
        &descriptor.encode(),
    ) != slime_rt::ERR_SUCCESS
    {
        fail_crossing(b"delegate narrowed endpoint");
    }
    slime_rt::debug_write(b"[init] endpoint capability exported before crossing\n");
    let mut payload = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(GATE_SLOT, &mut payload, &mut caps) {
            slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
            8 if &payload[..8] == b"survived" => break,
            _ => fail_crossing(b"sender copy did not remain usable"),
        }
    }
    slime_rt::debug_write(b"[init] sender retained delegated authority\n");
    slime_rt::debug_write(b"[init] imported endpoint survived crossing\n");
    for _ in 0..CHANNEL_LOOP_PAIRS {
        if slime_rt::send(CARRIER_SLOT, b"ping", &[]) != slime_rt::ERR_SUCCESS {
            fail_crossing(b"native crossing send");
        }
        loop {
            match slime_rt::recv(GATE_SLOT, &mut payload, &mut caps) {
                slime_rt::ERR_WOULDBLOCK => slime_rt::yield_now(),
                4 if &payload[..4] == b"pong" => break,
                _ => fail_crossing(b"native crossing reply"),
            }
        }
    }
    slime_rt::debug_write(b"[init] channel lifetime bound crossed\n");
    loop {
        match slime_rt::supervision_status(peer.supervision_slot) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(slime_rt::Termination::Exit(0))) => break,
            _ => fail_crossing(b"crossing peer failed"),
        }
    }
}
fn fail_crossing(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[init] crossing plane fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
