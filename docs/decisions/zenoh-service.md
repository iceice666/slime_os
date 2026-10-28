# A shared Zenoh service component

**Status:** Proposed
**Related work items:** `01a15d1f-1f52-7d62-806f-90ef3c119b59` (a session that holds several subscriptions of its own) and `01a15d1f-26d2-7653-924b-a3f113a718e6` (the grants contract). `01a15d1f-2e50-7f0f-9721-ce4b3b5f5aa8` is the service core, which depends on both. `01a17dad-f158-7efd-bea7-dfdf0288432b` is the core over a link. `01a17dad-fa21-739d-a55e-ef59fa3ce0ea` is the client protocol. `01a182bb-ad9c-7d12-9fcc-5a850932dd55` is the client wire records. `01a182bb-b5f5-7879-9340-997b1042b4e3` is the client adapter. Zenoh Profile 0 epic: `01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`.

## Context

Every Zenoh participant embeds the whole stack. `ros2-demo-publisher` and `ros2-demo-subscriber` each link the codec, the session and the link (`zenoh_profile0`, `zenoh_link`, `zenoh_node`). Each needs its own network authority. The publisher holds a destination row in `network-application/v3`, and the subscriber holds a listener row. Two nodes need two stacks and, over a real NIC, two TCP connections. A third component cannot publish or subscribe without its own stack and its own network right.

The [typed data fabric](../architecture/typed-data-fabric.md) already moves samples between local components. The owner decided that the Zenoh service does not depend on the fabric. Where the two overlap, the service may replace the fabric. This record plans no bridge to it.

## Decision

Add one component, the Zenoh service. It owns the Zenoh link and session and serves any number of local holders. The owner made these seven choices.

1. **The service owns the network authority.** It holds the destination rows for its peers. A holder that publishes or subscribes through it holds no network right.
2. **A grant is a row, and a key is a full Zenoh key.** [`zenoh-service-grants/v1`](../../contracts/zenoh-service-grants/README.md) lists rows. A row names a holder, one exact key, and the rights to publish and to subscribe. A key string in a request grants nothing. The service does not map a topic and a type to a key. A holder that wants an `rmw_zenoh` key states it in full.
3. **The record declares the limits.** The contract sets no holder, key or slot count. A record states them, and the contract checks them against the stack that the record also states. [`zenoh-link-limits/v1`](../../contracts/zenoh-link-limits/README.md) does the same for a link. Each fixed bound has a source: the 16-bit and 32-bit widths, a 65535-byte key and a 1048576-byte sample. Reference Zenoh behaves the same way. Its protocol caps no subscribers or resources. `max_sessions`, the queue sizes and `max_message_size` are configuration (`DEFAULT_CONFIG.json5`, 1.0.0).
4. **The service declares keys to the peer once the session is open.** It declares one subscription for each distinct key that some holder may subscribe to. The session must hold that many subscriptions of its own, and the contract checks it. This matches the static discovery that the demo contract fixes. The peer's view of the service does not change at run time.
5. **Delivery to a holder is best-effort with counted loss.** Each holder has its own ring of the declared slots. A sample that finds the ring full drops the oldest sample. The ring counts the loss, and the holder's next take reports it once. A holder that does not drain its ring never blocks another holder and never grows a queue.
6. **A publisher hears itself.** The service delivers a publication to every holder with a subscribe row on the key, the publisher included. In Zenoh 1.0.0, `Locality::Any` is the default for subscribers and publishers (`zenoh/src/api/sample.rs`). A session's own subscriber therefore receives its own put. A holder cannot exclude its own samples. A per-row flag, as `allowed_origin(Remote)` does, is a later decision.
7. **The client interface follows the network service.** The service has a nontransferable control endpoint and a provisioning endpoint. At attach it delegates a ring page and a data page as shared-buffer loans. A typed synchronous adapter in `components/lib` owns one outstanding request. The adapter poisons the session when a completion is missing. The service reuses the network service's attach, teardown and client-death handling as a pattern. See [`contracts/network-service/README.md`](../../contracts/network-service/README.md).

