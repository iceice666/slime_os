use boot_contracts::fabric_graph::{
    CONTRACT_KIND_CALL, DIRECTION_CLIENT, DIRECTION_SERVER, route_identity,
};
use slime_proto::capability_transfer::{
    CAPABILITY_TRANSFER_MAGIC, FORMAT_VERSION as TRANSFER_VERSION, OBJECT_KIND_ENDPOINT,
    OBJECT_KIND_SUPERVISION, WireCapabilityTransfer,
};
use slime_proto::fabric_call::{
    CALL_MAGIC, FLAG_NON_IDEMPOTENT, FORMAT_VERSION, KIND_CANCEL, KIND_REPLY, KIND_REQUEST,
    KIND_TERMINAL, STATUS_CANCELLED, STATUS_DUPLICATE, STATUS_MALFORMED_REPLY, STATUS_PEER_DEAD,
    STATUS_REJECTED, STATUS_RETRY_EXHAUSTED, STATUS_STALE, STATUS_TIMEOUT, WireCallEnvelope,
    WireCallTimeAdvance,
};
use slime_proto::interface_schema::parameter_call;
use slime_proto::sample_descriptor::{
    CAPABILITY_KIND_LOAN, SAMPLE_DESCRIPTOR_MAGIC, WireSampleDescriptor,
};

#[allow(dead_code)]
mod fabric_profile {
    include!(concat!(env!("OUT_DIR"), "/fabric_profile.rs"));
}
use fabric_profile::*;
use slime_rt::{
    ERR_OUT_OF_MEMORY, ERR_PEER_DEAD, ERR_SUCCESS, ERR_WOULDBLOCK, MAX_CAPS_PER_MSG, MAX_MSG,
    WaitSource,
};

const ROUTE_NAME: &str = "parameters";
const RIGHT_SEND: u64 = 1;
const RIGHT_RECV: u64 = 2;
const RIGHT_SUPERVISE: u64 = 1 << 18;
const SESSION: u64 = 0x000e_0000_0000_0001;
const MAX_CALLS: usize = FABRIC_MAX_IN_FLIGHT_CALLS;
const RETRY_LIMIT: u8 = FABRIC_MAX_RETRIES;
const DEADLINE_NS: u64 = FABRIC_CALL_DEADLINE_NS;
const RETRY_INTERVAL_NS: u64 = DEADLINE_NS / (RETRY_LIMIT as u64 + 1);
const PAGE: u64 = 4096;
const LOAN_BASE: u64 = 0x7200_0000;
const BUFFER_BASE: u64 = LOAN_BASE + PAGE;
const MAX_PENDING_TERMINALS_PER_CLIENT: usize = MAX_CALLS * 2;
const MAX_PENDING_TERMINALS: usize = MAX_PENDING_TERMINALS_PER_CLIENT * 2;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Free,
    Forwarding,
    AwaitingReply,
    Cancelling,
    ForwardingReply,
    PendingTerminal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Payload {
    None,
    Inline(WireCallEnvelope),
    Shared {
        buffer_slot: u32,
        descriptor: WireSampleDescriptor,
    },
    SharedOutstanding {
        buffer_slot: u32,
        loan_id: u64,
    },
    CancellingShared {
        buffer_slot: u32,
        loan_id: u64,
        message: WireCallEnvelope,
    },
    InlineReply(WireCallEnvelope),
    SharedReply {
        buffer_slot: u32,
        descriptor: WireSampleDescriptor,
    },
}

#[derive(Clone, Copy)]
struct Call {
    phase: Phase,
    request_id: u64,

    server_request_id: u64,
    client_session: u64,
    client_slot: u32,
    client_index: u8,
    retries: u8,
    deadline_ns: u64,
    next_retry_ns: u64,
    terminal_status: i32,
    payload: Payload,
}

impl Call {
    const EMPTY: Self = Self {
        phase: Phase::Free,
        request_id: 0,
        server_request_id: 0,
        client_session: 0,
        client_slot: 0,
        client_index: 0,
        retries: 0,
        deadline_ns: 0,
        next_retry_ns: 0,
        terminal_status: 0,
        payload: Payload::None,
    };
}

pub struct Broker {
    endpoint_factory_slot: u32,
    buffer_factory_slot: u32,
    client_control: [u32; 2],
    server_control: u32,
    time_control: u32,
    supervision: [u32; 3],
    clients: [Option<u32>; 2],
    server_slot: Option<u32>,
    calls: [Call; MAX_CALLS],
    high_water: [u64; 2],
    next_server_request_id: u64,
    now_ns: u64,
    pending_terminals: [Option<Call>; MAX_PENDING_TERMINALS],
    time_closed: bool,
}

impl Broker {
    pub const fn new(
        endpoint_factory_slot: u32,
        buffer_factory_slot: u32,
        client_control: [u32; 2],
        server_control: u32,
        time_control: u32,
        _legacy_server_supervision: u32,
    ) -> Self {
        Self {
            endpoint_factory_slot,
            buffer_factory_slot,
            client_control,
            server_control,
            time_control,
            supervision: [0; 3],
            clients: [None; 2],
            server_slot: None,
            calls: [Call::EMPTY; MAX_CALLS],
            high_water: [0; 2],
            next_server_request_id: 1,
            now_ns: 0,
            pending_terminals: [None; MAX_PENDING_TERMINALS],
            time_closed: false,
        }
    }

