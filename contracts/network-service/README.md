# Network service protocol

`v1/schema.zt` owns the application request, completion, and provisioning bytes.
Regenerate the Rust bindings with
`python3 scripts/generate/generate-network-service-bindings.py`.
The existing request and completion layouts remain 56 and 24 bytes respectively.
The retained endpoint-only authority fixture exercises policy and capability
bookkeeping without a transport. Only the provisioned IO0 path described below
performs TCP I/O; a successful legacy endpoint request is not transport evidence.

## Provisioning and lifetime

The client resolves separate declared control and provisioning endpoints. The
control endpoint is nontransferable; it carries the endpoint `OP_ATTACH` request
with all operation fields zero. After admitting the binding declared by
[`network-application/v1`](../network-application/README.md), the service allocates two distinct backing buffers through
its explicit shared-buffer factory: one ring page and one 4096-byte payload page.
It formats an IO0 ring with `QUEUE_SLOTS = 4`, then delegates a writable
`SharedBufferLoan` for each page to the client on the provisioning endpoint,
ring first. Each delegation carries
the 64-byte generated `WireNetworkLoan` descriptor. `role` distinguishes ring and
data; both reserved arrays must be zero. `buffer` and `lease` are nonzero
identities and `length` is the exact generated size for that role.

The service never imports application-selected backing memory. Buffer and lease
identities come from root allocation/loan results retained by the service, not
from application descriptor claims. The service accepts IO0 slices only when
their identities match that session's retained payload identity and their ranges
and directions fit its exact mapping. The client can return its loans but cannot
revoke the service's own mappings or substitute aliased ring/data backing.

The client imports and maps the service's loans in descriptor order. Root's
logical capability import is a task-global FIFO, not an authenticated binding of
descriptor bytes to a specific export. Consequently the client attach API requires
trusted service endpoints and no concurrent logical import during provisioning;
it is not a generic untrusted multi-export broker. Application-to-service loan
provisioning is unsupported. Native delegation/endpoint sends are blocking
rendezvous; bounded ring polling does not qualify dead-peer-safe provisioning.

The provisioning endpoint permits transfers but carries no application control
operations. Copying it cannot authorize attach or teardown, and loan imports are
still bound to the generation's original peer task. The service's provisioning
slot is pinned above its bounded live logical-buffer capability footprint to
avoid the root's separate logical/native slot namespaces shadowing loan receiver
resolution. Increasing composition resource budgets requires rechecking that bound.

After delegating both loans, the service sends a control-endpoint completion
with `OP_ATTACH`, `STATUS_SUCCESS`, `CAPABILITY_NONE`, capability zero, and flags
zero. During descriptor reception the client also polls control for typed attach
errors; an allocation failure cannot strand it waiting on the provisioning edge.
`OP_ATTACH` is provisioning control, not an admissible IO0 request.
Application requests and completions thereafter use the IO0 rings. Connect and
close have an entirely zero `WireBufferSlice`; send uses `DIRECTION_DEVICE_READ`
and receive uses `DIRECTION_DEVICE_WRITE`. Transferred length comes from the IO0
completion and may be smaller than the supplied payload capacity.

The synchronous adapter has one outstanding request. It settles that identity
exactly once on every terminal path. Missing or malformed completion poisons the
session and returns the loans without further payload reuse; local settlement
alone does not establish that the service stopped accessing a shared mapping.
A readiness-enabled adapter signals request notifications and waits for completion
or service-liveness/deadline evidence; the external profile without readiness
bindings retains bounded polling. Dropping the adapter or reaching its local
limit does not send the endpoint teardown sentinel. Returning client loans alone
therefore does not prove service-side reclamation. Profiles with a declared
client supervision capability reclaim that holder's sockets and application
buffers after observed death; normal callers must still use successful `finish`.
A live client that silently abandons its adapter is not client-death evidence.

Session teardown retains the endpoint `OP_CLOSE` request with capability
`u64::MAX` as a sentinel. It is not a connection capability. After the client has
settled its outstanding request, it drops its mapped views and returns both loans
before sending this sentinel. The service checks ring quiescence, drops its mapped
views, and unmaps/releases its own buffers before sending an endpoint `OP_CLOSE`
success completion with no capability or flags. Individual TCP closes remain IO0
requests. A failed session marks its ring dead and settles its live work; teardown
releases service-owned buffers without taking down other sessions.

