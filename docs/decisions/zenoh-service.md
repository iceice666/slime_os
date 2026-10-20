# A shared Zenoh service component

**Status:** Proposed
**Related work items:** `01a15900-b7a5-715e-9706-110fde699faf` (a session that holds several subscriptions of its own), `01a15b9a-21c1-749d-b945-c8ab0fe5f90d` (the grants contract) and `01a15b9a-2924-7131-a81b-74e6f385a3d6` (the service core, which depends on both). Zenoh Profile 0 epic: `01a0f5fa-f2c4-71cf-962b-c62cbea2ef68`.

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

1. **A session that holds several subscriptions of its own** (`01a15900-b7a5-715e-9706-110fde699faf`). `Params` gains `local`, and the table holds `per_side + local` slots. `Params::DEMO` and `Params::from_limits` keep `local` at 1. No link built from `zenoh-link-limits/v1` and no landed exam changes. The slice `--slice subscribers` pins 28 cases and 90 controls. It runs the links exam and the stock-peer exam beside it. A link configuration that declares more than one subscription needs a new version of the link-limits contract and re-admits the links item. This item does not do that.
2. **The grants contract** (`01a15b9a-21c1-749d-b945-c8ab0fe5f90d`). `contracts/zenoh-service-grants/v1`, judged through its own decoder. The slice `--slice grants` pins 68 cases and 58 controls. Ten of the controls are weakened contracts that must each take the record that the removed check refuses. Four more make an upper bound exclusive, and the weakened contract must then refuse the record at that bound. The binding generator and the generated Rust constants belong to the implementation.
3. **The service core** (`01a15b9a-2924-7131-a81b-74e6f385a3d6`). It follows items 1 and 2. The slice `--slice service` pins 50 cases and 126 controls. The item adds the matching subscription's id to `SampleDelivered`. The Rust test also prints the `size_of` of a slot, a row, a holder and the core. The judge refuses a size above the matching overhead constant in the grants contract, so the declared stack covers what the core stores.
4. **The component, the client interface and a booted composition.** This item covers the service and its control and provisioning endpoints. It covers the ring and data loans and the typed adapter. It covers the path of the grants from the system spec to the service. It also covers a composition with two clients under QEMU. It is not planned, because it cannot be graded until item 3 lands.

## Out of scope

- Calls and operations. The fabric provides them. The Zenoh equivalent is the queryable, which Profile 0 refuses and no stock-peer capture shows.
- Discovery: scouting, gossip and liveliness tokens. See [`zenoh-transport.md`](../architecture/zenoh-transport.md). None is captured or admitted.
- More than one peer. The service owns one link.
- Wildcard keys. A grant is an exact key.
- Replacing the fabric. This record changes nothing in the fabric.

## Still open

1. **How the grants reach the service.** The system spec lists network authority. The generation derives it, and a root-served table delivers it. A grants list needs a new resource kind and a read label in `slime-root` and `boot-contracts`. A planning PR cannot carry them. I did not find where resource kinds are registered. Item 4 owns the work.
2. **Large samples.** A ring slot has a fixed size equal to the session's payload bound. The record declares the sample size. A larger sample needs a receiver-bound loan, and the `heap` link configuration fragments it. The component owns both. This record's exams do not judge them.
3. **Several subscriptions on one key.** The session delivers a sample once when any subscription on its key matches. With two subscriptions on one key, the event names one of them. Each declared key must therefore be subscribed once. The grants contract's count of distinct keys already guarantees it.
4. **Unequal sessions.** A probe gave the sender three subscriptions and the peer room for two of the sender's. The peer refused the third declaration as `subscriber-limit`, and the sender still counted three. The subscribers exam does not judge this divergence. I did not measure the sender's later frames. A refused frame is rolled back on the peer, so its sequence number is not consumed there, while the sender has spent it. I expect later frames to fail as a sequence mismatch. That is an inference from reading the session. The implementation decides whether the sender learns of the refusal.

## Limits

This record is a plan, and nothing in it is implemented. A throwaway session, a throwaway service core and a throwaway event change produced the pinned counts in a worktree. A revert removed them. The numbers come from those runs and not from reading the code. The first measurement of the row size gave 48 bytes, so the grants contract's row overhead is 48 and not the 24 I had assumed. The sizes are 28 for a slot, 48 for a row, 24 for a holder and 8 for the core beyond its rows and rings. The row holds a reference to its key, and the core keeps a second reference to each distinct key. No exam runs against an implementation until the implementation PRs land. The three new recipes fail until then.
