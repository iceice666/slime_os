# ROS 2 wire compatibility

## Goal

Qualify bounded ROS 2 wire interoperability without implying support beyond the
admitted profile. Work-item state lives in `.tasks/items/`.

## Release boundary

R0 defines the minimum topic path; R1/R2 broaden it to external `rmw_zenoh`
peers, services, and actions. Transport-level security is not claimed by
R0/R1/R2 unless separately admitted and verified.

## R0: Zenoh Profile 0 transport

**Canonical work items:** epic `01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`, with
slices encoder `01a0f5fa-f704-73e0-b3a1-cd2debc429e9`, session machine
`01a0f5fa-fb06-7cbd-9a39-1539785433b7`, classic-CDR codec
`01a0f5fa-ff13-7667-8c59-6f4c6d312535`, transport runtime
`01a0f5fb-0344-7bb5-9d5b-2b79bc0339a8`, nodes and composition
`01a0f5fb-0780-754f-b2f4-e7513c86b63c`, and QEMU gate
`01a0fc73-3bcd-75c7-9ec5-0e9f84c7505b`. R0 itself,
`01a00b4d-2400-7987-93d6-19201c397dff`, is a pre-cutoff prose item and keeps its
record; the epic owns the remaining work because only a spec-driven item can bind
evidence. R0 closes by hand once the epic's acceptance holds. The design and its
alternatives are in
[the decision record](../decisions/zenoh-profile0-transport.md).

### Present state

The wire codec, session machine, CDR codec, transport runtime, the two node
components and the `sel4-zenoh` composition exist; [the architecture
page](../architecture/zenoh-transport.md) owns how they work. The
[format-2 demo contract](../../contracts/rpi5-ros2-demo/v2/README.md) fixes the
topic, the `Counter` type, the key expression, the 33-byte attachment, the
bounds and the four sample byte strings.