The additive control-only `OP_ABORT` uses the same sentinel and otherwise empty
request fields. The authenticated nontransferable control endpoint selects the
holder; no request field can name another session. It is refused as an IO0
operation. Unlike normal close, abort does not require ring quiescence: after the
client drops its views and returns loans, the service releases that holder's
transport state, marks the ring dead, invalidates admitted and queued work,
revokes any remaining loans and releases both backing buffers before acknowledging.
Invalidation does not promise a separate completion for each abandoned request. It may
also close an admitted client that never attached. A successful abort is explicit
failure-path reclamation, not successful HTTP delivery or graceful TCP close.

## Bounded HTTP launch input

The additive `WireNetworkLaunch` record in `v1/schema.zt` is exactly 64 bytes.
Its generated Rust and Python bindings share one layout: magic/version, role,
reserved zero bytes, total length, offset, chunk length, and a 48-byte zero-padded
payload. The `LAUNCH_SEED` role is exactly one 32-byte seed at offset zero;
`LAUNCH_URL` carries contiguous nonempty chunks of one URL of at most 2048 bytes.
Receivers reject role confusion, inconsistent totals, gaps, overlaps, nonzero
padding and excess bytes before using the payload. The role alone authorizes
nothing: separate nontransferable launch endpoints select service and client.

The QEMU `http-launcher` testkit component has explicit input and clock authority.
It reads lowercase hexadecimal encodings of those records, one per input line,
without echoing them; the host supplies fresh operating-system entropy, never an
image-embedded or fixed test seed. Only the service receives the seed; only the
HTTP client receives URL chunks. Missing input, malformed input, an all-zero
seed, or an input deadline failure refuses startup. This trusted host-launch
source is a QEMU qualification boundary, not a hardware entropy-source claim.
The seed must not appear in transcripts, command arguments, or persisted images.

Launch delivery, like existing loan provisioning, uses native blocking endpoint
rendezvous. Its declared receivers must remain alive during startup; the host
watchdog bounds failed qualification. Input polling has a 30-second deadline,
but that is not a dead-peer-safe rendezvous guarantee.

`NetworkIo::set_deadline` optionally binds subsequent transactions to one
absolute monotonic deadline, which can be shortened but not extended. Polling
and notification paths check it; notification timers use the earlier of the
request timeout and that deadline. Expiry before publication preserves the
session. Expiry after publication settles the local request and poisons the
session, returning loans without reusing the payload. Explicit control abort can
reclaim a poisoned session without reusing those mappings; profiles with declared
client supervision separately reclaim after observed death. A poisoned adapter
does not claim successful normal `finish` or replay the interrupted operation.
The deadline-aware attach API carries the same deadline through loan and control
polling; the HTTP client gives detach/abort a separate five-second cleanup grace.
HTTP compositions declare request/completion notifications and timer authority,
so an idle socket waits rather than spending the root dispatcher iteration budget
on repeated polling before a monotonic deadline can expire.
Native endpoint sends remain rendezvous operations, so these polling deadlines
are not a claim that a dead peer can never block a startup or teardown send.

## Exact-name DNS/connect

`NetworkIo::connect_hostname` submits the existing TCP `OP_CONNECT` with
`ADDRESS_DNS`; its original name is bounded to the wire contract's 24 bytes.
The service authorizes holder, exact name, TCP, port and CONNECT before sending
DNS. It reserves the connection against that original destination row, retaining
its independent SEND/RECV rights and budgets. Numeric `connect_ipv4` still needs
its own exact numeric grant. There is no application resolver socket, transferable
answer handle, DNS cache, or DNS-to-numeric grant conversion.

One extra fixed UDP socket serves one pending resolution/connect transaction.
The generation must declare exactly one service-held IPv4 UDP resolver destination
on port 53 or 1053, SEND/RECV rights, a socket, queue and timer, at least 1024
buffer bytes and a nonzero record bound. Queries use fresh launch seed material
and a checked SHA-256 counter construction to select a 16-bit ID and a source
port in 49152–65535. No seed, invalid seed or exhausted counter fails closed.
The egress guard permits only the pending encoded question to that exact resolver
and source port, once per attempt; ordinary clients gain no UDP authority.

Responses must match resolver address/port, transaction ID and complete A/IN
question. Parsing is capped at 512 bytes, 32 records, four CNAME hops, 64-byte
internal names, 16 compression-pointer hops and four distinct IPv4 answers.
Only answer-section records on the original name's alias chain contribute.
Truncated replies fail explicitly; TCP fallback and alias-only follow-up queries
are unsupported. CNAME traversal requires the terminal A RRset in the same
recursive response. NXDOMAIN, malformed replies and exhausted bounded retries
are terminal failures, never a compiled-in-address fallback.

Resolution lifetime is the minimum chain/RRset TTL, capped at 60 seconds; zero
TTL is refused. Every pre-establishment attempt checks expiry and the 15-second
total DNS/connect deadline. Up to two DNS retries use two-second timers; each
of at most four address attempts has a two-second handshake bound. An established
connection keeps its selected tuple and returns to the ordinary TCP timeout; it
is not silently re-resolved or reconnected after application bytes are sent.

