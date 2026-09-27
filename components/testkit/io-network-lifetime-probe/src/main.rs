#![no_std]
#![no_main]

use boot_contracts::generation::RIGHT_SUPERVISE;
use slime_components::network_io::{Connection, NetworkError, NetworkIo, NetworkNotifications};
use slime_proto::network_service as net;
use slime_rt::{PARAMETER_SELF_SLOT, SpawnGrant, Termination, debug_write, exit, resolve_binding};

slime_rt::entry!(main);

const ADDRESS: [u8; 4] = [127, 0, 0, 1];
const PORT: u16 = 7447;
const PHASE_KEY: u64 = 2;
const RING_BASE: u64 = 0x0000_001d_0000_0000;
const DATA_BASE: u64 = RING_BASE + 4096;

fn main(_: u32) {
    let rate = slime_rt::monotonic_frequency().unwrap_or_else(|_| fail(b"clock rate"));
    if let Ok(executable) = resolve_binding(b"reset-driver-executable") {
        supervise_driver_reset(executable, rate);
    }
    if let Ok(executable) = resolve_binding(b"lifetime-victim-executable") {
        supervise(executable, rate);
    }
    client(rate)
}

fn previous_connection(io: &mut NetworkIo<'_>, phase: u64) -> Option<u64> {
    if phase == 0 {
        return None;
    }
    let low = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 3)
        .unwrap_or_else(|_| fail(b"old identity low"));
    let high = slime_rt::lifecycle_parameter_read(slime_rt::PARAMETER_SELF_SLOT, 4)
        .unwrap_or_else(|_| fail(b"old identity high"));
    if low > u32::MAX as u64 || high > u32::MAX as u64 {
        fail(b"old identity bounds");
    }
    let old = low | (high << 32);
    if old == 0 {
        fail(b"old identity absent");
    }
    let request = net::WireNetworkRequest {
        magic: net::NETWORK_MAGIC,
        version: net::FORMAT_VERSION,
        op: net::OP_SEND,
        transport: net::TRANSPORT_NONE,
        flags: 0,
        port: 0,
        name_len: 0,
        capability: old,
        address_kind: net::ADDRESS_NONE,
        reserved: [0; 7],
        endpoint: [0; 24],
    };
    let reply = io
        .transact_raw(request, slime_proto::io_queue::DIRECTION_DEVICE_READ, 1)
        .unwrap_or_else(|_| fail(b"old handle request"));
    if reply.status_detail != net::STATUS_DENIED
        || reply.transferred != 0
        || reply.capability_kind != net::CAPABILITY_NONE
        || reply.capability != 0
    {
        fail(b"old handle accepted");
    }
    Some(old)
}

fn remember_connection(connection: &Connection, old: Option<u64>, role: &[u8]) {
    if let Some(old) = old {
        if connection.id() == old {
            fail(b"connection identity reused");
        }
        debug_write(b"[io-network-lifetime-probe] role=");
        debug_write(role);
        debug_write(b" old_handle_refused=1 fresh_identity=1\n");
    } else {
        for (key, value) in [
            (3, connection.id() & u32::MAX as u64),
            (4, connection.id() >> 32),
        ] {
            slime_rt::lifecycle_parameter_write(slime_rt::PARAMETER_SELF_SLOT, key, value)
                .unwrap_or_else(|_| fail(b"remember identity"));
        }
    }
}

fn binding(name: &[u8]) -> u32 {
    resolve_binding(name).unwrap_or_else(|_| fail(b"binding"))
}

fn now() -> u64 {
    slime_rt::monotonic_read().unwrap_or_else(|_| fail(b"clock read"))
}

fn deadline(rate: u64, seconds: u64) -> u64 {
    now()
        .checked_add(
            rate.checked_mul(seconds)
                .unwrap_or_else(|| fail(b"clock range")),
        )
        .unwrap_or_else(|| fail(b"deadline range"))
}

fn sleep(rate: u64, until: u64, wait_slot: u32) {
    let current = now();
    if current >= until {
        fail(b"deadline");
    }
    let delay = (rate / 100).max(1).min(until - current);
    let timer = slime_rt::timer_arm(delay).unwrap_or_else(|_| fail(b"timer arm"));
    slime_rt::notification_wait(wait_slot).unwrap_or_else(|_| fail(b"timer wait"));
    let _ = slime_rt::timer_cancel(timer);
}

