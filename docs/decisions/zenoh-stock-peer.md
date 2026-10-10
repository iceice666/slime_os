# Zenoh Profile 0 and a stock Zenoh peer

**Status:** Proposed
**Related work items:** `01a125c9-b193-7ab7-8ea0-43bbac09e946` (this decision's exam and
implementation); Zenoh Profile 0 epic `01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`; R1
`01a0724c-5400-7206-ae86-ad13584c8530` (deferred). R0 is `01a00b4d-2400-7987-93d6-19201c397dff`.

## Context

[Zenoh Profile 0](zenoh-profile0-transport.md) was designed against a decoder corpus
the upstream encoder produced, and it is checked against the real upstream codec
(`verification/zenoh-xcheck`). Neither shows what a Zenoh peer does on a link. The
transport decision named that gap: R1 "needs a capture of a real link".

A stock `eclipse-zenoh==1.0.0` peer (the Python wheel, `mode=peer`, scouting and
gossip off, loopback TCP) was run against a listener and a connector built on this
repository's encoder. What it sent is in `scripts/lib/zenoh_stock_peer.py`, with the
wheel hash and the way each batch was obtained.

## What was observed

Of the 19 batches the peer sent while connecting or listening, Profile 0's decoder
takes 5. The rest are refused for four reasons:

| The peer sends | Profile 0 today |
| --- | --- |
| an optional QoS unit extension (id 1) on `INIT_SYN`, and a per-message QoS extension (id 1, priority, congestion, express) on every `OAM` and `DECLARE` inside a `FRAME`; its `PUSH` and `Put` carry none | `unsupported-extension` |
| an `OAM` and a `DeclareFinal` straight after `OPEN` | `unsupported-message` |
| the demo key as a numeric alias (`DeclareKeyExpr`), then `DeclareSubscriber` and `Put` addressed through that alias, with an empty suffix | `unsupported-extension`, then `invalid-keyexpr` |
| a one-byte `KEEP_ALIVE` every quarter of its lease (every 2.5 s on 10 s, every 0.5 s on 2 s) | `unsupported-message` |

What the peer accepted from our encoder: our `INIT_ACK` and `OPEN_ACK` completed the
handshake; our `DeclareSubscriber` and `Put` frames with the 33-byte attachment were
delivered to its subscriber with the demo key, payload and attachment intact; and a
`Put` we sent to the peer's declared key arrived four of four times. A peer that
heard nothing from us closed the link one lease after our last transmission
(2.002 s on a 2 s lease, 10.002 s on a 10 s lease), and one we sent keep-alives to
every 0.7 s stayed connected for the whole 9 s run on a 2 s lease.

A stock peer in `client` or `router` mode sends an `INIT_SYN` with that role. The
decoder refuses it as `unsupported-whatami`, and those refusals are Profile 0's own and
stay. In the one run where our responder answered a router's `INIT_SYN` using the
upstream codec to read it, the router completed the handshake and reported one peer, so
the refusal is ours and does not come from the peer.

## Decision

Profile 0 takes a stock peer for the plain demo topic, and nothing wider. The session
and link, not the node components, change:

1. **QoS extensions.** Two optional, non-mandatory id 1 extensions are accepted and
   ignored, and only where they were observed: the unit extension on `INIT_SYN`, and the
   per-message extension on the `OAM` and `DECLARE` messages inside a `FRAME`. A `PUSH` carried none: its header is `0x5d`, and the `QoS` that the upstream decoder prints for it is the default it fills in. Any other extension,
   and either QoS extension marked mandatory, stays `unsupported-extension`. No
   extension is accepted on `INIT_ACK`, `OPEN_SYN` or `OPEN_ACK`: the stock peer sent
   none there, so there is nothing to replay and no claim.
2. **OAM and DeclareFinal.** Accepted and ignored after `OPEN`. Nothing is sent in
   reply, because the peer needed no reply in any run.
3. **Key-expression alias.** One alias, declared once with `DeclareKeyExpr`, resolves
   a `DeclareSubscriber` or a `Put` to the declared key. A second alias is `over-bound`,
   an undeclared alias or a redeclaration to another key is `invalid-keyexpr`. The
   key itself keeps every bound it has.
4. **Keep-alive.** An open session sends `KEEP_ALIVE` every quarter of the peer's
   announced lease when it has sent nothing else, accepts the peer's as traffic, and
   closes with the expiry reason after one lease of silence.