    pub fn run(&mut self) {
        self.provision();
        slime_rt::debug_write(b"[fabric] call roles provisioned\n");
        loop {
            let mut progressed = false;
            for index in 0..self.clients.len() {
                if self.can_receive_client(index) {
                    progressed |= self.pump_client(index);
                }
            }
            progressed |= self.observe_server_death();
            progressed |= self.pump_terminals();
            progressed |= self.pump_pending_terminals();
            progressed |= self.pump_server();
            progressed |= self.pump_replies();
            progressed |= self.pump_time();
            if self.calls.iter().all(|call| call.phase == Phase::Free)
                && self.pending_terminals.iter().all(Option::is_none)
                && self.server_slot.is_none()
                && self.time_closed
            {
                slime_rt::debug_write(b"[fabric] call state reclaimed\n");
                return;
            }
            if progressed {
                continue;
            }
            let mut sources = [WaitSource::Endpoint(0); 7];
            let mut count = 0;
            for (client, slot) in self
                .clients
                .iter()
                .enumerate()
                .filter_map(|(client, slot)| slot.map(|slot| (client, slot)))
            {
                if self.can_receive_client(client) {
                    sources[count] = WaitSource::Endpoint(slot);
                    count += 1;
                }
                if self.has_pending_delivery(slot) {
                    sources[count] = WaitSource::SendCapacity(slot);
                    count += 1;
                }
            }
            if let Some(slot) = self.server_slot {
                sources[count] = WaitSource::Endpoint(slot);
                count += 1;
            }
            if !self.time_closed {
                sources[count] = WaitSource::Endpoint(self.time_control);
                count += 1;
            }
            sources[count] = WaitSource::Supervision(self.supervision[2]);
            count += 1;
            slime_rt::wait(&sources[..count]);
        }
    }

    fn can_receive_client(&self, client: usize) -> bool {
        self.pending_terminals
            .iter()
            .flatten()
            .filter(|call| call.client_index as usize == client)
            .count()
            < MAX_PENDING_TERMINALS_PER_CLIENT
    }

    fn has_pending_delivery(&self, slot: u32) -> bool {
        self.calls.iter().any(|call| {
            call.client_slot == slot
                && matches!(call.phase, Phase::ForwardingReply | Phase::PendingTerminal)
        }) || self
            .pending_terminals
            .iter()
            .flatten()
            .any(|call| call.client_slot == slot)
    }

    fn provision(&mut self) {
        let route = route_identity(
            ROUTE_NAME,
            &parameter_call::INTERFACE_IDENTITY,
            CONTRACT_KIND_CALL,
        );
        let declared_clients: [&[u8]; 2] = [b"fabric-call-client", b"fabric-call-client-b"];
        for (index, component) in declared_clients.iter().enumerate() {
            let expected = FABRIC_PARTICIPANTS
                .iter()
                .filter(|(name, route_name, interface, direction)| {
                    *name == *component
                        && *route_name == ROUTE_NAME
                        && *interface == "ParameterCall"
                        && *direction == DIRECTION_CLIENT
                })
                .count();
            if expected != 1 {
                fail(b"call client graph declaration");
            }
            consume_request(self.client_control[index]);
            self.supervision[index] =
                consume_supervision(self.client_control[index], &route, DIRECTION_CLIENT);
            let (fabric_side, participant_side) =
                slime_rt::endpoint_create(self.endpoint_factory_slot)
                    .unwrap_or_else(|_| fail(b"client endpoint"));
            transfer_role(
                self.client_control[index],
                participant_side,
                &route,
                DIRECTION_CLIENT,
                RIGHT_SEND | RIGHT_RECV,
            );
            self.clients[index] = Some(fabric_side);
        }

        let servers = FABRIC_PARTICIPANTS
            .iter()
            .filter(|(name, route_name, interface, direction)| {
                *name == b"fabric-call-server"
                    && *route_name == ROUTE_NAME
                    && *interface == "ParameterCall"
                    && *direction == DIRECTION_SERVER
            })
            .count();
        if servers != 1 {
            fail(b"call server graph declaration");
        }
        consume_request(self.server_control);
        self.supervision[2] = consume_supervision(self.server_control, &route, DIRECTION_SERVER);
        let (fabric_side, participant_side) = slime_rt::endpoint_create(self.endpoint_factory_slot)
            .unwrap_or_else(|_| fail(b"server endpoint"));
        transfer_role(
            self.server_control,
            participant_side,
            &route,
            DIRECTION_SERVER,
            RIGHT_SEND | RIGHT_RECV,
        );
        self.server_slot = Some(fabric_side);
    }