fn supervise_driver_reset(driver_executable: u32, rate: u64) -> ! {
    let client_executable = binding(b"reset-client-executable");
    let service_executable = binding(b"reset-service-executable");
    let wait_slot = binding(b"notification:reset-supervisor-wake+wait");
    for round in 0..2 {
        let until = deadline(rate, 20);
        let driver = slime_rt::spawn(driver_executable, &[])
            .unwrap_or_else(|_| fail(b"reset driver spawn"))
            .supervision_slot;
        let client = slime_rt::spawn(client_executable, &[])
            .unwrap_or_else(|_| fail(b"reset client spawn"))
            .supervision_slot;
        let client_watch =
            slime_rt::supervision_derive(client).unwrap_or_else(|_| fail(b"reset client watch"));
        let service = slime_rt::spawn(
            service_executable,
            &[SpawnGrant {
                slot: client_watch,
                rights: RIGHT_SUPERVISE,
            }],
        )
        .unwrap_or_else(|_| fail(b"reset service spawn"))
        .supervision_slot;
        let _ = slime_rt::cap_drop(client_watch);
        let handles = [driver, client, service];
        let subjects = if round == 0 {
            handles.map(|slot| {
                slime_rt::supervision_derive(slot)
                    .unwrap_or_else(|_| fail(b"reset restart subject"))
            })
        } else {
            [0; 3]
        };
        for handle in handles {
            if collect(handle, rate, until, wait_slot) != Termination::Exit(0) {
                fail(b"reset child termination");
            }
        }
        if round == 0 {
            debug_write(b"[io-network-lifetime-probe] driver_reset round=1 exits=3\n");
            let mut ready_at = now();
            for subject in subjects {
                let admission = slime_rt::lifecycle_restart_admit(subject)
                    .unwrap_or_else(|_| fail(b"reset restart admit"));
                ready_at = ready_at.max(admission.ready_at);
                let _ = slime_rt::cap_drop(subject);
            }
            while now() < ready_at {
                sleep(rate, until, wait_slot);
            }
        } else {
            debug_write(b"[io-network-lifetime-probe] driver_reset round=2 exits=3\n");
        }
    }
    debug_write(b"[io-network-lifetime-probe] driver_reset=1 restarted=1 rounds=2\n");
    exit(0)
}