Everything else the profile refuses stays refused with its class.

## Alternatives

- **Capture an `rmw_zenoh` peer instead.** It is what R1 targets, but it needs the
  `@ros2_lv` liveliness tokens, and the 1.0.0 Python binding cannot declare one, so
  no token was observed. A plain topic over a stock peer is the part that was.
- **Write a new profile.** The four changes are small and bounded, and a second profile
  would duplicate the decoder. They are added to Profile 0 under the same exam.
- **A text hand-off to the Rust tests.** The checker gives the tests the captured bytes
  through a file. A bespoke `<name> <hex>` format would be a second serialized format
  crossing a process boundary, so it is a versioned contract,
  `contracts/zenoh-stock-fixtures/v1`, validated before the tests read it.
- **Fix the node components.** The refusals are in the decoder and session; the nodes
  only see their notices.
- **Leave the profile closed and write a host adaptor.** An adaptor in front of the
  guest would have to speak the same stock behaviour, and would hide it from the exam.

## Consequences

- A stock Zenoh 1.0 peer can complete a session with the Profile 0 session and link
  and exchange the demo topic, as listener or connector, in a replay of captured bytes.
- Profile 0 is no longer "refuses what it does not implement" for these four things.
  Of the 102 refused vectors in `vectors.txt`, one may change: `transport_keep_alive`, the
  single byte `0x04`, which the stock peer sent four times. It may be removed or kept as
  an accepted vector. The other 101 keep their name, class and bytes. I had first allowed
  nine, on the strength of their names, and checking them byte for byte against the capture
  showed that eight do not match anything the stock peer sent: the existing vectors for an
  extension flag, an alias, a scope, a suffix and an `OAM` are different shapes from the
  stock peer's (it attaches a QoS extension and a body they lack). The implementation
  accepts the stock shapes, and the 19 replays prove it; it may add a positive vector for
  a captured batch under a `stock_` name when its bytes are exactly that batch's. Every
  extension position no captured batch used (a frame header, a subscriber declaration, a
  Put header) and the undeclare forms stay refused. The 17 accepted batches keep their name, bytes and summary, and so do the 14
  stream-framing rows (ten accepted and four refused), which carry the 512-byte stream
  bound and the reassembly rules and none of which a stock peer exercised. The exam pins
  the lists and a digest of each in `scripts/check/check-zenoh-profile0.py`, so the
  implementation cannot widen the profile by editing the corpus it is judged against. The
  digests prove the corpus text is unchanged, not that the decoder still agrees with it, so
  the recipe also runs `zenoh_profile0::tests::corpus_agrees` and requires it to pass.
- The closure digests change with the Rust change, as with any product edit.

## Limits

- The QoS extension was seen only where it is listed above. The stock peer's `INIT_ACK`
  carried none, but our `INIT_SYN` carried none either, so whether it would attach one
  when answering a QoS-bearing `INIT_SYN` was not tested.
- One peer, one wheel, one machine, one run each. The batch bytes replay
  deterministically; the peer's behaviour was not measured beyond what is stated here.
- Python binding only. It wraps the Rust implementation, but `rmw_zenoh` and its
  liveliness tokens were not run.
- The exchange never leaves the machine: no QEMU, no NIC, no external listener. Reaching
  a service in the guest from outside needs the virtio-net backend, an external
  listener row and a host-reachable port, and is a separate item.
- Whether a keep-alive cadence slower than 0.7 s on a 2 s lease also keeps the peer
  connected was not measured; the item requires one every quarter of the lease, the
  cadence the peer itself uses.

## Revisit when

- the external backend carries a Zenoh session, which would let the capture be taken
  across a NIC;
- liveliness tokens are captured from an `rmw_zenoh` peer, which is R1.

## References

- [`zenoh-profile0-transport.md`](zenoh-profile0-transport.md)
- [`../../contracts/zenoh-stock-fixtures/README.md`](../../contracts/zenoh-stock-fixtures/README.md)
- [`../architecture/zenoh-transport.md`](../architecture/zenoh-transport.md)
- [`../plans/ros2-wire-compatibility.md`](../plans/ros2-wire-compatibility.md)
- [`../../roadmap/03-ros2-compatibility.md`](../../roadmap/03-ros2-compatibility.md#r1-broader-ros-2-topic-wire-profile)
