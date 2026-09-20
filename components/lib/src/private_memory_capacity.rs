use core::fmt::{self, Write};
use slime_proto::private_memory_probe::{self as protocol, WireMessage};
use slime_rt::{CapabilityDisposition, PrivateMemory, Termination};

const PAGES: usize = 65_536;
const CYCLES: u32 = 20;
/// Every task's private window starts at the same virtual address, so an
/// attacker probing this far into the victim's declared extent names an address
/// its own VSpace also covers, while holding only one page there: nothing but
/// VSpace separation can refuse it.
const FOREIGN_OFFSET: u64 = (PAGES as u64 / 2) * 4096;
const EXECUTABLES: [&[u8]; 4] = [
    b"holder-a-executable",
    b"holder-b-executable",
    b"holder-c-executable",
    b"holder-d-executable",
];
const ENDPOINTS: [&[u8]; 4] = [
    b"holder-a-control",
    b"holder-b-control",
    b"holder-c-control",
    b"holder-d-control",
];

pub fn run(_: u32) {
    let stress = slime_rt::resolve_binding(b"private-stress-control").is_ok();
    if slime_rt::resolve_binding(EXECUTABLES[0]).is_ok() {
        if slime_rt::resolve_binding(b"private-stress-heap-executable").is_ok() {
            coordinate_heap_stress();
        }
        if stress {
            coordinate_stress();
        }
        if slime_rt::resolve_binding(EXECUTABLES[3]).is_err() {
            coordinate_isolation();
        }
        coordinate();
    }
    let endpoint = ENDPOINTS
        .iter()
        .find_map(|name| slime_rt::resolve_binding(name).ok())
        .unwrap_or_else(|| fail(b"missing control"));
    if stress {
        hold_stress(endpoint);
    }
    let initial = if let Ok(factory) = slime_rt::resolve_binding(b"attacker-b-factory") {
        Some(reciprocal_loans(factory, 1))
    } else if let Ok(factory) = slime_rt::resolve_binding(b"attacker-c-factory") {
        Some(reciprocal_loans(factory, 2))
    } else {
        None
    };
    hold(endpoint, initial)
}

fn fail(reason: &[u8]) -> ! {
    trace(format_args!(
        "[private-memory-1g] FAIL {}",
        core::str::from_utf8(reason).unwrap_or("invalid diagnostic")
    ));
    slime_rt::exit(1)
}

fn message(operation: u32, holder: u32, incarnation: u32) -> WireMessage {
    WireMessage {
        version: protocol::FORMAT_VERSION,
        operation,
        holder,
        incarnation,
        address: 0,
        reserved: [0; 40],
    }
}

fn decode(bytes: &[u8]) -> WireMessage {
    if bytes.len() != protocol::MESSAGE_LEN {
        fail(b"message length");
    }
    let value = WireMessage::decode(bytes).unwrap_or_else(|| fail(b"message decode"));
    if value.version != protocol::FORMAT_VERSION
        || value.holder >= 4
        || value.incarnation > CYCLES
        || value.reserved != [0; 40]
    {
        fail(b"message bounds");
    }
    value
}

fn command(endpoint: u32, operation: u32, holder: usize, incarnation: u32) -> u64 {
    command_at(endpoint, operation, holder, incarnation, 0)
}

fn command_at(endpoint: u32, operation: u32, holder: usize, incarnation: u32, address: u64) -> u64 {
    let mut response = [0u8; slime_rt::MAX_MSG];
    let mut request = message(operation, holder as u32, incarnation);
    request.address = address;
    let size = slime_rt::call(endpoint, &request.encode(), &mut response);
    if size != protocol::MESSAGE_LEN as i64 {
        fail(b"command reply");
    }
    let reply = decode(&response[..size as usize]);
    if reply.operation != protocol::ACKNOWLEDGE
        || reply.holder != holder as u32
        || reply.incarnation != incarnation
    {
        fail(b"reply identity");
    }
    reply.address
}