fn supervise(victim_executable: u32, rate: u64) -> ! {
    let survivor_executable = binding(b"lifetime-survivor-executable");
    let service_executable = binding(b"lifetime-service-executable");
    let wait_slot = binding(b"notification:lifetime-supervisor-wake+wait");
    let service_fault = resolve_binding(b"service-fault-victim").is_ok();
    if service_fault != resolve_binding(b"service-fault-survivor").is_ok() {
        fail(b"partial service fault mode");
    }
    for round in 0..2 {
        let until = deadline(rate, 20);
        let victim = slime_rt::spawn(victim_executable, &[])
            .unwrap_or_else(|_| fail(b"victim spawn"))
            .supervision_slot;
        let survivor = slime_rt::spawn(survivor_executable, &[])
            .unwrap_or_else(|_| fail(b"survivor spawn"))
            .supervision_slot;
        let victim_watch =
            slime_rt::supervision_derive(victim).unwrap_or_else(|_| fail(b"victim watch"));
        let survivor_watch =
            slime_rt::supervision_derive(survivor).unwrap_or_else(|_| fail(b"survivor watch"));
        let service = slime_rt::spawn(
            service_executable,
            &[
                SpawnGrant {
                    slot: victim_watch,
                    rights: RIGHT_SUPERVISE,
                },
                SpawnGrant {
                    slot: survivor_watch,
                    rights: RIGHT_SUPERVISE,
                },
            ],
        )
        .unwrap_or_else(|_| fail(b"service spawn"))
        .supervision_slot;
        let _ = slime_rt::cap_drop(victim_watch);
        let _ = slime_rt::cap_drop(survivor_watch);
        let subjects = if round == 0 {
            [victim, survivor, service].map(|slot| {
                slime_rt::supervision_derive(slot).unwrap_or_else(|_| fail(b"restart subject"))
            })
        } else {
            [0; 3]
        };
        let victim_outcome = collect(victim, rate, until, wait_slot);
        let survivor_outcome = collect(survivor, rate, until, wait_slot);
        let service_outcome = collect(service, rate, until, wait_slot);
        if round == 0 && service_fault {
            if !matches!(service_outcome, Termination::Fault(_))
                || !matches!(victim_outcome, Termination::Fault(_) | Termination::Exit(1))
                || !matches!(
                    survivor_outcome,
                    Termination::Fault(_) | Termination::Exit(1)
                )
            {
                fail(b"service fault invalidation outcomes");
            }
            debug_write(
                b"[io-network-lifetime-probe] round=1 service_fault=1 clients_invalidated=2\n",
            );
        } else if survivor_outcome != Termination::Exit(0)
            || service_outcome != Termination::Exit(0)
            || !(if round == 0 {
                matches!(victim_outcome, Termination::Fault(_))
            } else {
                victim_outcome == Termination::Exit(0)
            })
        {
            fail(b"round termination");
        }
        if round == 0 {
            if !service_fault {
                debug_write(b"[io-network-lifetime-probe] round=1 victim_fault=1 survivor_exit=1 service_exit=1\n");
            }
            let mut ready_at = now();
            for subject in subjects {
                let admission = slime_rt::lifecycle_restart_admit(subject)
                    .unwrap_or_else(|_| fail(b"restart admit"));
                ready_at = ready_at.max(admission.ready_at);
                let _ = slime_rt::cap_drop(subject);
            }
            while now() < ready_at {
                sleep(rate, until, wait_slot);
            }
        } else {
            debug_write(b"[io-network-lifetime-probe] round=2 victim_exit=1 survivor_exit=1 service_exit=1\n");
        }
    }
    if service_fault {
        debug_write(
            b"[io-network-lifetime-probe] service_fault=1 clients_invalidated=2 restarted=1\n",
        );
    } else {
        debug_write(b"[io-network-lifetime-probe] rounds=2 reclaimed=1 restarted=1\n");
    }
    exit(0)
}

fn collect(handle: u32, rate: u64, until: u64, wait_slot: u32) -> Termination {
    loop {
        match slime_rt::supervision_status(handle) {
            Ok(Some(outcome)) => return outcome,
            Ok(None) => sleep(rate, until, wait_slot),
            Err(_) => fail(b"supervision status"),
        }
    }
}

fn client(rate: u64) -> ! {
    let service_fault = resolve_binding(b"service-fault-victim").is_ok();
    let control = binding(b"lifetime-victim-control");
    let provision = binding(b"lifetime-victim-provision");
    let completion = binding(b"notification:lifetime-victim-done+wait");
    let phase = match slime_rt::lifecycle_parameter_read(PARAMETER_SELF_SLOT, PHASE_KEY) {
        Ok(value) => value,
        Err(slime_rt::ERR_INVALID_ARG) => 0,
        Err(_) => fail(b"phase read"),
    };
    if phase > 1 {
        fail(b"phase range");
    }
    let until = deadline(rate, 10);
    let notifications = NetworkNotifications {
        request_signal: binding(b"notification:lifetime-network-request+signal"),
        completion_wait: completion,
        timeout_ticks: rate.checked_mul(5).unwrap_or_else(|| fail(b"timer range")),
    };
    // SAFETY: these disjoint page-aligned mappings belong exclusively to this
    // adapter, and both declared endpoints terminate at the network service.
    let mut io = unsafe {
        NetworkIo::attach_with_notifications(
            control,
            provision,
            RING_BASE,
            DATA_BASE,
            notifications,
        )
    }
    .unwrap_or_else(|_| fail(b"attach"));
    if service_fault && phase == 0 {
        slime_rt::lifecycle_parameter_write(PARAMETER_SELF_SLOT, PHASE_KEY, 1)
            .unwrap_or_else(|_| fail(b"service fault phase write"));
    }
    victim_round(&mut io, phase, until, service_fault);
    slime_rt::lifecycle_parameter_write(PARAMETER_SELF_SLOT, PHASE_KEY, 1)
        .unwrap_or_else(|_| fail(b"phase write"));
    io.finish().unwrap_or_else(|_| fail(b"finish"));
    debug_write(b"[io-network-lifetime-probe] role=victim clean=1 loans_returned=2\n");
    exit(0)
}

