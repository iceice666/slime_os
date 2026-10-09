# Zenoh Profile 0 transport

The bounded Zenoh subset two ROS 2 demo nodes use to exchange one topic: the
wire codec, a pure session machine, a runtime that carries a session over a byte
stream, and the two node components that run it on `sel4-zenoh`. It is a
userspace consumer of the [network service](network-service.md): the service
carries bytes and parses nothing, and every Zenoh concept lives in these
modules.

Owners:

- wire vocabulary and bounds: `contracts/zenoh-profile/v1/` (generated
  `components/proto/src/zenoh_profile.rs`); the wire records themselves belong
  to eclipse-zenoh wire version `0x09` and are not declared here;
- decoder and `StreamFramer`: `components/lib/src/zenoh_profile0.rs`; encoder:
  `zenoh_profile0/encode.rs`; session machine: `zenoh_profile0/session.rs`;
- classic CDR for `Counter`: `components/lib/src/ros_cdr.rs`;
- transport runtime: `components/lib/src/zenoh_link.rs`;
- the nodes' shared exchange logic: `components/lib/src/zenoh_node.rs`;
- nodes: `components/applications/ros2-demo-publisher` and
  `components/applications/ros2-demo-subscriber`;
- composition: `contracts/system-spec/v1/systems/sel4-zenoh.zti` (generation 168),
  from which `contracts/generation-manifest/v1/compositions/sel4-zenoh.zti` is
  derived;
- the demo it serves: [`contracts/rpi5-ros2-demo/v2`](../../contracts/rpi5-ros2-demo/v2/README.md);
- plan and decision: [`../plans/ros2-wire-compatibility.md`](../plans/ros2-wire-compatibility.md),
  [`../decisions/zenoh-profile0-transport.md`](../decisions/zenoh-profile0-transport.md).

## Boundary

Profile 0 admits one static peer link over one TCP connection: the
`INIT`/`OPEN` handshake as connector or listener, reliable `FRAME` batches
carrying `DECLARE_SUBSCRIBER`, `UNDECLARE_SUBSCRIBER` and a `PUSH` with a `Put`
body and the 33-byte `rmw_zenoh` attachment, and `CLOSE`. Everything else is
refused with a class from the closed vocabulary in `contracts/zenoh-profile/v1`,
never tolerated: routers and clients, wildcard key expressions, other
resolutions, unknown extensions and messages.

Bounds are enforced before any byte is taken and equal the demo contract's: a
512-byte batch, 129-byte key expression, 33-byte attachment, 12-byte payload,
64-byte cookie, one subscriber, four outstanding samples and three retries.
Nothing in these modules allocates.

## Layers

| Layer | What it owns | What it does not own |
| --- | --- | --- |
| Decoder, encoder | Bytes to and from `Message`; every length checked against its bound first; decoded values borrow from the batch | Session state, time, the link |
| Session machine | Handshake, cookie, frame sequence numbers, lease, subscriber declaration and delivery; time is an argument | I/O, allocation, any clock capability |
| `Link` over `ByteLink` | One framed batch awaiting send, partial writes, backpressure, the retry limit, reassembly, notices, generation-checked handles | A socket or a capability: the node passes it the connection the generation granted |
| Node exchange (`zenoh_node`) | The publisher and subscriber control flow as steppable state machines, the console lines, the service-reply-to-error mapping, the denied requests | Bindings, the connection, the console: those are the component's |
| Node components | Their bindings, the service connection, the clock and the console, handed to `zenoh_node` through two small traits | Any control flow worth testing |

The session keeps two subscriber slots. The *local* slot is the subscription
this side declared; a received `PUSH` is delivered only to its key. The *peer*
slot is the subscription the other side declared; a `Put` this side sends must
name its key. A publisher therefore learns the subscriber's key from its
declaration before it may publish, and a subscriber never accepts a `Put` for a
key it did not declare. Frames the session sends use the sender key mapping,
as the upstream encoder writes them.

There is no keep-alive: the decoder refuses message id 4, so the lease is renewed
only by traffic, and a link quiet for longer than the peer's lease is closed by
`tick` with the expiry reason. The demo exchanges its samples well inside the 2 s
lease.

A refused call leaves the session unchanged. The only transitions into
`Closed` are a received `CLOSE`, a local `close()` and lease expiry; a caller
that wants the link gone after a refusal says so.

## Link behaviour

- `Link::pump` flushes the pending send, reads, decodes and replies, then checks
  the lease. A send that accepts nothing counts as a stall; more than
  `MAX_RETRIES` consecutive stalls end the link with `RetryLimit`, and any
  progress resets the count.