fn coordinate_isolation() -> ! {
    let executable = EXECUTABLES[..3].iter().map(|name| {
        slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"isolation executable"))
    });
    let mut executables = [0; 3];
    for (slot, value) in executables.iter_mut().zip(executable) {
        *slot = value;
    }
    let mut endpoints = [0; 3];
    for (slot, name) in endpoints.iter_mut().zip(ENDPOINTS.iter()) {
        *slot = slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"isolation endpoint"));
    }
    let victim = slime_rt::spawn(executables[0], &[]).unwrap_or_else(|_| fail(b"victim spawn"));
    let address = command(endpoints[0], protocol::GROW, 0, 0);
    if address == 0 {
        fail(b"victim address");
    }
    trace(format_args!(
        "[private-memory-isolation] victim base={} pages=65536",
        address
    ));
    // Both peers must finish lending and receiving before either faults; a
    // loan's receiver and its source owner must remain live through settlement.
    let attackers = [
        slime_rt::spawn(executables[1], &[]).unwrap_or_else(|_| fail(b"attacker-b spawn")),
        slime_rt::spawn(executables[2], &[]).unwrap_or_else(|_| fail(b"attacker-c spawn")),
    ];
    for (index, (holder, operation)) in [(1, protocol::READ_FOREIGN), (2, protocol::WRITE_FOREIGN)]
        .into_iter()
        .enumerate()
    {
        if command_at(endpoints[holder], operation, holder, 0, address) != address + FOREIGN_OFFSET
        {
            fail(b"attacker probe address");
        }
        await_end(attackers[index].supervision_slot, true);
        command(endpoints[0], protocol::VERIFY, 0, 0);
        trace(format_args!(
            "[private-memory-isolation] denied holder={} operation={} address={} victim_preserved=1",
            holder,
            operation,
            address + FOREIGN_OFFSET
        ));
    }
    command_at(endpoints[0], protocol::EXECUTE_PRIVATE, 0, 0, address);
    await_end(victim.supervision_slot, true);
    slime_rt::debug_write(
        b"[private-memory-isolation] complete read=1 write=1 execute=1 buffer_window_denied=4 loan_window_denied=4 unowned_denied=6 seal_denied=2 loan_transfer=2\n",
    );
    slime_rt::exit(0)
}

fn coordinate() -> ! {
    let executable = EXECUTABLES
        .map(|name| slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"executable")));
    let endpoint =
        ENDPOINTS.map(|name| slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"endpoint")));
    let mut handles = [0u32; 4];
    for holder in 0..4 {
        handles[holder] = slime_rt::spawn(executable[holder], &[])
            .unwrap_or_else(|_| fail(b"initial spawn"))
            .supervision_slot;
        command(endpoint[holder], protocol::GROW, holder, 0);
    }
    slime_rt::debug_write(b"[private-memory-1g] resident holders=4 pages=262144\n");
    for cycle in 0..CYCLES {
        for (holder, control) in endpoint.iter().enumerate() {
            command(
                *control,
                protocol::VERIFY,
                holder,
                if holder == 3 { cycle } else { 0 },
            );
        }
        let subject =
            slime_rt::supervision_derive(handles[3]).unwrap_or_else(|_| fail(b"restart subject"));
        command(endpoint[3], protocol::FAULT, 3, cycle);
        await_end(handles[3], true);
        let admission = slime_rt::lifecycle_restart_admit(subject)
            .unwrap_or_else(|_| fail(b"restart admission"));
        if admission.attempts_remaining != CYCLES - cycle - 1 {
            fail(b"restart ordinal");
        }
        if slime_rt::cap_drop(subject) != slime_rt::ERR_SUCCESS {
            fail(b"subject drop");
        }
        handles[3] = slime_rt::spawn(executable[3], &[])
            .unwrap_or_else(|_| fail(b"replacement spawn"))
            .supervision_slot;
        command(endpoint[3], protocol::GROW, 3, cycle + 1);
        for (holder, control) in endpoint.iter().enumerate() {
            command(
                *control,
                protocol::VERIFY,
                holder,
                if holder == 3 { cycle + 1 } else { 0 },
            );
        }
        trace(format_args!(
            "[private-memory-1g] retained cycle={} holders=4 pages=262144",
            cycle + 1
        ));
    }
    for holder in 0..4 {
        command(
            endpoint[holder],
            protocol::FINISH,
            holder,
            if holder == 3 { CYCLES } else { 0 },
        );
        await_end(handles[holder], false);
    }
    slime_rt::debug_write(b"[private-memory-1g] complete faults=20 replacements=20 exits=4\n");
    slime_rt::exit(0)
}

fn stress_peers() -> ([u32; 3], [u32; 3]) {
    let mut handles = [0; 3];
    let mut endpoints = [0; 3];
    for holder in 0..3 {
        let executable = slime_rt::resolve_binding(EXECUTABLES[holder])
            .unwrap_or_else(|_| fail(b"stress peer executable"));
        endpoints[holder] = slime_rt::resolve_binding(ENDPOINTS[holder])
            .unwrap_or_else(|_| fail(b"stress peer endpoint"));
        handles[holder] = slime_rt::spawn(executable, &[])
            .unwrap_or_else(|_| fail(b"stress peer spawn"))
            .supervision_slot;
        command(endpoints[holder], protocol::GROW, holder, 0);
    }
    (handles, endpoints)
}

fn verify_stress_peers(endpoints: &[u32; 3]) {
    for (holder, endpoint) in endpoints.iter().enumerate() {
        command(*endpoint, protocol::VERIFY, holder, 0);
    }
}

fn finish_stress_peers(handles: [u32; 3], endpoints: [u32; 3]) {
    for holder in 0..3 {
        command(endpoints[holder], protocol::FINISH, holder, 0);
        await_end(handles[holder], false);
    }
}