fn victim_round(io: &mut NetworkIo<'_>, phase: u64, until: u64, service_fault: bool) {
    let previous = previous_connection(io, phase);
    let mut attempts = 0;
    let connection = loop {
        attempts += 1;
        let mut reply = io
            .connect_ipv4(ADDRESS, PORT)
            .unwrap_or_else(|_| fail(b"connect request"));
        if let Some(connection) = reply.take_connection() {
            break connection;
        }
        if !matches!(
            reply.status_detail,
            net::STATUS_REFUSED | net::STATUS_WOULD_BLOCK
        ) || attempts >= 64
        {
            fail(b"connect");
        }
        check_deadline(until);
        slime_rt::yield_now();
    };
    remember_connection(&connection, previous, b"victim");
    let mut bytes = [0u8; 1024];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index * 29 + 7) as u8;
    }
    send_all(io, &connection, &bytes, until);
    let mut answer = [0u8; 16];
    recv_all(
        io,
        &connection,
        &mut answer,
        until,
        service_fault && phase == 0,
    );
    if answer != [0x5a; 16] {
        fail(b"ack identity");
    }
    if phase == 0 {
        if service_fault {
            fail(b"service fault exchange unexpectedly completed");
        }
        slime_rt::lifecycle_parameter_write(PARAMETER_SELF_SLOT, PHASE_KEY, 1)
            .unwrap_or_else(|_| fail(b"fault phase write"));
        debug_write(
            b"[io-network-lifetime-probe] role=victim sent=1024 acknowledged=16 faulting=1\n",
        );
        // SAFETY: address zero is unmapped in this component. The explicit
        // machine instruction raises a kernel fault without a Rust dereference.
        #[cfg(target_arch = "aarch64")]
        unsafe {
            core::arch::asm!("str xzr, [{address}]", address = in(reg) 0usize, options(nostack));
        }
        #[cfg(target_arch = "x86_64")]
        unsafe {
            core::arch::asm!("mov qword ptr [{address}], 0", address = in(reg) 0usize, options(nostack));
        }
        #[cfg(target_arch = "riscv64")]
        unsafe {
            core::arch::asm!("sd zero, 0({address})", address = in(reg) 0usize, options(nostack));
        }
        fail(b"fault returned");
    }
    if !io
        .close(connection)
        .unwrap_or_else(|_| fail(b"victim close request"))
        .is_success()
    {
        fail(b"victim close");
    }
    debug_write(b"[io-network-lifetime-probe] role=victim sent=1024 acknowledged=16 fresh=1\n");
}

fn send_all(io: &mut NetworkIo<'_>, connection: &Connection, bytes: &[u8], until: u64) {
    let mut sent = 0;
    while sent < bytes.len() {
        let reply = io
            .send(connection, &bytes[sent..])
            .unwrap_or_else(|_| fail(b"send request"));
        if reply.is_success() && reply.transferred > 0 {
            sent += reply.transferred as usize;
        } else if reply.status_detail != net::STATUS_WOULD_BLOCK {
            fail(b"send");
        }
        check_deadline(until);
    }
}

fn recv_all(
    io: &mut NetworkIo<'_>,
    connection: &Connection,
    bytes: &mut [u8],
    until: u64,
    expect_service_fault: bool,
) {
    let mut read = 0;
    while read < bytes.len() {
        let reply = match io.recv(connection, &mut bytes[read..]) {
            Ok(reply) => reply,
            Err(NetworkError::Lost | NetworkError::Malformed) if expect_service_fault => {
                debug_write(b"[io-network-lifetime-probe] role=victim service_invalidated=1\n");
                exit(1)
            }
            Err(_) => fail(b"recv request"),
        };
        if reply.is_success() && reply.transferred > 0 && reply.flags == 0 {
            read += reply.transferred as usize;
        } else if reply.status_detail != net::STATUS_WOULD_BLOCK {
            fail(b"recv");
        }
        check_deadline(until);
    }
}

fn check_deadline(until: u64) {
    if now() >= until {
        fail(b"deadline");
    }
}

fn fail(reason: &[u8]) -> ! {
    debug_write(b"[io-network-lifetime-probe] fail: ");
    debug_write(reason);
    debug_write(b"\n");
    exit(1)
}