    fn pump_client(&mut self, client: usize) -> bool {
        if !self.can_receive_client(client) {
            return false;
        }
        let Some(slot) = self.clients[client] else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.clients[client] = None;
                self.reclaim_client(slot);
                if slime_rt::cap_drop(slot) != ERR_SUCCESS {
                    fail(b"client endpoint drop")
                }
                return true;
            }
            value if value < 0 => fail(b"client recv"),
            value => value as usize,
        };
        let magic = (length >= 4).then(|| u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        match magic {
            Some(CALL_MAGIC) => {
                release_caps(&caps);
                let Some(message) = WireCallEnvelope::decode(&bytes[..length.min(MAX_MSG)]) else {
                    return true;
                };
                if length != MAX_MSG
                    || !slime_proto::valid_call_envelope(&message, parameter_call::TYPE_TAG)
                {
                    self.reject_terminal(
                        client,
                        slot,
                        message.session.max(1),
                        message.request_id.max(1),
                        STATUS_STALE,
                    );
                    return true;
                }
                if message.session == SESSION {
                    self.reject_terminal(
                        client,
                        slot,
                        message.session,
                        message.request_id,
                        STATUS_STALE,
                    );
                    slime_rt::debug_write(b"[fabric] stale call rejected\n");
                    return true;
                }
                match message.kind {
                    KIND_REQUEST => self.admit_inline(client, slot, message),
                    KIND_CANCEL => self.cancel(client, slot, message),
                    _ => {
                        self.reject_terminal(
                            client,
                            slot,
                            message.session,
                            message.request_id,
                            STATUS_STALE,
                        );
                        slime_rt::debug_write(b"[fabric] client reply authority denied\n");
                    }
                }
            }
            Some(SAMPLE_DESCRIPTOR_MAGIC) => {
                let loan_slot = caps[0] as u32;
                for cap in caps.iter().skip(1).filter(|cap| **cap != 0) {
                    let _ = slime_rt::cap_drop(*cap as u32);
                }
                let Some(descriptor) = WireSampleDescriptor::decode(&bytes[..length.min(MAX_MSG)])
                else {
                    if loan_slot != 0 {
                        let _ = slime_rt::cap_drop(loan_slot);
                    }
                    return true;
                };
                if length != MAX_MSG
                    || loan_slot == 0
                    || !slime_proto::valid_sample_descriptor(
                        &descriptor,
                        descriptor.loan_id,
                        parameter_call::TYPE_TAG,
                        PAGE,
                    )
                    || descriptor.capability_kind != CAPABILITY_KIND_LOAN
                {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    self.reject_terminal(
                        client,
                        slot,
                        client_session(client),
                        descriptor.sequence.max(1),
                        STATUS_STALE,
                    );
                    return true;
                }
                self.admit_shared(client, slot, descriptor, loan_slot);
            }
            _ => release_caps(&caps),
        }
        true
    }

    fn admit_inline(&mut self, client: usize, slot: u32, message: WireCallEnvelope) {
        self.admit(
            client,
            slot,
            message.session,
            message.request_id,
            Payload::Inline(message),
        );
    }

    fn admit_shared(
        &mut self,
        client: usize,
        slot: u32,
        descriptor: WireSampleDescriptor,
        loan_slot: u32,
    ) {
        let buffer_slot = relay_shared_payload(self.buffer_factory_slot, loan_slot, &descriptor);
        self.admit(
            client,
            slot,
            client_session(client),
            descriptor.sequence,
            Payload::Shared {
                buffer_slot,
                descriptor,
            },
        );
    }

    fn admit(&mut self, client: usize, slot: u32, session: u64, request_id: u64, payload: Payload) {
        if session != client_session(client) {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_STALE);
            slime_rt::debug_write(b"[fabric] stale call rejected\n");
            return;
        }
        if request_id <= self.high_water[client] {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_DUPLICATE);
            slime_rt::debug_write(b"[fabric] duplicate call rejected\n");
            return;
        }
        let Some(index) = self.calls.iter().position(|call| call.phase == Phase::Free) else {
            settle_payload(payload);
            self.reject_terminal(client, slot, session, request_id, STATUS_RETRY_EXHAUSTED);
            slime_rt::debug_write(b"[fabric] call retry exhausted\n");
            return;
        };
        let server_request_id = self.next_server_request_id;
        self.next_server_request_id = self
            .next_server_request_id
            .checked_add(1)
            .unwrap_or_else(|| fail(b"server correlation exhausted"));
        self.high_water[client] = request_id;
        self.calls[index] = Call {
            phase: Phase::Forwarding,
            request_id,
            server_request_id,
            client_session: session,
            client_slot: slot,
            client_index: client as u8,
            retries: 0,
            deadline_ns: self.now_ns.saturating_add(DEADLINE_NS),
            next_retry_ns: self.now_ns,
            terminal_status: 0,
            payload,
        };
        self.forward(index);
    }

    fn reject_terminal(
        &mut self,
        client: usize,
        slot: u32,
        session: u64,
        request_id: u64,
        status: i32,
    ) {
        let pending = Call {
            phase: Phase::PendingTerminal,
            request_id,
            server_request_id: 0,
            client_session: session,
            client_slot: slot,
            client_index: client as u8,
            retries: 0,
            deadline_ns: self.now_ns.saturating_add(DEADLINE_NS),
            next_retry_ns: 0,
            terminal_status: status,
            payload: Payload::None,
        };
        if let Some(index) = self.calls.iter().position(|call| call.phase == Phase::Free) {
            self.calls[index] = pending;
            self.pump_terminal(index);
            return;
        }
        let Some(index) = self.pending_terminals.iter().position(Option::is_none) else {
            self.clients[client] = None;
            self.reclaim_client(slot);
            let _ = slime_rt::cap_drop(slot);
            slime_rt::debug_write(b"[fabric] saturated client isolated\n");
            return;
        };
        self.pending_terminals[index] = Some(pending);
        slime_rt::debug_write(b"[fabric] terminal delivery queued\n");
    }

    fn pump_terminal(&mut self, index: usize) -> bool {
        let call = self.calls[index];
        match try_send_terminal(
            call.client_slot,
            call.client_session,
            call.request_id,
            call.terminal_status,
        ) {
            ERR_SUCCESS | ERR_PEER_DEAD => {
                self.calls[index] = Call::EMPTY;
                true
            }
            ERR_WOULDBLOCK => {
                slime_rt::debug_write(b"[fabric] terminal delivery queued\n");
                false
            }
            _ => fail(b"call terminal"),
        }
    }

    fn pump_pending_terminals(&mut self) -> bool {
        let mut progressed = false;
        let mut dead_slot = None;
        for pending in &mut self.pending_terminals {
            let Some(call) = *pending else {
                continue;
            };
            match try_send_terminal(
                call.client_slot,
                call.client_session,
                call.request_id,
                call.terminal_status,
            ) {
                ERR_SUCCESS => {
                    *pending = None;
                    progressed = true;
                }
                ERR_PEER_DEAD => {
                    *pending = None;
                    dead_slot = Some(call.client_slot);
                    progressed = true;
                    break;
                }
                ERR_WOULDBLOCK => {}
                _ => fail(b"call terminal"),
            }
        }
        if let Some(slot) = dead_slot {
            for pending in &mut self.pending_terminals {
                if pending.is_some_and(|call| call.client_slot == slot) {
                    *pending = None;
                }
            }
            self.reclaim_client(slot);
        }
        progressed
    }

    fn forward(&mut self, index: usize) {
        let Some(server) = self.server_slot else {
            self.finish(index, STATUS_PEER_DEAD);
            return;
        };
        let call = self.calls[index];
        let mut next_payload = Payload::None;
        let result = match call.payload {
            Payload::Inline(mut message) => {
                message.session = SESSION;
                message.request_id = call.server_request_id;
                slime_rt::send(server, &message.encode(), &[])
            }
            Payload::Shared {
                buffer_slot,
                mut descriptor,
            } => {
                descriptor.sequence = call.server_request_id;
                let loan = match slime_rt::shared_buffer_loan(
                    buffer_slot,
                    self.supervision[2],
                    0,
                    descriptor.length,
                ) {
                    Ok(loan) => loan,
                    Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => return,
                    Err(_) => fail(b"shared request loan"),
                };
                descriptor.loan_id = loan.id;
                let sent = slime_rt::send(server, &descriptor.encode(), &[loan.slot]);
                if sent == ERR_SUCCESS {
                    next_payload = Payload::SharedOutstanding {
                        buffer_slot,
                        loan_id: loan.id,
                    };
                } else {
                    let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                }
                sent
            }
            Payload::CancellingShared {
                buffer_slot,
                loan_id,
                message,
            } => {
                let sent = slime_rt::send(server, &message.encode(), &[]);
                if sent == ERR_SUCCESS {
                    next_payload = Payload::SharedOutstanding {
                        buffer_slot,
                        loan_id,
                    };
                }
                sent
            }
            Payload::None
            | Payload::SharedOutstanding { .. }
            | Payload::InlineReply(_)
            | Payload::SharedReply { .. } => fail(b"forward invalid payload"),
        };
        match result {
            ERR_SUCCESS => {
                let was_cancelling = self.calls[index].phase == Phase::Cancelling;
                self.calls[index].phase = if was_cancelling {
                    Phase::Cancelling
                } else {
                    Phase::AwaitingReply
                };
                self.calls[index].payload = next_payload;
                if was_cancelling {
                    slime_rt::debug_write(b"[fabric] call cancellation forwarded\n");
                } else {
                    slime_rt::debug_write(b"[fabric] call forwarded\n");
                }
            }
            ERR_WOULDBLOCK => {
                self.calls[index].retries = self.calls[index].retries.saturating_add(1);
                self.calls[index].next_retry_ns = self.now_ns.saturating_add(RETRY_INTERVAL_NS);
                if self.calls[index].retries >= RETRY_LIMIT {
                    let status = if self.calls[index].phase == Phase::Cancelling {
                        STATUS_CANCELLED
                    } else {
                        STATUS_RETRY_EXHAUSTED
                    };
                    self.finish(index, status);
                    if status == STATUS_CANCELLED {
                        slime_rt::debug_write(b"[fabric] call cancelled\n");
                    } else {
                        slime_rt::debug_write(b"[fabric] call retry exhausted\n");
                    }
                }
            }
            ERR_PEER_DEAD => {
                self.server_slot = None;
                self.finish(index, STATUS_PEER_DEAD);
            }
            _ => fail(b"call forward"),
        }
    }

    fn cancel(&mut self, client: usize, client_slot: u32, message: WireCallEnvelope) {
        let Some(index) = self.calls.iter().position(|call| {
            call.phase != Phase::Free
                && call.request_id == message.request_id
                && call.client_session == message.session
                && call.client_slot == client_slot
        }) else {
            self.reject_terminal(
                client,
                client_slot,
                message.session,
                message.request_id,
                STATUS_STALE,
            );
            return;
        };
        if self.calls[index].phase == Phase::AwaitingReply {
            let Some(server) = self.server_slot else {
                self.finish(index, STATUS_PEER_DEAD);
                return;
            };
            let cancel = WireCallEnvelope {
                magic: CALL_MAGIC,
                version: FORMAT_VERSION,
                kind: KIND_CANCEL,
                flags: 0,
                session: SESSION,
                request_id: self.calls[index].server_request_id,
                type_identity: parameter_call::TYPE_TAG,
                status: STATUS_CANCELLED,
                payload_len: 0,
                payload: [0; 16],
            };
            match slime_rt::send(server, &cancel.encode(), &[]) {
                ERR_SUCCESS => {
                    self.calls[index].phase = Phase::Cancelling;
                    self.calls[index].retries = 0;
                    self.calls[index].terminal_status = STATUS_CANCELLED;
                    slime_rt::debug_write(b"[fabric] call cancellation forwarded\n");
                }
                ERR_WOULDBLOCK => {
                    let outstanding = self.calls[index].payload;
                    self.calls[index].phase = Phase::Cancelling;
                    self.calls[index].payload = match outstanding {
                        Payload::SharedOutstanding {
                            buffer_slot,
                            loan_id,
                        } => Payload::CancellingShared {
                            buffer_slot,
                            loan_id,
                            message: cancel,
                        },
                        _ => Payload::Inline(cancel),
                    };
                    self.calls[index].retries = 1;
                    self.calls[index].next_retry_ns = self.now_ns.saturating_add(RETRY_INTERVAL_NS);
                    self.calls[index].terminal_status = STATUS_CANCELLED;
                }
                ERR_PEER_DEAD => {
                    self.server_slot = None;
                    self.finish(index, STATUS_PEER_DEAD);
                }
                _ => fail(b"call cancel forward"),
            }
            return;
        }
        self.finish(index, STATUS_CANCELLED);
        slime_rt::debug_write(b"[fabric] call cancelled\n");
    }

    fn pump_server(&mut self) -> bool {
        let Some(slot) = self.server_slot else {
            return false;
        };
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.server_slot = None;
                self.reclaim_all(STATUS_PEER_DEAD);
                return true;
            }
            value if value < 0 => fail(b"server recv"),
            value => value as usize,
        };
        let magic = (length >= 4).then(|| u32::from_le_bytes(bytes[..4].try_into().unwrap()));
        match magic {
            Some(CALL_MAGIC) => {
                release_caps(&caps);
                let decoded = WireCallEnvelope::decode(&bytes[..length.min(MAX_MSG)]);
                let Some(reply) = decoded.filter(|reply| {
                    length == MAX_MSG
                        && slime_proto::valid_call_envelope(reply, parameter_call::TYPE_TAG)
                        && reply.kind == KIND_REPLY
                        && reply.session == SESSION
                }) else {
                    if let Some(reply) = decoded
                        && let Some(index) = self.find_call(reply.request_id)
                    {
                        self.finish(index, STATUS_MALFORMED_REPLY);
                        slime_rt::debug_write(b"[fabric] malformed call reply rejected\n");
                    }
                    return true;
                };
                let Some(index) = self.find_call(reply.request_id) else {
                    slime_rt::debug_write(b"[fabric] stale call reply rejected\n");
                    return true;
                };
                let mut outward = reply;
                settle_outstanding_request(&mut self.calls[index]);
                outward.session = self.calls[index].client_session;
                outward.request_id = self.calls[index].request_id;
                let status = outward.status;
                if self.calls[index].phase == Phase::Cancelling {
                    self.finish(index, STATUS_CANCELLED);
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                } else {
                    self.deliver_inline_reply(index, outward);
                    if status == STATUS_REJECTED {
                        slime_rt::debug_write(b"[fabric] server rejection routed\n");
                    } else {
                        slime_rt::debug_write(b"[fabric] call reply correlated\n");
                    }
                }
            }
            Some(SAMPLE_DESCRIPTOR_MAGIC) => {
                let loan_slot = caps[0] as u32;
                for cap in caps.iter().skip(1).filter(|cap| **cap != 0) {
                    let _ = slime_rt::cap_drop(*cap as u32);
                }
                let Some(descriptor) = WireSampleDescriptor::decode(&bytes[..length.min(MAX_MSG)])
                else {
                    if loan_slot != 0 {
                        let _ = slime_rt::cap_drop(loan_slot);
                    }
                    return true;
                };
                let Some(index) = self.find_call(descriptor.sequence) else {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    slime_rt::debug_write(b"[fabric] stale call reply rejected\n");
                    return true;
                };
                if length != MAX_MSG
                    || loan_slot == 0
                    || !slime_proto::valid_sample_descriptor(
                        &descriptor,
                        descriptor.loan_id,
                        parameter_call::TYPE_TAG,
                        PAGE,
                    )
                {
                    if loan_slot != 0 {
                        let _ = slime_rt::shared_buffer_return(loan_slot);
                    }
                    self.finish(index, STATUS_MALFORMED_REPLY);
                    return true;
                }
                settle_outstanding_request(&mut self.calls[index]);
                if self.calls[index].phase == Phase::Cancelling {
                    let _ = slime_rt::shared_buffer_return(loan_slot);
                    self.finish(index, STATUS_CANCELLED);
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                    return true;
                }
                let mut outward = descriptor;
                outward.sequence = self.calls[index].request_id;
                let buffer_slot =
                    relay_shared_payload(self.buffer_factory_slot, loan_slot, &descriptor);
                self.deliver_shared_reply(index, outward, buffer_slot);
            }
            _ => release_caps(&caps),
        }
        true
    }

    fn deliver_inline_reply(&mut self, index: usize, outward: WireCallEnvelope) {
        let client = self.calls[index].client_slot;
        match slime_rt::send(client, &outward.encode(), &[]) {
            ERR_SUCCESS => self.calls[index] = Call::EMPTY,
            ERR_WOULDBLOCK => {
                self.calls[index].phase = Phase::ForwardingReply;
                self.calls[index].payload = Payload::InlineReply(outward);
            }
            ERR_PEER_DEAD => self.drop_dead_client(index, b"client endpoint drop"),
            _ => fail(b"reply delivery"),
        }
    }

    fn deliver_shared_reply(
        &mut self,
        index: usize,
        mut descriptor: WireSampleDescriptor,
        buffer_slot: u32,
    ) {
        let client = self.calls[index].client_slot;
        let supervision = self.supervision[self.calls[index].client_index as usize];
        let loan =
            match slime_rt::shared_buffer_loan(buffer_slot, supervision, 0, descriptor.length) {
                Ok(loan) => loan,
                Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => {
                    self.calls[index].phase = Phase::ForwardingReply;
                    self.calls[index].payload = Payload::SharedReply {
                        buffer_slot,
                        descriptor,
                    };
                    return;
                }
                Err(_) => fail(b"shared client loan"),
            };
        descriptor.loan_id = loan.id;
        match slime_rt::send(client, &descriptor.encode(), &[loan.slot]) {
            ERR_SUCCESS => {
                let _ = slime_rt::shared_buffer_release(buffer_slot);
                self.calls[index] = Call::EMPTY;
            }
            ERR_WOULDBLOCK => {
                let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                self.calls[index].phase = Phase::ForwardingReply;
                self.calls[index].payload = Payload::SharedReply {
                    buffer_slot,
                    descriptor,
                };
            }
            ERR_PEER_DEAD => {
                let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                let _ = slime_rt::shared_buffer_release(buffer_slot);
                self.drop_dead_client(index, b"shared client endpoint drop");
            }
            _ => fail(b"shared reply delivery"),
        }
    }

    fn pump_replies(&mut self) -> bool {
        let mut progressed = false;
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::ForwardingReply {
                continue;
            }
            let call = self.calls[index];
            match call.payload {
                Payload::InlineReply(reply) => {
                    match slime_rt::send(call.client_slot, &reply.encode(), &[]) {
                        ERR_SUCCESS => {
                            self.calls[index] = Call::EMPTY;
                            progressed = true;
                        }
                        ERR_WOULDBLOCK => {}
                        ERR_PEER_DEAD => {
                            self.drop_dead_client(index, b"client endpoint drop");
                            progressed = true;
                        }
                        _ => fail(b"reply delivery"),
                    }
                }
                Payload::SharedReply {
                    buffer_slot,
                    mut descriptor,
                } => {
                    let supervision = self.supervision[call.client_index as usize];
                    let loan = match slime_rt::shared_buffer_loan(
                        buffer_slot,
                        supervision,
                        0,
                        descriptor.length,
                    ) {
                        Ok(loan) => loan,
                        Err(ERR_WOULDBLOCK) | Err(ERR_OUT_OF_MEMORY) => continue,
                        Err(_) => fail(b"shared client loan"),
                    };
                    descriptor.loan_id = loan.id;
                    match slime_rt::send(call.client_slot, &descriptor.encode(), &[loan.slot]) {
                        ERR_SUCCESS => {
                            let _ = slime_rt::shared_buffer_release(buffer_slot);
                            self.calls[index] = Call::EMPTY;
                            progressed = true;
                        }
                        ERR_WOULDBLOCK => {
                            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                        }
                        ERR_PEER_DEAD => {
                            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan.id);
                            let _ = slime_rt::shared_buffer_release(buffer_slot);
                            self.drop_dead_client(index, b"shared client endpoint drop");
                            progressed = true;
                        }
                        _ => fail(b"shared reply delivery"),
                    }
                }
                _ => fail(b"pending reply payload"),
            }
        }
        progressed
    }

    fn drop_dead_client(&mut self, index: usize, reason: &[u8]) {
        let client_index = self.calls[index].client_index as usize;
        let client = self.calls[index].client_slot;
        settle_payload(self.calls[index].payload);
        self.clients[client_index] = None;
        self.calls[index] = Call::EMPTY;
        if slime_rt::cap_drop(client) != ERR_SUCCESS {
            fail(reason)
        }
    }

    fn pump_time(&mut self) -> bool {
        if self.time_closed {
            return false;
        }
        let mut bytes = [0u8; MAX_MSG];
        let mut caps = [0u64; MAX_CAPS_PER_MSG];
        let length = match slime_rt::recv(self.time_control, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => return false,
            ERR_PEER_DEAD => {
                self.time_closed = true;
                return true;
            }
            value if value < 0 => fail(b"call time recv"),
            value => value as usize,
        };
        release_caps(&caps);
        let Some(value) = WireCallTimeAdvance::decode(&bytes[..length.min(MAX_MSG)]) else {
            return true;
        };
        if length != MAX_MSG
            || !slime_proto::valid_call_time_advance(&value)
            || value.now_ns < self.now_ns
        {
            fail(b"invalid call time");
        }
        self.now_ns = value.now_ns;
        for index in 0..self.calls.len() {
            if self.calls[index].phase == Phase::Free {
                continue;
            }
            if self.now_ns >= self.calls[index].deadline_ns {
                let status = if self.calls[index].phase == Phase::Cancelling {
                    STATUS_CANCELLED
                } else {
                    STATUS_TIMEOUT
                };
                self.finish(index, status);
                if status == STATUS_CANCELLED {
                    slime_rt::debug_write(b"[fabric] call cancelled\n");
                } else {
                    slime_rt::debug_write(b"[fabric] call timed out\n");
                }
            } else if matches!(
                self.calls[index].phase,
                Phase::Forwarding | Phase::Cancelling
            ) && !matches!(self.calls[index].payload, Payload::None)
                && self.now_ns >= self.calls[index].next_retry_ns
            {
                self.forward(index);
            }
        }
        true
    }

    fn observe_server_death(&mut self) -> bool {
        if self.server_slot.is_none() {
            return false;
        }
        match slime_rt::supervision_status(self.supervision[2]) {
            Ok(None) => false,
            Ok(Some(_)) => {
                self.server_slot = None;
                self.reclaim_all(STATUS_PEER_DEAD);
                slime_rt::debug_write(b"[fabric] call peer death propagated\n");
                self.time_closed = true;
                true
            }
            Err(_) => fail(b"server supervision"),
        }
    }

    fn find_call(&self, server_request_id: u64) -> Option<usize> {
        self.calls.iter().position(|call| {
            matches!(call.phase, Phase::AwaitingReply | Phase::Cancelling)
                && call.server_request_id == server_request_id
        })
    }
    fn finish(&mut self, index: usize, status: i32) {
        let call = self.calls[index];
        if call.phase == Phase::Free {
            return;
        }
        settle_payload(call.payload);
        self.calls[index].payload = Payload::None;
        match try_send_terminal(
            call.client_slot,
            call.client_session,
            call.request_id,
            status,
        ) {
            ERR_SUCCESS | ERR_PEER_DEAD => self.calls[index] = Call::EMPTY,
            ERR_WOULDBLOCK => {
                self.calls[index].phase = Phase::PendingTerminal;
                self.calls[index].terminal_status = status;
            }
            _ => fail(b"call terminal"),
        }
    }

    fn pump_terminals(&mut self) -> bool {
        let mut progressed = false;
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::PendingTerminal {
                continue;
            }
            progressed |= self.pump_terminal(index);
        }
        progressed
    }

    fn reclaim_client(&mut self, slot: u32) {
        for index in 0..self.calls.len() {
            if self.calls[index].phase != Phase::Free && self.calls[index].client_slot == slot {
                settle_payload(self.calls[index].payload);
                self.calls[index] = Call::EMPTY;
            }
        }
    }

    fn reclaim_all(&mut self, status: i32) {
        for index in 0..self.calls.len() {
            self.finish(index, status);
        }
    }
}