Every returned address must be public unicast under the conservative implementation
filter, which refuses private, loopback, link-local, shared-address, documentation,
benchmarking, multicast and reserved ranges. A controlled resolver on port 1053
may return a non-public address only when the **same client** also has an exact
numeric CONNECT grant for that address and port. A mixed forbidden answer set is
refused rather than partially trusted. The public composition has no numeric
exception. DNS offers no cryptographic server authentication; neither this policy
nor QEMU NAT qualifies physical networking or TLS.

## Results

The IO0 queue status describes admission and settlement. The network completion's
`status_detail` describes the network operation. Neither replaces the other.
The schema defines success (`0`), denied (`-1`), malformed (`-2`), unsupported
(`-3`), would-block (`-4`), exhausted (`-5`), refused (`-6`), timeout (`-7`), and
reset (`-8`). A receive with `FLAG_END_OF_STREAM` is orderly EOF, distinct from
reset and from an empty nonblocking receive. A successful connect produces a
session-owned typed TCP connection capability; errors carry no capability.

## TCP engine bounds and operation semantics

These are implementation semantics of
[`tcp.rs`](../../components/services/network-service/src/tcp.rs). The host harness in
[`verification/network-tcp`](../../verification/network-tcp/Cargo.toml) compiles
that same source and exercises real smoltcp peers. `just io_tcp_check` additionally
observes the transmitted and received bytes, TCP handshake and FIN close through
the QEMU userspace driver. `just io_local_check` exercises local TCP without a
NIC, including connect/listen/accept, duplex bytes, EOF, coalesced readiness,
authority refusals and a typed receive timeout followed by resumed traffic.
`just io_network_lifetime_check` has observed client VM fault, session reclamation,
peer reset and supervised same-boot restart. In AArch64 QEMU,
`just io_network_service_fault_check` has observed an actual service VM fault with
payload work pending, root reclamation, client VM faults on revoked mappings,
and supervised fresh-incarnation recovery with bytes and EOF. Both restart paths
reject predecessor handles. `just io_network_driver_reset_check` separately
observes external-link reset with application receive work pending, typed reset
completion, root reclamation and supervised fresh-session recovery. The aggregate
entry point is `just io_network_qualification_check`; none of these results
qualifies physical networking.

The reset gate's controlled frame peer acknowledges the first 1024 actual payload
bytes but deliberately withholds echo. On a fresh SYN/initial sequence number it
discards that first server session, then verifies the second 4096-byte exchange
and FIN closure. The first session is abandoned, not FIN-closed or reset by the
peer; no transparent TCP continuation or ordinary-server reset interoperability
is claimed. Driver and root evidence, not that fixture behavior, establish actual
device reset, reclamation and epoch advance.

A LinkDevice reset must first consume and settle valid published IO0 submissions,
including RX work not yet present in its admitted-request table. Each such identity
receives one terminal result; admitted work is settled separately. The gate has
observed a queued RX submission on this path. Link queue epochs derive from the
root device incarnation rather than restarting at a constant value. The host
harness also exercises the production queued-request settlement helper.

External and loopback backends each own four fixed socket slots in separate
engines. Their socket-buffer arrays are static service-owned storage. Each socket
has a 2048-byte receive buffer and a 2048-byte transmit buffer. Admission checks the exact holder,
transport, IPv4 address, port, and connect right before changing a socket. Each
live connection reserves 4096 bytes against its destination's `byte_budget`,
one unit against `socket_limit`, one unit against `queue_depth`, and one timer
against `timer_budget`. These are conservative per-connection reservations, not
per-packet allocations. Send and receive independently require their declared
rights. Loopback listeners use separate application-authority rows, require the
exact local endpoint and admitted peer, and return typed listener/accepted
connection handles. Only backlog one is supported and admitted: one pending
handshake/accept slot, separate from already accepted children. A successful
accept rearms that slot when the accepted-child, byte, timer, queue and fixed-pool
limits permit; multiple live children of one listener are allowed. The bounded
service-owned DNS path above is separate from the TCP pools; general application
UDP, IPv6 and arbitrary external listener transport remain unsupported.