fn coordinate_heap_stress() -> ! {
    let (handles, endpoints) = stress_peers();
    let executable = slime_rt::resolve_binding(b"private-stress-heap-executable")
        .unwrap_or_else(|_| fail(b"stress heap executable"));
    let endpoint = slime_rt::resolve_binding(b"private-stress-heap-control")
        .unwrap_or_else(|_| fail(b"stress heap endpoint"));
    let heap = slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail(b"stress heap spawn"));
    verify_stress_peers(&endpoints);
    command(endpoint, protocol::GROW, 3, 0);
    verify_stress_peers(&endpoints);
    command(endpoint, protocol::VERIFY, 3, 0);
    verify_stress_peers(&endpoints);
    command(endpoint, protocol::FINISH, 3, 0);
    await_end(heap.supervision_slot, false);
    verify_stress_peers(&endpoints);
    finish_stress_peers(handles, endpoints);
    slime_rt::debug_write(b"[private-memory-stress] heap_complete peers=3 exits=4\n");
    slime_rt::exit(0)
}

fn coordinate_stress() -> ! {
    let (handles, endpoints) = stress_peers();
    let executable =
        slime_rt::resolve_binding(EXECUTABLES[3]).unwrap_or_else(|_| fail(b"stress executable"));
    let endpoint =
        slime_rt::resolve_binding(ENDPOINTS[3]).unwrap_or_else(|_| fail(b"stress endpoint"));
    for attempt in 1..=3 {
        if !matches!(
            slime_rt::spawn(executable, &[]),
            Err(slime_rt::ERR_OUT_OF_MEMORY)
        ) {
            fail(b"stress spawn refusal");
        }
        verify_stress_peers(&endpoints);
        trace(format_args!(
            "[private-memory-stress] spawn_refused attempt={} peers_preserved=3",
            attempt
        ));
    }
    for incarnation in 0..2 {
        let subject = slime_rt::spawn(executable, &[]).unwrap_or_else(|_| fail(b"stress spawn"));
        let mut stage = 0;
        let mut expected_pages = 0;
        for (operation, delta) in [
            (protocol::STRESS_GROW, 1),
            (protocol::STRESS_GROW, 511),
            (protocol::STRESS_REFUSE, 1024),
            (protocol::STRESS_GROW, 1024),
            (protocol::STRESS_GROW, 63_998),
            (protocol::STRESS_REFUSE, 2),
            (protocol::STRESS_GROW, 2),
        ] {
            if incarnation == 1 && operation == protocol::STRESS_REFUSE {
                continue;
            }
            if operation == protocol::STRESS_GROW {
                expected_pages += delta;
            }
            if command_at(endpoint, operation, 3, incarnation, delta) != expected_pages {
                fail(b"stress reply extent");
            }
            verify_stress_peers(&endpoints);
            trace(format_args!(
                "[private-memory-stress] retained incarnation={} stage={} peers=3 pages=196608",
                incarnation, stage
            ));
            stage += 1;
        }
        if command(endpoint, protocol::STRESS_VERIFY, 3, incarnation) != PAGES as u64 {
            fail(b"stress final reply extent");
        }
        let operation = if incarnation == 0 {
            protocol::FINISH
        } else {
            protocol::FAULT
        };
        command(endpoint, operation, 3, incarnation);
        await_end(subject.supervision_slot, incarnation == 1);
        verify_stress_peers(&endpoints);
    }
    finish_stress_peers(handles, endpoints);
    slime_rt::debug_write(
        b"[private-memory-stress] complete spawn_refused=3 growth_refused=2 retries=2 replacements=1 faults=1 exits=4\n",
    );
    slime_rt::exit(0)
}