fn consume_supervision(control: u32, route: &[u8; 32], direction: u32) -> u32 {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(control, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => {
                slime_rt::yield_now();
                continue;
            }
            value if value < 0 => fail(b"supervision receive"),
            value => {
                if value as usize != MAX_MSG || caps[0] == 0 {
                    release_caps(&caps);
                    fail(b"supervision shape");
                }
                let descriptor = WireCapabilityTransfer::decode(&bytes)
                    .unwrap_or_else(|| fail(b"supervision decode"));
                if descriptor.magic != CAPABILITY_TRANSFER_MAGIC
                    || descriptor.version != TRANSFER_VERSION
                    || descriptor.status != 0
                    || descriptor.flags != 0
                    || descriptor.object_kind != OBJECT_KIND_SUPERVISION
                    || descriptor.direction != direction
                    || descriptor.rights_mask != RIGHT_SUPERVISE
                    || descriptor.route_identity != *route
                {
                    release_caps(&caps);
                    fail(b"supervision authority");
                }
                for cap in caps.iter().skip(1).filter(|cap| **cap != 0) {
                    let _ = slime_rt::cap_drop(*cap as u32);
                }
                return caps[0] as u32;
            }
        }
    }
}

fn settle_outstanding_request(call: &mut Call) {
    let outstanding = match call.payload {
        Payload::SharedOutstanding {
            buffer_slot,
            loan_id,
        }
        | Payload::CancellingShared {
            buffer_slot,
            loan_id,
            ..
        } => Some((buffer_slot, loan_id)),
        _ => None,
    };
    if let Some((buffer_slot, loan_id)) = outstanding {
        let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan_id);
        let _ = slime_rt::shared_buffer_release(buffer_slot);
        call.payload = Payload::None;
    }
}