A host state machine over a session and a scripted peer judges the service core, as `zenoh_links` judges a link. The client interface and a booted composition come later.

## What the existing code did not support

- **The session held one subscription of its own.** `Session::send_declare_subscriber` refused with `subscriber-limit` when `local_n >= 1`. The table held `per_side + 1` slots (`components/lib/src/zenoh_profile0/session.rs`). #322 let a link configuration raise the peer's subscriptions and keys. It did not raise this side's. A probe on the unmodified session accepted the first declaration and refused the second, on another key. Item 1 lifts the limit.
- **A delivered sample does not name the subscription that matched.** `Event::SampleDelivered` carries the sequence, the timestamp and the payload. The core routes a peer's sample by the matching subscription. Item 3 adds that id to the event.
- **A send needs a peer subscriber.** `send_put` refuses with `undeclared-key` unless the peer declared a subscriber for the key. The core does not pass that on as an error. The local copies of a publication still arrive. The core reports the peer's copy as not subscribed.
- **No contract described per-holder keys.** Item 2 adds one.

## Alternatives considered

- **A fabric participant that bridges routes to Zenoh keys.** It reuses local fan-out, loans and QoS. The owner rejected it, because the service must not depend on the fabric.
- **Each node keeps its own stack.** This needs no work. It costs a stack and a network right per participant. `sel4-zenoh` and `sel4-zenoh-external` keep it, and this decision does not change them.
- **Key rows inside `network-application`.** This needs fewer contracts. It edits a shipped format and puts two authorities in one record.
- **A topic and a type instead of a full key.** A holder would not see Zenoh. The service would own the name mangling and the RIHS01 type hash, and the exam would judge both.
- **Lazy declaration on the first subscribe.** This is more flexible. The peer's view changes at run time, and the exam needs ordering cases for declare, undeclare and reconnect.
- **Reliable delivery with credits.** It loses no sample. A stalled holder can back-pressure the shared link. That is interference between holders, which the capability model prevents.
- **A fixed number of holders and keys, as in the four application rows of the network service.** That is a limit of that format and not a rule. See choice 3.
- **No echo to the publisher.** It departs from Zenoh, and a holder that wants the echo has no option.
- **Plain endpoint messages with no ring.** This is simpler. A 64-byte message limit needs a second path for larger samples.

## Work breakdown

The planning PR that adds this record also adds items 1 to 3. Each item has an exam. A throwaway implementation produced the counts, and a revert removed it afterwards.