The destination byte reservation covers the socket buffers only. The separate
application ring and payload page, link frame pages, and fixed engine metadata
are not included in it; their backing storage is bounded by the composition's
service resources. A destination byte limit is therefore not an aggregate
network-memory accounting claim. Closing or failing a connect releases its
live-handle reservations, but a socket in TCP TIME-WAIT remains quarantined in
the fixed pool until smoltcp closes it. Such sockets can temporarily exhaust the
pool even when the live-handle count is zero. Source-port selection wraps within
49152–65535 and skips every port retained at either end of a socket tuple, in a
pending reset, or by a listener. This includes TIME-WAIT and a loopback peer that
outlives its client. Reuse is allowed only after those references disappear; a
failed connect does not advance the cursor. One holder cannot permanently consume
the ephemeral range by repeated failed requests. Fixed-pool limits still apply.

Connect and close retain pending operation state while the service continues
polling the interface. The engine itself transfers an available prefix or returns
`would-block`. For notification-enabled profiles, the application adapter retains
a validated descriptor for blocking send/receive/accept and retries until ready
or a four-second deadline; `FLAG_NONBLOCKING` requests return immediately. No Rust
reference into application payload memory is retained across dispatch calls. A successful send reports bytes queued in the socket, not
remote delivery. Receive reports the actual copied length and sets EOF only
after an orderly peer FIN and drainage of queued receive bytes. Applications
must handle partial transfers and distinguish EOF from temporary unreadiness.

Connect and close have ten-second deadlines. smoltcp also has a ten-second
socket timeout for an unanswered connect or outstanding transmit data. Explicit
engine deadlines and retry-budget exhaustion report `timeout`; after an
established socket closes internally, smoltcp's public state does not expose
whether a reset or its internal timeout caused the closure, so the engine can
report `reset`. A pending connect that closes before its deadline reports
`refused`.

Before publishing a TCP frame, the egress guard charges each segment whose
sequence range begins below the previous transmitted high-water mark as one
retry. The destination's `retry_limit` bounds these repeated segments over the
whole connection lifetime, including SYN, payload, and FIN retransmissions.
This is more conservative than a per-segment retry limit: legitimate overlap
can consume the budget. Pure acknowledgments do not consume retries. The engine
performs no automatic reconnects. The destination and application
`reconnect_limit` fields bound automatic recovery attempts, not explicit
application `OP_CONNECT` requests or listener rearming after accept; this engine
uses zero such attempts even when a nonzero limit is declared. Unknown-tuple TCP resets are not published;
TIME-WAIT sockets may still acknowledge duplicate FINs for their retained tuple.

TCP egress has holder-bound connection accounting; DNS UDP egress has the
separate exact pending-query guard described above. ARP neighbor traffic and smoltcp
IPv4 ICMP responses (including automatic echo replies) are interface-owned
control traffic, admitted without a holder tuple or destination byte/retry/timer
charge. An ICMP reply can therefore be emitted with no application connection.
The fixed link buffers bound storage, not a per-holder control-traffic rate; no
claim of all-frame holder authorization or ICMP/ARP rate limiting is made.

Close success means local handle disposal, not a certificate of graceful remote
delivery. An active close normally reaches TIME-WAIT; unexpected closure before
that state reports `reset`. For a peer-first FIN, smoltcp's public state cannot
distinguish a later reset from the final acknowledgment, so passive-close success
still means local disposal only. Successful close preserves TIME-WAIT rather
than aborting the socket. A gate claiming a graceful exchange must independently
observe both FINs and the final acknowledgment. Closing a connection that
already ended in `timeout` reports `timeout` again and still disposes of the
handle, returning its socket, buffer, work-slot and timer reservations.

Connection capabilities are checked against their exact holder and live engine
metadata. Checked packing combines service incarnation, backend and monotonically
allocated serial; exhaustion refuses instead of wrapping. Incarnation is advanced
by the service's explicitly authorized self lifecycle parameter before application
activation. Root retains that parameter over a same-boot service restart; it is
neither a timestamp nor cross-boot persistent storage. Released identities are
rejected. Same-boot restart, raw-send refusal of predecessor handles, fresh
identities and fresh exchange have been observed in the client-death and
service-fault QEMU profiles.

The service drains ready rings and stack work before sleeping. Declared profiles
register notifications in existing wait sets and arm the earlier of a stack
poll deadline and a 10 ms control fallback; notification badges do not count
requests. The readiness client bounds completion waiting at five seconds;
supervision in qualified compositions observes task death separately.
Connect/close engine deadlines remain ten seconds,
so client liveness bounds can terminate a session sooner than the engine deadline.

Socket, byte and timer limits are admission ceilings, not simultaneous capacity
guarantees. Timer accounting is a conservative reservation per live transport
object, not a measurement of smoltcp's internal timer queue. TIME-WAIT socket
quarantine can outlive a disposed handle. Cleanup observations must distinguish
live handles, retained sockets, byte reservations, pending request settlement and
shared-buffer release; a single zero counter does not establish all of them.
