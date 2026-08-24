#![no_std]
#![no_main]

//! C9.2's plane: a bounded wait set over one declared Notification.
//!
//! Three instances of this binary, told apart by the sources the generation
//! declares for each — the same shape `clock-authority-probe` uses, and for the
//! same reason: what a component may do is authenticated generation data, so the
//! role is discovered rather than compiled in.
//!
//! - `wait-set-waiter` registers a stream source, a timer source, and a
//!   supervision source on one Notification, spawns the peer it supervises, and
//!   blocks. The evidence it emits is the whole point of the milestone: one wake
//!   carrying several badges, every ready source recovered from that one word,
//!   dispatch in the documented ascending-badge order, and each ceiling refused
//!   with its own error while the set stays usable.
//! - `wait-set-signaller` signals its declared badge on the same object.
//! - `wait-set-denied` is named by no wait-set entry: its declared set is empty,
//!   every registration is refused, and that is the deny-by-default answer
//!   rather than an unbounded one.

use boot_contracts::wait_set::SourceKind;
use slime_rt::wait_set::{MAX_CALLBACKS_PER_WAKE, MAX_SOURCES};
use slime_rt::{
    ERR_SUCCESS, WaitError, WaitSet, debug_write, exit, resolve_binding, send, spawn, timer_arm,
    yield_now,
};

/// The waiter's declared wait binding on `wait-set-wake`.
const WAKE_SLOT: u32 = 0;
/// Spins the waiter allows the signaller and its peer before concluding a
/// missing wake. Bounded so a stall is a named failure rather than a hang.
const SETTLE_SPINS: usize = 200_000;

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    // Which role this instance is, read from the sources the generation declares
    // for it. A waiter has sources; a signaller has the endpoint it signals
    // through; a denied instance has neither.
    let declared = WaitSet::declared(WAKE_SLOT);
    match declared {
        Ok(set) if !set.declarations().is_empty() => run_waiter(set),
        Ok(set) => {
            // No declared sources. Either the signaller — which holds a message
            // endpoint — or the denied instance, which holds nothing.
            if resolve_binding(b"notification:wait-set-wake+signal").is_ok() {
                run_signaller()
            } else {
                run_denied(set)
            }
        }
        Err(error) => fail_with(b"declared source read", error),
    }
}