1. **A session that holds several subscriptions of its own** (`01a15d1f-1f52-7d62-806f-90ef3c119b59`). `Params` gains `local`, and the table holds `per_side + local` slots. `Params::DEMO` and `Params::from_limits` keep `local` at 1. No link built from `zenoh-link-limits/v1` and no landed exam changes. The slice `--slice subscribers` pins 28 cases and 90 controls. It runs the links exam and the stock-peer exam beside it. A link configuration that declares more than one subscription needs a new version of the link-limits contract and re-admits the links item. This item does not do that.
2. **The grants contract** (`01a15d1f-26d2-7653-924b-a3f113a718e6`). `contracts/zenoh-service-grants/v1`, judged through its own decoder. The slice `--slice grants` pins 68 cases and 77 controls. Ten of the controls are weakened contracts that must each take the record that the removed check refuses. Four more make an upper bound exclusive, and the weakened contract must then refuse the record at that bound. Nineteen more are binding files that differ from the rendered text by one edit. Four of those are read from disk as bytes, so a newline that a text read would translate is still seen. The exam renders the Rust constants from the schema and requires the committed file to equal that text byte for byte. Earlier drafts used a scanner and then an allowlist. A constant could hide behind an attribute, a comment or a raw string in both. The check therefore compares whole files. This planning PR lands the script `scripts/generate/generate-zenoh-service-grants-bindings.py` and the recipe `just zenoh_service_grants_gen`, because the implementation PR cannot add or change a script that grades it. The implementation owns the generated Rust file.
3. **The service core** (`01a15d1f-2e50-7f0f-9721-ce4b3b5f5aa8`). It follows items 1 and 2. The slice `--slice service` pins 59 cases and 155 controls. The item adds the matching subscription's id to `SampleDelivered`. The Rust test also prints the `size_of` of a slot, a row, a holder and the core. The judge refuses a size above the matching overhead constant in the grants contract, so the declared stack covers what the core stores. The test also reads the nine constants from `slime_proto::zenoh_service_grants` and prints them, and the judge compares each with the schema. A crate that does not declare that module does not compile the test, so the exam cannot pass without the module line in `components/proto/src/lib.rs`. A text check of that line would have to rule out a comment, a `cfg` attribute and a macro. A compile is the sound check.
4. **The core over a link** (`01a17dad-f158-7efd-bea7-dfdf0288432b`). A component owns a link, and the link owns the session, one send slot, retries and keep-alives. The core of item 3 borrows a bare session, so a component cannot use it. This item adds a layer beside the core and leaves the core and its exam as merged. The slice `--slice service-link` pins 61 cases and 151 controls. The layer is described under [The core over a link](#the-core-over-a-link).

5. **The client protocol** (`01a17dad-fa21-739d-a55e-ef59fa3ce0ea`). `contracts/zenoh-service-client/v1`, judged as text against the IO0 and grants contracts. The slice `--slice client` pins 24 cases and 80 controls. The schema has no `gen_rust.zt`, because that file is product code. This planning PR therefore fixes constants and layouts only. See [The client protocol](#the-client-protocol).

6. **The wire records** (`01a182bb-ad9c-7d12-9fcc-5a850932dd55`). A generator script renders the request, completion, sample header and loan descriptor as Rust structs with `encode` and `decode`, from the layouts of `contracts/zenoh-service-client/v1`. It is a script and not a `gen_rust.zt` renderer, because that file is product code and a planning PR cannot carry it. The grants and client constants are rendered the same way. The slice `--slice client-wire` pins 17 cases and 58 controls. See [The wire records](#the-wire-records).
7. **The client adapter** (`01a182bb-b5f5-7879-9340-997b1042b4e3`). A synchronous adapter in `components/lib` that owns one outstanding request, bounds every request, checks every answer and poisons the session when a completion is missing. It follows `network_io.rs` and needs step 6, because it cannot hand-write field offsets. The slice `--slice client-io` pins 68 cases and 146 controls. See [The client adapter](#the-client-adapter).

A third planning PR is proposed for the steps below. Each needs a `slime-root` change and a booted composition, so none can be measured on the host alone.

8. **The grants path.** A resource kind and a read label in `slime-root` and `boot-contracts`, so the grants reach the service from the system spec.
9. **The service component.** It wraps the core over a link and serves the control and provisioning endpoints and the loans.
10. **A booted composition.** Two clients under QEMU.

## The core over a link

The layer adds four operations to the core. `declare_next_over` and `publish_over` queue one frame through the link. `settle` reads the link's send slot. `drain_notices` takes the link's notices and routes the samples.

- **One frame at a time.** The link has one send slot. The core holds one waiting declaration or one waiting publication. A call that finds either taken returns `Busy` and changes nothing.
- **A declaration counts after the flush.** Subscription `n` is declared when `settle` sees the send slot empty.
- **Local copies follow the flush.** A publication whose key the peer declared is queued. Its copies reach the rings when `settle` sees the send slot empty. A publication whose key the peer did not declare reaches the rings at once and is reported as not subscribed.
- **A sample names its subscription.** The link's `Sample` gains the id of the subscription that matched. The core routes by that id.
- **A burst is bounded.** The exam link holds eight notices. A burst of 20 that the service does not drain left eight notices queued and the rest in the stream. After the service drained everything, the ring held four samples and counted 16 lost.

The guarantee is that the link handed the frame to the byte stream. It is not that the peer received the frame. Profile 0 has no acknowledgement. When the stream takes a frame and then resets, the core drops the waiting local copies and returns `Link(SendAfterClose)`. The exam scripts that case. The peer received the frame. A holder is therefore told that a publication failed which the peer received.

## The client protocol

The protocol follows [`network-service`](../../contracts/network-service/README.md). The holder resolves a nontransferable control endpoint and a provisioning endpoint. At attach, the service delegates one IO0 ring page and one 4096-byte data page as shared-buffer loans. A 64-byte descriptor accompanies each loan. The attach completion reports the number of rows the holder has.

- **A request names a row.** It carries the row index, the operation and the holder's own sequence number. It names no key, holder, session, link or peer. The endpoint tells the service who the holder is.
- **Publish.** The payload travels in the data page. The service stamps the timestamp from its own clock. The completion reports how many local holders the sample reached. One flag reports that a copy went to the peer.
- **Take.** A take returns none, a sample or a loss count. A sample returns a 16-byte header and then the payload in the data page.
- **Sizes.** A request is 32 bytes and a completion is 24 bytes. The IO0 payloads are 56 and 24 bytes. The largest sample payload is 4080 bytes, because the header and the payload fill the data page.
- **A larger sample is outside this version.** The grants contract allows samples up to 1048576 bytes. A holder declared with a larger sample cannot be served by version 1.

The client library is proposed in step 7 and has no exam yet. Its shape follows the network adapter. It owns one outstanding request. A missing or malformed completion poisons the session and returns the loans.

## The wire records

The generator reads the four layouts and writes one struct, one `encode` and one `decode` for each. A field's type comes from its width and sign. A reserved field is a byte array. An offset is the sum of the widths before it. The renderer refuses a layout whose widths do not add up to the size the schema declares.

The judge writes each record again with Python's `struct` from the same layout, and requires the Rust encoder's bytes to equal them. Every field, reserved padding included, holds a distinct nonzero pattern in the test, so a swapped, shifted, truncated or sign-extended field changes the bytes. The judge also refuses seven corrupted layouts and eleven edits of the generated file.

I ran twelve mutations of the generated Rust against the test and the judge. They moved an offset, changed a sign or an endianness, dropped a field, read from the wrong offset, and removed or shortened the length check. The test or the compiler caught all twelve, and the judge refused all twelve.

## The client adapter

The adapter follows the network adapter. Attach sends one control request and imports two loans, the ring first. A request names a grant row by its index. The payload and the sample header travel in the data page. The adapter holds one outstanding request, and it checks each completion against the request it sent.

- **A request outside the protocol is refused first.** A row past the holder's rows, a payload past 4080 bytes and a take buffer past 4080 bytes publish nothing.
- **An answer outside the protocol poisons the session.** The adapter has returned both loans, and every later request is refused as lost. The service may still use the data page, so it does not reuse it.
- **A refusal by the service is a status.** It leaves the session usable.
- **Teardown returns the loans first.** Detach and abort return both loans, send one request and wait for the acknowledgement. Detach on a poisoned session is refused, and abort succeeds.

The tests run in `components/proto/tests/zenoh_service_client.rs`, which compiles the adapter by path against a stand-in for the runtime, as the network adapter's tests do. One ordered test prints the transcript, because tests that run in name order would reorder it whenever one is renamed.

I ran thirty mutations of the adapter against the tests and the judge. The first run caught 21. Eight survived, and one anchor was dead. Each survivor was a missing case in my tests. They were a reserved field, a failure carrying a count and a teardown that never waits. They also included a poisoned detach, a data loan not returned, a buffer at the page bound and a redundant loan-length check. The last was an oversize sample whose transfer was consistent with its length. I added the cases and 29 of 30 are now caught. The survivor changes the guard on a sample longer than the buffer. `transact` already refuses a completion that moved more bytes than the request's capacity, so that guard cannot be reached. It stays as defense in depth, and the mutation changes no behaviour.

## Out of scope

- Calls and operations. The fabric provides them. The Zenoh equivalent is the queryable, which Profile 0 refuses and no stock-peer capture shows.
- Discovery: scouting, gossip and liveliness tokens. See [`zenoh-transport.md`](../architecture/zenoh-transport.md). None is captured or admitted.
- More than one peer. The service owns one link.
- Wildcard keys. A grant is an exact key.
- Replacing the fabric. This record changes nothing in the fabric.

## Still open

1. **How the grants reach the service.** The system spec lists network authority. The generation derives it, and a root-served table delivers it. A grants list needs a new resource kind and a read label in `slime-root` and `boot-contracts`. A planning PR cannot carry them. I did not find where resource kinds are registered. Step 8 owns the work.
2. **Large samples.** A ring slot has a fixed size equal to the session's payload bound. The record declares the sample size. A larger sample needs a receiver-bound loan, and the `heap` link configuration fragments it. The component owns both. This record's exams do not judge them.
3. **Several subscriptions on one key.** The session delivers a sample once when any subscription on its key matches. With two subscriptions on one key, the event names one of them. Each declared key must therefore be subscribed once. The grants contract's count of distinct keys already guarantees it.
4. **Unequal sessions.** A probe gave the sender three subscriptions and the peer room for two of the sender's. The peer refused the third declaration as `subscriber-limit`, and the sender still counted three. The subscribers exam does not judge this divergence. I did not measure the sender's later frames. A refused frame is rolled back on the peer, so its sequence number is not consumed there, while the sender has spent it. I expect later frames to fail as a sequence mismatch. That is an inference from reading the session. The implementation decides whether the sender learns of the refusal.

## Limits

The core over a link was first measured in a throwaway on a scripted byte stream, which produced the 61 cases and 151 controls. The implementation PR then ran 11 mutations of the link-facing core against the exam and the judge. The judge refused 9 of them. The first run accepted one, a second publication allowed while one waits, and the exam then gained a call after the flush and before `settle`. Review then showed that no case mixed a declaration with a publication, and none held the send slot with a keep-alive. The exam gained both. Two mutations are still accepted when applied alone. One skips the core's check of the link slot. The other passes the link's busy error through as a link error. I did not trace why. From reading the code, either the core's check or the link's own check still returns a busy refusal. Each mutation alone then changes no printed line. A core with both changes is refused. The exam therefore cannot tell which of the two guards a core relies on. One unreproduced segmentation fault occurred once in an earlier link probe, and I do not know its cause. The recipe `just zenoh_service_link_check` passes against the implementation with 61 cases and 151 controls.

The wire records and the adapter were measured on a throwaway, and a revert removed it. The adapter has run only against a scripted service, because no service exists. The grants path, the component and the composition are not measured. The client protocol has no Rust throwaway. Its judge reads the schema as text, so its 24 cases and 80 controls come from running that judge. In the first run of the controls, one edit was refused for the wrong reason. It meant to make a named field a byte array and broke the line instead. I rewrote it, and it is now refused as a byte array. Review then found that the judge compared each layout with the schema's own record types. Removing a field from a type and its layout together was accepted. The judge now holds the field list of each record apart from the schema. It also requires each constant to fit the width and sign of the field that carries it. The magic is a little-endian integer, so the four bytes on the wire read `ZSVC`. The rest of this record is a plan, and nothing in it is implemented. A throwaway session, a throwaway service core and a throwaway event change produced the pinned counts in a worktree. A revert removed them. The numbers come from those runs and not from reading the code. The first measurement of the row size gave 48 bytes, so the grants contract's row overhead is 48 and not the 24 I had assumed. The sizes are 28 for a slot, 48 for a row, 24 for a holder and 8 for the core beyond its rows and rings. The row holds a reference to its key, and the core keeps a second reference to each distinct key. No exam runs against an implementation until the implementation PRs land. The three new recipes fail until then.