fn client_session(client: usize) -> u64 {
    0x00c1_0000_0000_0001 + client as u64 * 0x0001_0000_0000_0000
}

fn settle_payload(payload: Payload) {
    match payload {
        Payload::Shared { buffer_slot, .. } | Payload::SharedReply { buffer_slot, .. } => {
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        Payload::SharedOutstanding {
            buffer_slot,
            loan_id,
        }
        | Payload::CancellingShared {
            buffer_slot,
            loan_id,
            ..
        } => {
            let _ = slime_rt::shared_buffer_revoke(buffer_slot, loan_id);
            let _ = slime_rt::shared_buffer_release(buffer_slot);
        }
        Payload::None | Payload::Inline(_) | Payload::InlineReply(_) => {}
    }
}

fn relay_shared_payload(
    factory_slot: u32,
    loan_slot: u32,
    descriptor: &WireSampleDescriptor,
) -> u32 {
    if slime_rt::shared_buffer_loan_map(loan_slot, LOAN_BASE, 0, descriptor.length) != ERR_SUCCESS {
        let _ = slime_rt::shared_buffer_return(loan_slot);
        fail(b"shared relay source map");
    }
    let buffer = slime_rt::shared_buffer_create(factory_slot, 1, true)
        .unwrap_or_else(|_| fail(b"shared relay create"));
    if slime_rt::shared_buffer_map(buffer.slot, BUFFER_BASE, 0, descriptor.length, true)
        != ERR_SUCCESS
    {
        fail(b"shared relay target map");
    }
    unsafe {
        core::ptr::copy_nonoverlapping(
            LOAN_BASE as *const u8,
            BUFFER_BASE as *mut u8,
            descriptor.length as usize,
        );
    }
    if slime_rt::shared_buffer_unmap(loan_slot, LOAN_BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_return(loan_slot) != ERR_SUCCESS
        || slime_rt::shared_buffer_unmap(buffer.slot, BUFFER_BASE) != ERR_SUCCESS
        || slime_rt::shared_buffer_seal(buffer.slot) != ERR_SUCCESS
    {
        fail(b"shared relay settle");
    }
    buffer.slot
}

fn consume_request(slot: u32) {
    let mut bytes = [0u8; MAX_MSG];
    let mut caps = [0u64; MAX_CAPS_PER_MSG];
    loop {
        match slime_rt::recv(slot, &mut bytes, &mut caps) {
            ERR_WOULDBLOCK => slime_rt::wait(&[WaitSource::Endpoint(slot)]),
            value if value < 0 => fail(b"call role request"),
            _ => {
                release_caps(&caps);
                return;
            }
        }
    }
}

fn transfer_role(control: u32, capability: u32, route: &[u8; 32], direction: u32, rights: u64) {
    let descriptor = WireCapabilityTransfer {
        magic: CAPABILITY_TRANSFER_MAGIC,
        version: TRANSFER_VERSION,
        status: 0,
        flags: 0,
        object_kind: OBJECT_KIND_ENDPOINT,
        direction,
        rights_mask: rights,
        route_identity: *route,
    };
    if slime_rt::cap_transfer(control, capability, &descriptor.encode()) != ERR_SUCCESS {
        fail(b"call role transfer");
    }
}

fn try_send_terminal(slot: u32, session: u64, request_id: u64, status: i32) -> i64 {
    let message = WireCallEnvelope {
        magic: CALL_MAGIC,
        version: FORMAT_VERSION,
        kind: KIND_TERMINAL,
        flags: 0,
        session,
        request_id,
        type_identity: parameter_call::TYPE_TAG,
        status,
        payload_len: 0,
        payload: [0; 16],
    };
    slime_rt::send(slot, &message.encode(), &[])
}

fn release_caps(caps: &[u64; MAX_CAPS_PER_MSG]) {
    for slot in caps.iter().filter(|slot| **slot != 0) {
        let _ = slime_rt::cap_drop(*slot as u32);
    }
}

fn fail(reason: &[u8]) -> ! {
    slime_rt::debug_write(b"[fabric] fail: ");
    slime_rt::debug_write(reason);
    slime_rt::debug_write(b"\n");
    slime_rt::exit(1)
}

const _: () = assert!(slime_proto::fabric_call::CALL_LEN == MAX_MSG);
const _: () = assert!(slime_proto::fabric_call::CALL_TIME_LEN == MAX_MSG);
const _: () = assert!(FLAG_NON_IDEMPOTENT == 1);