/// Register three sources on one Notification, block once, and dispatch the
/// whole ready set.
fn run_waiter(mut set: WaitSet) -> ! {
    let declared = *set.declarations();
    if declared.len() != 3 {
        fail(b"waiter did not receive its three declared sources")
    }
    // Named by slot and by kind, never by badge: the component knows which of
    // its own slots carries the message endpoint, and the generation supplies
    // the bit. That is what keeps a peer's slot number out of this binary.
    let stream = set
        .register_slot(SourceKind::Stream, 0)
        .unwrap_or_else(|error| fail_with(b"register stream source", error));
    let timer = set
        .register_timer()
        .unwrap_or_else(|error| fail_with(b"register timer source", error));
    let supervision = set
        .register_slot(SourceKind::Supervision, 1)
        .unwrap_or_else(|error| fail_with(b"register supervision source", error));
    if stream.badge == timer.badge || timer.badge == supervision.badge {
        fail(b"declared sources aliased one badge")
    }
    write_hex(b"[wait-set:waiter] registered=3 mask=", set.mask());

    // Every ceiling refuses with its own error and leaves the set usable. Proven
    // before the wake, so a later dispatch is evidence the refusals were
    // non-destructive rather than merely reported.
    if set.register(stream.badge)
        != Err(WaitError::Registry(
            boot_contracts::wait_set::dispatch::RegistryError::DuplicateSource,
        ))
    {
        fail(b"a second registration of one badge was admitted")
    }
    if set.register(1 << 40)
        != Err(WaitError::Registry(
            boot_contracts::wait_set::dispatch::RegistryError::UndeclaredSource,
        ))
    {
        fail(b"an undeclared badge was registered")
    }
    if set
        .dispatch_bounded(MAX_CALLBACKS_PER_WAKE + 1, |_| {
            fail(b"an over-budget dispatch ran a handler")
        })
        .is_ok()
    {
        fail(b"the callback ceiling did not bind")
    }
    if set.registered() != 3 || set.ready_queued() != 0 {
        fail(b"a refused operation disturbed the wait set")
    }
    debug_write(b"[wait-set:waiter] ceilings duplicate=1 undeclared=1 callbacks=1 usable=1\n");

    // Make two sources ready *before* the first block, so the coalescing this
    // milestone exists to demonstrate is deterministic rather than a scheduling
    // accident: seL4 ORs the badges of signals that arrive while nobody is
    // waiting, so a wake that follows both must carry both bits.
    //
    // The supervised peer is the first: it exits immediately, and the root
    // signals the declared supervision badge when it does.
    let peer = spawn(
        // The grant's own name, not `executable:` — that axis is the bootstrap
        // instance's layout view, and this component is not it. A grant name is
        // what every non-init spawner resolves.
        resolve_binding(b"wait-set-peer-executable")
            .unwrap_or_else(|_| fail(b"resolve supervised peer executable")),
        &[],
    )
    .unwrap_or_else(|_| fail(b"spawn supervised peer"));
    if peer.supervision_slot != 1 {
        fail(b"supervised peer handle did not land in its declared slot")
    }
    // The second is the signaller's message badge. Poll until both are pending,
    // recording the widest *single* answer rather than the running total: a sum
    // reaching two cannot tell one coalesced word carrying two badges from two
    // separate single-badge wakes, and the coalescing is the whole property this
    // plane exists to observe. Both sources are made ready before any block, so
    // the kernel ORs their badges onto the one notification and a single poll
    // must see both.
    let mut queued = 0;
    let mut widest = 0;
    let mut settled = 0;
    while settled < SETTLE_SPINS && queued < 2 {
        let batch = set.poll().unwrap_or_else(|error| fail_with(b"poll", error));
        if batch > widest {
            widest = batch;
        }
        queued += batch;
        settled += 1;
        yield_now();
    }
    if widest != 2 || queued != 2 || set.ready_queued() != 2 {
        fail(b"two independently signalled sources did not coalesce into one wake")
    }
    // Dispatch that ready set: both sources recovered from one badge word, in
    // ascending order, which is the contract's tie rule and therefore the
    // property repeated boots must reproduce.
    let mut order = [0u64; MAX_SOURCES];
    let mut count = 0;
    let mut seen_stream = false;
    let mut seen_supervision = false;
    let dispatched = set
        .dispatch(|ready| {
            order[count] = ready.badge;
            count += 1;
            match ready.kind {
                SourceKind::Stream => {
                    seen_stream = true;
                    // A badge means readiness, not a message count, so the
                    // endpoint is drained until it would block.
                    let slot = ready
                        .drain_slot
                        .unwrap_or_else(|| fail(b"stream source slot"));
                    let mut buffer = [0u8; slime_rt::MAX_MSG];
                    let mut drained = 0;
                    while let Ok(Some(_)) = slime_rt::wait_set::drain(slot, &mut buffer) {
                        drained += 1;
                    }
                    if drained == 0 {
                        fail(b"a ready stream source drained nothing")
                    }
                }
                SourceKind::Supervision => seen_supervision = true,
                _ => fail(b"dispatched an undeclared source kind"),
            }
        })
        .unwrap_or_else(|error| fail_with(b"dispatch", error));
    if dispatched != 2 || !seen_stream || !seen_supervision {
        fail(b"one wake did not carry both signalled sources")
    }
    if order[0] >= order[1] {
        fail(b"dispatch order was not ascending by badge")
    }
    if order[0] != stream.badge || order[1] != supervision.badge {
        fail(b"dispatch order did not follow the declared badge order")
    }
    // Both numbers are live and read *after* dispatch: `widest` is what one
    // badge word actually carried — the widest single poll, not the running
    // total, so two separate single-badge wakes cannot produce this line — and
    // `dispatched` is what the ready queue actually handed out.
    write_pair(
        b"[wait-set:waiter] wake ready=",
        widest as u64,
        b" dispatched=",
        dispatched as u64,
    );

    // The timer is the third source, and it is armed only now — after the pair
    // above was dispatched — so it cannot join their coalesced word and the
    // block below has exactly one way to be woken. Nothing else can make it
    // ready, so this is the wait set blocking on *time* through the same
    // notification it blocks on messages, which is the half a message-only
    // sweep cannot do.
    if timer_arm(20_000_000).is_err() {
        fail(b"timer arm")
    }
    // Passes that dispatched something, counting the coalesced one above. A
    // pass, not a wake: `set.wakes()` counts every kernel answer including the
    // empty polls the loop above may have made, and the two numbers are printed
    // together precisely so the gate can see that the ready sets were fewer than
    // the sources.
    let mut waits = 1;
    let mut blocks = 0;
    loop {
        if blocks == 8 {
            fail(b"the timer source never became ready")
        }
        blocks += 1;
        let queued = set.wait().unwrap_or_else(|error| fail_with(b"wait", error));
        if queued == 0 {
            continue;
        }
        waits += 1;
        let mut kinds = 0;
        set.dispatch(|ready| {
            if ready.kind != SourceKind::Timer || ready.drain_slot.is_some() {
                fail(b"a non-timer source became ready after its peers exited")
            }
            kinds += 1;
        })
        .unwrap_or_else(|error| fail_with(b"dispatch timer", error));
        if kinds == 1 {
            write_pair(
                b"[wait-set:waiter] wake ready=",
                queued as u64,
                b" dispatched=",
                kinds as u64,
            );
            break;
        }
    }
    write_pair(
        b"[wait-set:waiter] sources stream=1 timer=1 supervision=1 waits=",
        waits as u64,
        b" wakes=",
        set.wakes() as u64,
    );
    // A retired source stops being waited on, and its badge leaves the mask.
    if !set.unregister(supervision.badge) {
        fail(b"unregister a live source")
    }
    if set.registered() != 2 || set.mask() & supervision.badge != 0 {
        fail(b"unregister left the source registered")
    }
    debug_write(b"[wait-set:waiter] retired supervision registered=2\n");
    exit(0)
}