fn hold_stress(endpoint: u32) -> ! {
    let initial = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"stress query"));
    if initial.base == 0 || initial.pages != 0 {
        fail(b"stress initial region");
    }
    let mut pages = 0usize;
    let mut incarnation = None;
    loop {
        let mut bytes = [0u8; slime_rt::MAX_MSG];
        let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
        if slime_rt::recv_blocking(endpoint, &mut bytes, &mut caps) != protocol::MESSAGE_LEN as i64
        {
            fail(b"stress receive");
        }
        let request = decode(&bytes[..protocol::MESSAGE_LEN]);
        if request.holder != 3
            || request.incarnation > 1
            || incarnation.is_some_and(|value| value != request.incarnation)
        {
            fail(b"stress identity");
        }
        incarnation = Some(request.incarnation);
        let previous = pages;
        sweep_range(initial, request, 0, pages, false);
        match request.operation {
            protocol::STRESS_GROW | protocol::STRESS_REFUSE => {
                let delta =
                    usize::try_from(request.address).unwrap_or_else(|_| fail(b"stress delta"));
                if delta == 0 || delta > PAGES - pages {
                    fail(b"stress growth bounds");
                }
                let result = slime_rt::private_memory_grow(delta);
                if request.operation == protocol::STRESS_REFUSE {
                    if result != Err(slime_rt::ERR_OUT_OF_MEMORY) {
                        fail(b"stress growth refusal");
                    }
                } else {
                    let before = result.unwrap_or_else(|_| fail(b"stress growth"));
                    if before.base != initial.base || before.pages != pages {
                        fail(b"stress growth origin");
                    }
                    pages += delta;
                    sweep_range(initial, request, previous, pages, true);
                }
            }
            protocol::STRESS_VERIFY | protocol::FINISH | protocol::FAULT => {
                if request.address != 0
                    || (request.operation != protocol::STRESS_VERIFY && pages != PAGES)
                {
                    fail(b"stress final extent");
                }
                if pages == PAGES
                    && slime_rt::private_memory_grow(1) != Err(slime_rt::ERR_OUT_OF_MEMORY)
                {
                    fail(b"stress ceiling");
                }
            }
            _ => fail(b"stress operation"),
        }
        let current = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"stress query"));
        if current.base != initial.base || current.pages != pages {
            fail(b"stress extent changed");
        }
        sweep_range(initial, request, 0, pages, false);
        if matches!(
            request.operation,
            protocol::STRESS_GROW | protocol::STRESS_REFUSE | protocol::STRESS_VERIFY
        ) {
            trace(format_args!(
                "[private-memory-stress] stage incarnation={} operation={} delta={} previous={} pages={} zeroed={} preserved=1",
                request.incarnation,
                request.operation,
                request.address,
                previous,
                pages,
                u8::from(request.operation == protocol::STRESS_GROW)
            ));
        }
        let mut response = message(protocol::ACKNOWLEDGE, 3, request.incarnation);
        response.address = pages as u64;
        if slime_rt::reply(&response.encode()) != slime_rt::ERR_SUCCESS {
            fail(b"stress reply");
        }
        if request.operation == protocol::FINISH {
            slime_rt::exit(0);
        }
        if request.operation == protocol::FAULT {
            access_fault(initial.base + PAGES * 4096, protocol::WRITE_FOREIGN);
            fail(b"stress guard did not fault");
        }
    }
}

fn await_end(handle: u32, fault: bool) {
    for _ in 0..65_536 {
        match slime_rt::supervision_status(handle) {
            Ok(None) => slime_rt::yield_now(),
            Ok(Some(Termination::Fault(_))) if fault => return,
            Ok(Some(Termination::Exit(0))) if !fault => return,
            _ => fail(b"termination"),
        }
    }
    fail(b"termination bound")
}