Observed under AArch64 QEMU: generation 168 is admitted with four instances,
four samples cross one session, the batches the nodes report are byte-identical
on both sides and equal the host reference's encoding of their own fields, and
the graph ends `HEALTHY` with four required instances completed. The first exam
landed with this plan ordered the two concurrent nodes against each other and
refused that guest; the corrected exam (a chain per node plus one crossing chain)
accepts it. See [Boundaries](#boundaries) for what is and is not observed.

An earlier boot of the same sources stopped at `attach`: a non-blocking receive
that found nothing was returned as a zero-byte message, and the attach handshake
rejected it as malformed. That was a defect in the runtime's receive path, not in
the Zenoh code, and it is fixed separately; the existing `--arm local` network
plane check, which carries no Zenoh code, failed the same way until then.

### What was built

| Slice | Implementation | Recipe |
| --- | --- | --- |
| Encoder | INIT, OPEN, FRAME, D/U_SUBSCRIBER, PUSH+Put with the 33-byte attachment, CLOSE and the stream length, into fixed storage, in `components/lib/src/zenoh_profile0*` | `zenoh_encoder_check` |
| Session machine | A pure state machine: handshake as connector and listener, lease, frame sequence numbers, declare, deliver, undeclare, close; time injected | `zenoh_session_check` |
| CDR codec | `components/lib/src/ros_cdr*`: classic CDR_LE for `Counter`, encoder and decoder | `zenoh_cdr_check` |
| Transport runtime | `components/lib/src/zenoh_link*`: the session over a byte-stream trait, with reassembly, partial writes, bounded queues and a retry limit | `zenoh_transport_check` |
| Nodes and composition | `ros2-demo-publisher` and `ros2-demo-subscriber` under `components/applications`, and the `sel4-zenoh` system spec (generation 168) with its derived manifest and closures | `zenoh_composition_check` |
| QEMU arm | The `zenoh` arm of `check-sel4-io-network-plane.py` and its 36 markers | `rpi5_ros2_zenoh_check` |

### How each slice is verified

None of these recipes is in CI or an aggregate. The first column is what the
recipe requires; the second is why a wrong implementation cannot pass. The host
recipes and `zenoh_composition_check` pass, and `rpi5_ros2_zenoh_check` passes on a
booted `sel4-zenoh` image.

| Slice | The recipe requires | Why a wrong implementation fails |
| --- | --- | --- |
| Encoder | All 17 accepted corpus batches reproduced byte for byte; 7 named refusals | The batches come from the upstream eclipse-zenoh 1.0.0 encoder. `scripts/lib/zenoh_wire.py` re-derives each from its summary first, so the corpus, the reference and the Rust encoder must all agree |
| Session | 5 named scenarios and 7 named refusals, exactly once each | The names are the checker's; a missing, extra or repeated name fails |
| CDR | 4 samples whose bytes equal the demo fixture's `cdrHex` and bytes the checker packs from the field values; 7 malformed inputs | A fixture edited to match a wrong codec still disagrees with the packed bytes |
| Transport | 6 scenarios and 7 refusals over a scripted byte link, with the Rust filter `zenoh_link::` | No QEMU is needed, and a runtime under another path runs no test and fails |
| Composition | 15 authority facts on the derived manifest, after 15 refused mutations | Another port or address, a missing right, listen or resolver authority, a wildcard listener, an unadmitted peer, a UDP destination and an external backend are each refused |
| QEMU arm | The composition judge, the four host recipes, 36 markers in six chains (a chain per node and one crossing between them), then 4 batches (736 bytes) judged against the reference | Each reported batch must equal the reference's encoding of its own decoded fields, with the demo key, the contract's payload, an attachment sequence rising by one and one GID; 28 transcript mutations are refused first |

Each Rust slice prints one `[zenoh-exam]` line per case it judges.
`scripts/check/check-zenoh-profile0.py` owns the expectation and never the tests:
it requires exactly the names it lists and compares reported bytes with bytes from
an independent source. Before it trusts a judge it refuses corrupted evidence
(8, 6, 8 and 6 mutated transcripts for the encoder, session, CDR and transport,
15 for the composition and 22 for the exchange) and reports the counts to the
`just-observations` gate.

The QEMU arm's 36 markers join the network plane's pinned gate-control count
(193 to 229), so `just sel4_gate_control_check` applies its missing, reordered and
failure-marker mutations to them.

### Traceability to R0's verification list

| R0 verification | Observed by |
| --- | --- |
| Publisher and subscriber create only their declared session, topic and direction | `zenoh_composition_check`'s authority facts and the arm's four named denials, each reported only after the reply was checked to be a denial |
| Alternate domain, key, endpoint, port or direction fails | The composition mutations and the transcript's wrong-key and wildcard-key controls |
| Messages serialize to the pinned CDR bytes and cross the session with deterministic values | `zenoh_cdr_check` and the arm's byte-for-byte comparison |
| Malformed lengths, headers, LEB128, key expressions, attachments and CDR alignment fail before allocation | The decoder corpus (landed with the decoder), `zenoh_encoder_check`'s refusals, `zenoh_cdr_check` and `zenoh_transport_check` |
| A wildcard key, a router endpoint and a scouting attempt are rejected | The session refusals `wildcard-key-declaration` and `router-peer`, and the publisher's real scouting request (a UDP connect to Zenoh's multicast group) which the network service refuses; the arm requires the denial |
| Node restart issues fresh sessions and cannot replay stale samples | `zenoh_transport_check`'s `restart-uses-fresh-session`, which asserts the predecessor's frames are refused by sequence number and deliver nothing, and `stale-session-handle` |
| The same profile passes under AArch64 QEMU | `rpi5_ros2_zenoh_check` |

### Boundaries

- The ROSIDL importer and the RIHS01 type-hash generator are not slices. The demo
  contract's checker derives those values today, and this work consumes them as
  frozen constants.
- The exchange crosses the network service's loopback backend, so there is no
  packet capture. The arm judges the batches the nodes report handing to `send`
  and receiving from `recv`; it observes the node codecs and session, not the
  stack's own bytes.
- Nothing here is `rmw_zenoh` interoperability, liveliness, queryables, a router,
  scouting or transport security, and nothing here is a Raspberry Pi 5 claim.
- **The first exam was wrong, and booting the guest showed it.** Its marker chains
  ordered one node's lines against the other's, and its judge required the
  publisher's denials to follow its close, while the nodes run concurrently and
  report their denied requests during setup. No real transcript satisfied both
  halves. The corrected exam checks causal order only and landed as its own
  planning change; the bytes, values, teardown order and health census were never
  in question.
- **No keep-alive.** The decoder refuses message id 4, so only traffic renews the
  lease; a link quiet for longer than the peer's lease is closed. The demo is well
  inside the 2 s lease, and the behaviour is tested, not exercised on a quiet
  link under QEMU.
- The nodes' reported batches are judged against an independent reference, but
  the guest chooses what it prints, and the loopback backend gives no packet
  capture.

## Owning references

- [`../plans/rpi5-ros2-demo.md`](../plans/rpi5-ros2-demo.md)
- [`../../roadmap/03-ros2-compatibility.md`](../../roadmap/03-ros2-compatibility.md)
- `contracts/zenoh-profile/` (lands with the decoder)