- Reassembly takes a length prefix as soon as both bytes arrive, so a zero or
  oversized prefix is refused before its body. A received batch is never dropped:
  while the send slot or the notice queue is full the batch stays in the
  framer, and a full buffer with no complete batch is `ReceiveQueueFull`.
- A pass that raised notices ends there, so `last_received_batch` names the batch
  behind the notices the owner is about to take; `last_sent_batch` names the
  batch most recently queued. Nodes print both as evidence.
- A `LinkHandle` carries the generation of the link that issued it. A restart
  builds a new link under the next generation, and an operation through an old
  handle is `StaleHandle`.

## Node exchange

The nodes are thin. Everything that decides what a node does lives in
`components/lib/src/zenoh_node.rs`, which builds on the host: `Publisher` and
`Subscriber` are steppable state machines over a `Link`, and a component supplies
only a `Host` (clock, yield, console) and a `Connection` (non-blocking send and
receive on one service connection). The host tests run the real publisher and
subscriber against each other over a scripted stream, so the order a node acts in
and every line it prints are judged without QEMU.

`Line` collects a console line in a fixed buffer and emits it by a single write. A
line that does not fit is refused whole and never truncated, so a clipped hex line
cannot reach the judge. The demo's frames are 184 bytes (368 hex digits); a full
512-byte batch would clip and is refused.

## Node authority

`sel4-zenoh` contains `init`, `network-service` and the two nodes over the
loopback backend. The publisher holds one destination, exact IPv4 TCP to
`127.0.0.1:7447` with `connect`, `send` and `recv` and no listener or DNS
authority. The subscriber is the one loopback listener on that endpoint with
`listen`, `send` and `recv`, admitting exactly the publisher's holder. Each
node attempts requests it was not granted, checks that each reply is a denial
(status `denied`, nothing moved, no capability), and only then reports it by name.
The publisher attempts an undeclared host, a port the endpoint does not name, a
listen, and a *scouting* request: a well-formed UDP connect to Zenoh's multicast
scouting group, which no destination row names. The subscriber attempts a connect.
Profile 0 has no scouting message, so the denial is the service refusing the
request, not the codec; a refusal of a malformed request would prove nothing about
authority.
Each console line is written in one `debug_write`, because the two nodes run
concurrently and a line built from several writes can be interleaved.

## Limitations

- The exchange crosses the loopback backend, so there is no packet capture. The
  nodes' reported batches observe their codecs and session at the service's
  application boundary, not the stack's wire, and the guest chooses what it
  prints.
- The exchange crosses the loopback backend, so the arm's evidence is the nodes'
  own reports at the service boundary. Under QEMU the corrected exam accepts the
  booted guest; see the plan for what was observed and on which image.
- A Zenoh ID ending in `0x00`, or all zero, is refused: upstream holds it as a
  little-endian integer, drops trailing zero bytes and refuses zero, so the bytes
  would not round-trip.
- No `rmw_zenoh` interoperability, liveliness, queryables, router, scouting or
  transport security, and no Raspberry Pi 5 claim. The ROSIDL importer and RIHS01
  generator are not here: the demo contract's checker derives those values.

## Verification

- `just zenoh_encoder_check` — all 17 upstream-encoded corpus batches
  reproduced byte for byte, after the host reference `scripts/lib/zenoh_wire.py`
  reproduces them; 7 named refusals;
- `just zenoh_session_check` — 5 scenarios and 7 refusals;
- `just zenoh_cdr_check` — the demo fixture's 4 samples and 7 malformed inputs;
- `just zenoh_transport_check` — 6 scenarios and 7 refusals over a scripted link;
- `just zenoh_composition_check` — 15 authority facts on the derived manifest;
- `just zenoh_xcheck_check` — the codec and session against the real eclipse-zenoh 1.0.0
  crates: the 17 corpus batches as upstream's encoder produces them, a 6000-case sweep
  (5993 compared, 7 refused as a non-canonical Zenoh ID, none differing), our decoder
  reading what upstream encodes, upstream reading and re-encoding ours, one 11-batch
  session, and eight Zenoh ID probes; 17 mutations of an honest transcript refused first.
  The harness is `verification/zenoh-xcheck`, a standalone crate outside the root
  workspace, so the upstream crates stay out of `Cargo.lock`, `just deny` and the
  closures. It fetches them from a registry, so the recipe is explicit and not in CI;
  `cargo deny` run on that crate reports `paste` as unmaintained (RUSTSEC-2024-0436) and
  Zlib-licensed transitive crates, which the root `deny.toml` does not allow;
- `just rpi5_ros2_zenoh_check` — the QEMU exchange; explicit, not part of `all`.