fn hold(endpoint: u32, initial: Option<PrivateMemory>) -> ! {
    let mut identity = None;
    let mut region = initial;
    let mut verify_round = 0u64;
    loop {
        let mut bytes = [0u8; slime_rt::MAX_MSG];
        let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
        let size = slime_rt::recv_blocking(endpoint, &mut bytes, &mut caps);
        if size != protocol::MESSAGE_LEN as i64 {
            fail(b"receive");
        }
        let request = decode(&bytes[..size as usize]);
        if matches!(
            request.operation,
            protocol::READ_FOREIGN | protocol::WRITE_FOREIGN
        ) {
            if identity.is_some() || request.address == 0 {
                fail(b"attacker identity");
            }
            // The attacker owns a real, working one-page private window at the
            // same virtual base the victim uses, so its later fault is the
            // victim's pages being unreachable rather than this task holding no
            // private memory or naming an address nothing ever maps.
            let own = match region {
                Some(memory) => memory,
                None => grow_attacker_page(request.address),
            };
            if own.base as u64 != request.address {
                fail(b"attacker window origin");
            }
            if slime_rt::private_memory_grow(1).is_ok() {
                fail(b"attacker exceeded its quota");
            }
            let probe = request.address + FOREIGN_OFFSET;
            share_paths_are_denied(request.holder, own.base as u64, probe);
            verify_attacker_page(own);
            trace(format_args!(
                "[private-memory-isolation] attacker holder={} operation={} address={} own_base={} own_pages=1 own_buffer=1 buffer_window_denied=2 unowned_denied=3 seal_denied=1 pages=0 buffers=0 mappings=0 loans=0",
                request.holder, request.operation, probe, own.base
            ));
            let mut reply = message(protocol::ACKNOWLEDGE, request.holder, request.incarnation);
            reply.address = probe;
            if slime_rt::reply(&reply.encode()) != slime_rt::ERR_SUCCESS {
                fail(b"attacker reply");
            }
            access_fault(probe as usize, request.operation);
            fail(b"foreign access succeeded");
        }
        if request.operation == protocol::GROW {
            if identity.is_some() {
                fail(b"duplicate grow");
            }
            let initial =
                slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"initial query"));
            if initial.pages != 0 || initial.base == 0 {
                fail(b"initial region");
            }
            let previous = slime_rt::private_memory_grow(PAGES).unwrap_or_else(|_| fail(b"grow"));
            if previous != initial {
                fail(b"growth origin");
            }
            sweep(initial, request, true);
            identity = Some((request.holder, request.incarnation));
            region = Some(initial);
            trace(format_args!(
                "[private-memory-1g] zeroed holder={} incarnation={} pages=65536",
                request.holder, request.incarnation
            ));
        } else {
            if identity != Some((request.holder, request.incarnation)) {
                fail(b"command identity");
            }
            let memory = region.unwrap_or_else(|| fail(b"missing region"));
            sweep(memory, request, false);
            match request.operation {
                protocol::VERIFY => {
                    if slime_rt::private_memory_grow(1).is_ok() {
                        fail(b"extra page admitted");
                    }
                    let current =
                        slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"query"));
                    if current.base != memory.base || current.pages != PAGES {
                        fail(b"refusal changed extent");
                    }
                    sweep(memory, request, false);
                    if request.holder == 0 {
                        check_shared(memory, verify_round);
                    }
                    trace(format_args!(
                        "[private-memory-1g] verified holder={} incarnation={} round={} pages=65536 refused=1 shared={}",
                        request.holder,
                        request.incarnation,
                        verify_round,
                        u8::from(request.holder == 0)
                    ));
                    verify_round += 1;
                }
                protocol::EXECUTE_PRIVATE => {
                    if request.address != memory.base as u64 {
                        fail(b"execute address");
                    }
                }
                protocol::FAULT | protocol::FINISH => (),
                _ => fail(b"operation"),
            }
        }
        let mut reply = message(protocol::ACKNOWLEDGE, request.holder, request.incarnation);
        reply.address = region.map_or(0, |memory| memory.base as u64);
        if slime_rt::reply(&reply.encode()) != slime_rt::ERR_SUCCESS {
            fail(b"reply");
        }
        if request.operation == protocol::FINISH {
            slime_rt::exit(0);
        }
        if request.operation == protocol::EXECUTE_PRIVATE {
            trace(format_args!(
                "[private-memory-isolation] execute address={} pages=65536",
                request.address
            ));
            access_fault(request.address as usize, protocol::EXECUTE_PRIVATE);
            fail(b"private execute returned");
        }
        if request.operation == protocol::FAULT {
            #[cfg(any(target_arch = "aarch64", target_arch = "riscv64"))]
            let address = region.unwrap().base + PAGES * 4096;
            // SAFETY: an explicit store instruction to the unmapped guard deliberately faults.
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!("str xzr, [{address}]", address = in(reg) address, options(nostack));
            }
            // SAFETY: same guard fault on RV64; no Rust pointer dereference is involved.
            #[cfg(target_arch = "riscv64")]
            unsafe {
                core::arch::asm!("sd zero, 0({address})", address = in(reg) address, options(nostack));
            }
            fail(b"guard did not fault");
        }
    }
}

const SHARE_SCRATCH: u64 = 0x0000_000b_0000_0000;
const LOAN_SCRATCH: u64 = SHARE_SCRATCH + 0x20_0000;
const LOAN_PEER: &[u8] = b"isolation-loan-peer";

fn grow_attacker_page(expected_base: u64) -> PrivateMemory {
    let own = slime_rt::private_memory_grow(1).unwrap_or_else(|_| fail(b"attacker grow"));
    if own.base == 0 || own.pages != 0 || own.base as u64 != expected_base {
        fail(b"attacker window origin");
    }
    // SAFETY: the admitted page is this task's own mapped private page.
    unsafe {
        (own.base as *mut u64).write_volatile(0xa77a_c6e4);
        if (own.base as *const u64).read_volatile() != 0xa77a_c6e4 {
            fail(b"attacker own page");
        }
    }
    own
}

fn verify_attacker_page(own: PrivateMemory) {
    let current = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"attacker query"));
    if current.base != own.base || current.pages != 1 {
        fail(b"attacker extent changed");
    }
    // SAFETY: the task retains its successfully grown private page throughout the exchange.
    if unsafe { (own.base as *const u64).read_volatile() } != 0xa77a_c6e4 {
        fail(b"attacker private payload changed");
    }
}

fn require_zero_buffers() {
    let usage = slime_rt::shared_buffer_occupancy().unwrap_or_else(|_| fail(b"buffer occupancy"));
    if usage.pages != 0 || usage.buffers != 0 || usage.mappings != 0 || usage.loans != 0 {
        fail(b"buffer cleanup");
    }
}

fn loan_message(operation: u32, source: u32, receiver: u32) -> WireMessage {
    message(operation, source, receiver)
}