/// Signal the waiter's declared badge on the shared Notification.
fn run_signaller() -> ! {
    let signal = resolve_binding(b"notification:wait-set-wake+signal")
        .unwrap_or_else(|_| fail(b"resolve wake signal slot"));
    // Signal first, then send. `send` is a rendezvous: it blocks until the peer
    // receives, and the peer is a wait set that will not drain a source it has
    // not been told is ready — so a message-then-signal order deadlocks the
    // pair. Signalling first is safe because the badge is level-triggered
    // readiness, not a message count: the waiter drains until the endpoint
    // would block, so an early signal costs one empty drain and never a lost
    // message. This ordering *is* the protocol rule the wait set documents.
    if slime_rt::notification_signal(signal) != ERR_SUCCESS {
        fail(b"signal the wake notification")
    }
    if send(0, b"wait-set", &[]) != ERR_SUCCESS {
        fail(b"send to the waiter")
    }
    debug_write(b"[wait-set:signaller] message=1 signal=1\n");
    exit(0)
}

/// A component the wait-set resource does not name registers nothing.
fn run_denied(mut set: WaitSet) -> ! {
    if !set.declarations().is_empty() {
        fail(b"an undeclared instance received sources")
    }
    // Every registration path refuses, including the two that name a source by
    // slot and by kind rather than by badge, so a component cannot reach an
    // undeclared source by asking a different question.
    if set.register(1).is_ok()
        || set.register_slot(SourceKind::Stream, 0).is_ok()
        || set.register_timer().is_ok()
    {
        fail(b"an undeclared instance registered a source")
    }
    // And a wake it never registered for queues nothing rather than dispatching.
    if set.queue(u64::MAX) != 0 {
        fail(b"an undeclared instance queued readiness")
    }
    debug_write(b"[wait-set:denied] declared=0 badge=-1 slot=-1 timer=-1 queued=0\n");
    exit(0)
}

fn write_pair(prefix: &[u8], first: u64, middle: &[u8], second: u64) {
    let mut first_digits = [0u8; 20];
    let mut second_digits = [0u8; 20];
    debug_write(prefix);
    debug_write(decimal(first, &mut first_digits));
    debug_write(middle);
    debug_write(decimal(second, &mut second_digits));
    debug_write(b"\n");
}

fn write_hex(prefix: &[u8], value: u64) {
    let mut digits = [0u8; 16];
    let mut index = digits.len();
    let mut remaining = value;
    if remaining == 0 {
        index -= 1;
        digits[index] = b'0';
    }
    while remaining != 0 {
        index -= 1;
        digits[index] = b"0123456789abcdef"[(remaining & 0xf) as usize];
        remaining >>= 4;
    }
    debug_write(prefix);
    debug_write(b"0x");
    debug_write(&digits[index..]);
    debug_write(b"\n");
}

fn decimal(value: u64, digits: &mut [u8; 20]) -> &[u8] {
    if value == 0 {
        digits[19] = b'0';
        return &digits[19..];
    }
    let mut remaining = value;
    let mut index = digits.len();
    while remaining != 0 {
        index -= 1;
        digits[index] = b'0' + (remaining % 10) as u8;
        remaining /= 10;
    }
    &digits[index..]
}

fn fail_with(reason: &[u8], error: WaitError) -> ! {
    debug_write(b"[wait-set] FAIL ");
    debug_write(reason);
    match error {
        WaitError::Registry(_) => debug_write(b" registry\n"),
        WaitError::Transport(_) => debug_write(b" transport\n"),
    };
    exit(1)
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[wait-set] FAIL ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
