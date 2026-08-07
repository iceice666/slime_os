#![no_std]
#![no_main]

use slime_rt::{ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG};

slime_rt::entry!(main);

/// The edge a transferred channel end arrives on. First spawn grant, so slot 0.
const CARRIER_SLOT: u32 = 0;
/// The edge that releases this peer. Second spawn grant, so slot 1.
const GATE_SLOT: u32 = 1;

/// A peer that holds a capability in flight across the channel crossing.
///
/// B22's gate needs a channel end parked in `Transit` — held by no capability
/// table at all — while init mints past `MAX_CHANNELS`. That state exists only
/// between a `cap_transfer` and the matching `recv`, so the receiver has to be
/// something that provably does *not* receive during the loop. Every other
/// unmodified component either ignores the capability array or drains its only
/// queue immediately, which closes the window before the first sweep fires.
///
/// So this waits on a **second** edge first. Init transfers the end over
/// `CARRIER_SLOT`, runs its loop while this task is parked on `GATE_SLOT`, and
/// only then writes the gate. The transferred end is in flight across every
/// sweep the loop triggered, which is the arm a predicate over live capability
/// tables alone would break.
///
/// It then exercises the landed end in both directions. That is the actual
/// claim: not merely that a slot number arrived, but that the channel it names
/// still resolves to a queue. Init dropped its own half of that pair before the
/// loop, so the transit entry was the only thing naming the channel — a sweep
/// that skips `Transit` frees it, and both operations then answer `ERR_BAD_CAP`
/// because `resolve_channel` finds no entry for the key the capability carries.
///
/// A send **and** a non-blocking receive, but not a round trip. `cap_transfer`
/// ran `ChannelTable::reassign` on a loopback, which takes the split branch:
/// init stays `producer`, this task becomes `consumer`, and the `reverse` queue
/// is allocated at that moment. So this task's send resolves to `reverse` and
/// its receive resolves to `forward` — two distinct queues, and init is the
/// only task that could ever enqueue on `forward`, which it no longer can.
/// The receive therefore expects `ERR_WOULDBLOCK`, and getting it proves the
/// entry resolves in the direction the send does not exercise. Delivery is the
/// channel plane's property and `sel4_channel_check` owns it.
///
/// The exit status is the verdict init reads through its supervision handle,
/// since init gave up both of the pair's slots and cannot observe the end
/// itself. Every failure names itself first: init reports one message for a
/// non-zero exit, and that message is the string B22's second fault injection
/// is identified by, so a wrong-cause failure must not be able to impersonate it.
fn main() {
    let mut payload = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];

    // The root launches every component the generation declares (P5.2), so this
    // boot also starts one *unconfigured* instance that init never spawned and
    // never granted a channel. It holds no slots at all, so its first `recv`
    // answers `ERR_BAD_CAP` rather than `ERR_WOULDBLOCK`, and it exits quietly:
    // this task is not the plane's subject and must not report a failure the
    // gate would read as the transit arm breaking.
    //
    // `ERR_BAD_CAP` at *this* point is unambiguous. The spawned instance
    // resolves both grants at construction, so it cannot reach here without
    // them; only the root-launched one can.
    //
    // The spawned instance parks here for the whole crossing. `wait` rather
    // than a spin, so the root records one park rather than a busy loop.
    loop {
        match slime_rt::recv(GATE_SLOT, &mut payload, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[slime_rt::WaitSource::Endpoint(GATE_SLOT)]),
            slime_rt::ERR_BAD_CAP => return,
            n if n < 0 => fail(b"the release gate stopped answering"),
            _ => break,
        }
    }

    // The transfer has been queued since before the loop began; collecting it
    // now is what ends the in-flight window.
    let landed = loop {
        match slime_rt::recv(CARRIER_SLOT, &mut payload, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[slime_rt::WaitSource::Endpoint(CARRIER_SLOT)]),
            n if n < 0 => fail(b"collecting the transferred end"),
            _ => break caps[0] as u32,
        }
    };
    // Distinct from a failed send: the capability never arrived at all, which
    // is a transit-table defect rather than a freed channel.
    if landed == 0 {
        fail(b"the transferred end landed in no slot");
    }
    if slime_rt::send(landed, b"survived", &[]) != slime_rt::ERR_SUCCESS {
        fail(b"the collected end no longer resolves for send");
    }
    if slime_rt::recv(landed, &mut payload, &mut caps) != ERR_WOULDBLOCK {
        fail(b"the collected end no longer resolves for receive");
    }
    slime_rt::debug_write(b"[crossing-peer] carried on the end it collected\n");
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[crossing-peer] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}