fn receive_loan_message(peer: u32, operation: u32, source: u32, receiver: u32) {
    let mut bytes = [0u8; slime_rt::MAX_MSG];
    let mut caps = [0u64; slime_rt::MAX_CAPS_PER_MSG];
    let size = slime_rt::recv_blocking(peer, &mut bytes, &mut caps);
    if size != protocol::MESSAGE_LEN as i64 {
        fail(b"loan message length");
    }
    let value = decode(&bytes[..protocol::MESSAGE_LEN]);
    if value.operation != operation
        || value.holder != source
        || value.incarnation != receiver
        || value.address != 0
    {
        fail(b"loan message identity");
    }
}

fn send_loan_message(peer: u32, operation: u32, source: u32, receiver: u32) {
    if slime_rt::send(
        peer,
        &loan_message(operation, source, receiver).encode(),
        &[],
    ) != slime_rt::ERR_SUCCESS
    {
        fail(b"loan message send");
    }
}

fn reciprocal_loans(factory: u32, holder: u32) -> PrivateMemory {
    let peer = slime_rt::resolve_binding(LOAN_PEER).unwrap_or_else(|_| fail(b"loan peer"));
    let initial = slime_rt::private_memory_grow(0).unwrap_or_else(|_| fail(b"attacker query"));
    if initial.base == 0 || initial.pages != 0 {
        fail(b"attacker initial region");
    }
    let own = grow_attacker_page(initial.base as u64);
    for (source, receiver) in [(1, 2), (2, 1)] {
        if holder == source {
            loan_lender_prelude(factory, peer, source, receiver);
        } else {
            loan_receiver_prelude(peer, own, source, receiver);
        }
        verify_attacker_page(own);
    }
    require_zero_buffers();
    own
}

fn loan_lender_prelude(factory: u32, peer: u32, source: u32, receiver: u32) {
    receive_loan_message(peer, protocol::LOAN_READY, source, receiver);
    let buffer =
        slime_rt::shared_buffer_create(factory, 1, true).unwrap_or_else(|_| fail(b"loan source"));
    if slime_rt::shared_buffer_map(buffer.slot, SHARE_SCRATCH, 0, 4096, true)
        != slime_rt::ERR_SUCCESS
    {
        fail(b"loan source map");
    }
    // SAFETY: the root mapped this page writable above.
    unsafe {
        (SHARE_SCRATCH as *mut u64).write_volatile(0x10a0_5ea1 + u64::from(source));
        if (SHARE_SCRATCH as *const u64).read_volatile() != 0x10a0_5ea1 + u64::from(source) {
            fail(b"loan source payload");
        }
    }
    if slime_rt::shared_buffer_unmap(buffer.slot, SHARE_SCRATCH) != slime_rt::ERR_SUCCESS
        || slime_rt::shared_buffer_seal(buffer.slot) != slime_rt::ERR_SUCCESS
    {
        fail(b"loan source finalize");
    }
    let loan = slime_rt::shared_buffer_loan(buffer.slot, peer, 0, 4096, false)
        .unwrap_or_else(|_| fail(b"loan creation"));
    let descriptor = loan_message(protocol::LOAN_OFFER, source, receiver).encode();
    if slime_rt::capability_delegate(
        peer,
        loan.slot,
        CapabilityDisposition::Move,
        slime_proto::capability_transfer::OBJECT_KIND_SHARED_BUFFER_LOAN,
        boot_contracts::generation::RIGHT_BUFFER_MAP,
        &descriptor,
    ) != slime_rt::ERR_SUCCESS
    {
        fail(b"loan transfer");
    }
    receive_loan_message(peer, protocol::LOAN_SETTLED, source, receiver);
    if slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_SUCCESS {
        fail(b"loan source release");
    }
    require_zero_buffers();
    trace(format_args!(
        "[private-memory-isolation] loan lender holder={} receiver={} transferred=1 released=1 pages=0 buffers=0 mappings=0 loans=0",
        source, receiver
    ));
    send_loan_message(peer, protocol::ACKNOWLEDGE, source, receiver);
}

fn loan_receiver_prelude(peer: u32, own: PrivateMemory, source: u32, receiver: u32) {
    send_loan_message(peer, protocol::LOAN_READY, source, receiver);
    receive_loan_message(peer, protocol::LOAN_OFFER, source, receiver);
    let loan = slime_rt::capability_import().unwrap_or_else(|_| fail(b"loan import"));
    let probe = own.base as u64 + FOREIGN_OFFSET;
    for target in [own.base as u64, probe] {
        if slime_rt::shared_buffer_loan_map(loan, target, 0, 4096) != slime_rt::ERR_INVALID_ARG {
            fail(b"loan mapped inside private window");
        }
    }
    if slime_rt::shared_buffer_loan_map(loan, LOAN_SCRATCH, 0, 4096) != slime_rt::ERR_SUCCESS {
        fail(b"loan positive map");
    }
    // SAFETY: the received sealed loan is mapped read-only at this word.
    if unsafe { (LOAN_SCRATCH as *const u64).read_volatile() } != 0x10a0_5ea1 + u64::from(source) {
        fail(b"loan payload");
    }
    if slime_rt::shared_buffer_unmap(loan, LOAN_SCRATCH) != slime_rt::ERR_SUCCESS {
        fail(b"loan unmap");
    }
    if slime_rt::shared_buffer_return(loan) != slime_rt::ERR_SUCCESS
        || slime_rt::shared_buffer_return(loan) != slime_rt::ERR_BAD_CAP
    {
        fail(b"loan single return");
    }
    require_zero_buffers();
    trace(format_args!(
        "[private-memory-isolation] loan receiver source={} holder={} positive=1 window_denied=2 unmapped=1 returned=1 return_denied=1 pages=0 buffers=0 mappings=0 loans=0",
        source, receiver
    ));
    send_loan_message(peer, protocol::LOAN_SETTLED, source, receiver);
    receive_loan_message(peer, protocol::ACKNOWLEDGE, source, receiver);
}

/// Exercise every shared-buffer operation that could alias bytes into this
/// task's private window, plus unowned-handle and sealed-write refusals.
fn share_paths_are_denied(holder: u32, own_base: u64, probe: u64) {
    // Grant names are image-wide, so each peer resolves its own factory.
    let name: &[u8] = match holder {
        1 => b"attacker-b-factory",
        2 => b"attacker-c-factory",
        _ => fail(b"attacker role"),
    };
    let factory = slime_rt::resolve_binding(name).unwrap_or_else(|_| fail(b"attacker factory"));
    let buffer = slime_rt::shared_buffer_create(factory, 1, true)
        .unwrap_or_else(|_| fail(b"attacker buffer"));
    // The buffer is real: it maps, carries bytes, and unmaps outside every
    // private window, so destination refusals cannot be caused by an unusable
    // handle.
    if slime_rt::shared_buffer_map(buffer.slot, SHARE_SCRATCH, 0, 4096, true)
        != slime_rt::ERR_SUCCESS
    {
        fail(b"attacker buffer unusable");
    }
    // SAFETY: the successful mapping covers this aligned word.
    unsafe {
        (SHARE_SCRATCH as *mut u64).write_volatile(0x5ade_5ade);
        if (SHARE_SCRATCH as *const u64).read_volatile() != 0x5ade_5ade {
            fail(b"attacker buffer payload");
        }
    }
    if slime_rt::shared_buffer_unmap(buffer.slot, SHARE_SCRATCH) != slime_rt::ERR_SUCCESS {
        fail(b"attacker buffer unmap");
    }
    // Aliasing shared pages over any private window, backed or not, in this
    // task's own address space.
    if slime_rt::shared_buffer_map(buffer.slot, own_base, 0, 4096, true)
        != slime_rt::ERR_INVALID_ARG
        || slime_rt::shared_buffer_map(buffer.slot, probe, 0, 4096, true)
            != slime_rt::ERR_INVALID_ARG
    {
        fail(b"shared mapping admitted inside a private window");
    }
    // A slot this task never received cannot borrow another holder's buffer
    // authority. Probe outside the private window so capability lookup, not
    // destination validation, owns the result.
    let unowned = buffer
        .slot
        .checked_add(1)
        .unwrap_or_else(|| fail(b"unowned slot"));
    if slime_rt::shared_buffer_map(unowned, SHARE_SCRATCH, 0, 4096, true) != slime_rt::ERR_BAD_CAP
        || slime_rt::shared_buffer_seal(unowned) != slime_rt::ERR_BAD_CAP
        || slime_rt::shared_buffer_release(unowned) != slime_rt::ERR_BAD_CAP
    {
        fail(b"unowned slot admitted");
    }
    if slime_rt::shared_buffer_seal(buffer.slot) != slime_rt::ERR_SUCCESS
        || slime_rt::shared_buffer_map(buffer.slot, SHARE_SCRATCH, 0, 4096, true)
            != slime_rt::ERR_BAD_CAP
    {
        fail(b"sealed buffer writable");
    }
    if slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_SUCCESS {
        fail(b"attacker buffer release");
    }
    require_zero_buffers();
}

fn access_fault(address: usize, operation: u32) {
    #[cfg(target_arch = "aarch64")]
    // SAFETY: explicit access/branch instructions deliberately test VSpace
    // permission denial; no invalid Rust pointer is dereferenced.
    unsafe {
        match operation {
            protocol::READ_FOREIGN => {
                core::arch::asm!("ldr xzr, [{a}]", a = in(reg) address, options(nostack))
            }
            protocol::WRITE_FOREIGN => {
                core::arch::asm!("str xzr, [{a}]", a = in(reg) address, options(nostack))
            }
            protocol::EXECUTE_PRIVATE => {
                core::arch::asm!("br {a}", a = in(reg) address, options(nostack))
            }
            _ => fail(b"fault operation"),
        }
    }
    #[cfg(target_arch = "riscv64")]
    // SAFETY: the same VSpace permission tests, as explicit RV64 instructions.
    unsafe {
        match operation {
            protocol::READ_FOREIGN => {
                core::arch::asm!("ld zero, 0({a})", a = in(reg) address, options(nostack))
            }
            protocol::WRITE_FOREIGN => {
                core::arch::asm!("sd zero, 0({a})", a = in(reg) address, options(nostack))
            }
            protocol::EXECUTE_PRIVATE => {
                core::arch::asm!("jr {a}", a = in(reg) address, options(nostack))
            }
            _ => fail(b"fault operation"),
        }
    }
    #[cfg(not(any(target_arch = "aarch64", target_arch = "riscv64")))]
    let _ = (address, operation);
}

fn sweep(region: PrivateMemory, request: WireMessage, initialize: bool) {
    sweep_range(region, request, 0, PAGES, initialize);
}

fn sweep_range(
    region: PrivateMemory,
    request: WireMessage,
    start: usize,
    end: usize,
    initialize: bool,
) {
    let start_word = start * 4096 / 8;
    let end_word = end * 4096 / 8;
    let base = region.base as *mut u64;
    let stamp = 0x4d45_4d00_0000_0000u64
        | ((request.holder as u64 + 1) << 40)
        | ((request.incarnation as u64 + 1) << 32);
    if initialize {
        for word in start_word..end_word {
            // SAFETY: successful public growth mapped the complete selected range.
            if unsafe { base.add(word).read_volatile() } != 0 {
                fail(b"stale word");
            }
        }
        for word in start_word..end_word {
            // SAFETY: the same admitted region is writable and wholly within quota.
            unsafe {
                base.add(word).write_volatile(stamp ^ word as u64);
            }
        }
    }
    for word in start_word..end_word {
        // SAFETY: the holder retains its mapped region until termination.
        if unsafe { base.add(word).read_volatile() } != stamp ^ word as u64 {
            fail(b"pattern mismatch");
        }
    }
}

fn check_shared(region: PrivateMemory, round: u64) {
    // Different leaf-table ranges expose lifetime-dependent table exhaustion.
    let base = 0x0000_000a_0000_0000 + round * 2 * 1024 * 1024;
    let factory = slime_rt::resolve_binding(b"capacity-shared-factory")
        .unwrap_or_else(|_| fail(b"shared factory"));
    let buffer =
        slime_rt::shared_buffer_create(factory, 1, true).unwrap_or_else(|_| fail(b"shared create"));
    if slime_rt::shared_buffer_create(factory, 1, true).is_ok() {
        fail(b"shared quota");
    }
    if slime_rt::shared_buffer_map(buffer.slot, region.base as u64, 0, 4096, true)
        == slime_rt::ERR_SUCCESS
    {
        fail(b"private collision");
    }
    if slime_rt::shared_buffer_map(buffer.slot, base, 0, 4096, true) != slime_rt::ERR_SUCCESS {
        fail(b"shared map");
    }
    for word in 0..512 {
        // SAFETY: the successful mapping covers this page and its aligned words.
        if unsafe { (base as *const u64).add(word).read_volatile() } != 0 {
            fail(b"shared stale bytes");
        }
    }
    // SAFETY: the successful shared-buffer mapping covers this aligned word.
    unsafe {
        (base as *mut u64).write_volatile(0x1234_5678);
    }
    if slime_rt::shared_buffer_seal(buffer.slot) != slime_rt::ERR_SUCCESS {
        fail(b"shared seal");
    }
    // SAFETY: sealing preserves read access to the existing mapping.
    if unsafe { (base as *const u64).read_volatile() } != 0x1234_5678 {
        fail(b"shared bytes");
    }
    if slime_rt::shared_buffer_unmap(buffer.slot, base) != slime_rt::ERR_SUCCESS
        || slime_rt::shared_buffer_release(buffer.slot) != slime_rt::ERR_SUCCESS
    {
        fail(b"shared release");
    }
}

/// One bounded console request per marker prevents interleaving fragments
/// with root timer and reclamation diagnostics.
fn trace(arguments: fmt::Arguments<'_>) {
    struct Line {
        bytes: [u8; 256],
        used: usize,
    }
    impl Write for Line {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let end = self
                .used
                .checked_add(text.len())
                .filter(|end| *end < self.bytes.len())
                .ok_or(fmt::Error)?;
            self.bytes[self.used..end].copy_from_slice(text.as_bytes());
            self.used = end;
            Ok(())
        }
    }
    let mut line = Line {
        bytes: [0; 256],
        used: 0,
    };
    if line.write_fmt(arguments).is_err() {
        slime_rt::debug_write(b"[private-memory-1g] FAIL marker bounds\n");
        slime_rt::exit(1);
    }
    line.bytes[line.used] = b'\n';
    slime_rt::debug_write(&line.bytes[..line.used + 1]);
}
