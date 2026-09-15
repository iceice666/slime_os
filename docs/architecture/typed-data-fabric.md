# Typed data fabric

## Boundary

The fabric is userspace policy over generation-declared native endpoints and
shared-buffer loans. The root admits the declared graph and exposes authenticated,
scoped reads of participant rows, route indices, and declared ceilings through
`GRAPH_READ`, `GRAPH_ROUTE_INDEX`, and `GRAPH_QUERY` in `slime-root/src/ipc.rs`.
It supplies capability and buffer mechanisms but does not implement stream,
call, operation, QoS, correlation, or interposition behavior; userspace does.

Authoritative owners:

- interface identity and generated bindings: `contracts/interface-schema/v1/`;
- graph, participants, QoS, visibility, interposition, and limits:
  `contracts/fabric-graph/v1/`;
- stream, call, operation, QoS, time, visibility, and trace formats:
  `contracts/fabric-*/v1/`;
- decoding and admission: `boot-contracts/src/fabric_graph.rs` and
  `slime-root/src/generation.rs`;
- runtime policy: `components/services/fabric-service/src/main.rs` and the
  focused brokers in `components/lib/src/`.

## Identity and authority

An interface identity is a domain-separated digest of normalized versioned Zutai
schema data. A route combines its name, full interface identity, and contract
kind. A participant grant adds component identity and direction. Route names,
type tags, request fields, and graph visibility are descriptive data; only the
generation-installed endpoint and matching participant grant carry authority.

A fabric control request is authenticated by the generation-provisioned control
endpoint on which it arrives. The service ignores any caller-supplied identity
that would otherwise purport to grant a role. Provisioned participant endpoints
are narrowed to one direction and omit transfer authority, so a participant
cannot redelegate its route.

## Streams, calls, and operations

- `Stream<T>` supports bounded many-to-many delivery. Payloads up to the inline
  bound use one 64-byte message; larger samples use a typed descriptor and a
  receiver-bound shared-buffer loan.
- For a large publication the broker maps the upstream loan read-only, copies
  once into a fabric-owned sealed buffer, and creates an independently charged
  downstream loan per subscriber. It does not copy once per subscriber.
- Per-subscriber history is bounded. Best-effort loss is counted and emitted as
  one event when delivery resumes; reliable delivery uses explicit credits and
  acknowledgements rather than an unbounded kernel queue.
- Calls add bounded request/reply correlation. Operations compose goal,
  feedback, result, cancellation, and peer-loss transport; application policy
  and ROS action semantics stay outside the fabric.

The fabric graph declares every route and resource ceiling before boot. Current
format ceilings include 32 routes, 32 participants, nine ingress sources per
waiter, 64-byte control messages, 32 fabric-owned frames, and bounded queue,
history, event, retry, in-flight, buffer, mapping, loan, capability-slot, and
trace counts. A graph whose topology or budgets cannot be served is refused at
admission rather than discovered as a runtime deadlock.

## Time, QoS, visibility, and recording

Timed QoS consumes an explicit clock source. Deterministic test compositions use
simulated time; the standard runtime clock service supplies component-facing
monotonic time without changing the QoS state machines.

Visibility is declared and filtered. Introspection reports only the portion of
the graph a holder may observe and grants no route authority. Interposition is a
declared chain of ordinary components, not a privileged hook.

`contracts/recording-policy/v1/` declares recorder/replayer roles and a bounded
recording stream. A determinism claim is admitted only when every granted right
is classified as recorded, neutral, or explicitly exempted for the recording
input itself. Importing an unrecorded authority into a deterministic component is
refused. A successful replay claim is therefore about a declared composition and
a complete typed input trace, not about arbitrary component code.

## Verification

Current executable gates include:

- `just fabric_authority_check` — authenticated role provisioning and denial;
- `just sel4_qos_check` — reliable, retained, and timed QoS;
- `just sel4_fault_check` — degradation and fault isolation;
- `just sel4_fabric_aggregate_check` — repeated aggregate schedules with
  field-identical per-participant semantic records, not byte-identical serial
  traces; arrival ordinals and designated poll-sampled high-water counters are
  exempt from equality;
- `just sel4_gate_control_check` — negative control over the marker contracts.

Historical delivery order and measured evidence remain in
[`roadmap/02-core-runtime.md`](https://git.justaslime.dev/iceice666/slime_os-history/src/commit/45ed1745907b2d0a13fdf70c8b34eb635bed5f23/roadmap/02-core-runtime.md); this page is the
current subsystem owner. Its historical evidence links resolve to the immutable
archive described in [the history guide](../history.md).

## Coverage limits

The current gates do not establish broader coverage than these boundaries:

- Exact resource ceilings are reached only for in-flight calls, in-flight
  operations, and retained operation results; other declared resource classes
  remain unsaturated. `queueDepth` is declared and recorded but unconsumed.
  `resourceEvent` has no emitter because `ERR_WOULDBLOCK` is unreachable through
  blocking `seL4_Send`.
- Loan evidence comes only from the stream broker. Its mapping occupancy is
  provisioning-fixed while loan occupancy varies with traffic, so a mapping-only
  record cannot prove traffic variation. The call worker has no trace-sink
  headroom for mapping/loan pairs.
- End-of-run mapping counts are steady-state samples, not lifetime invariants or
  peaks: subscriber loan mappings and a publisher's extra mapping are transient.
  Three arms do not report: `fabric-call-client` releases its transient charge
  inside the helper before sampling, so a synthetic `[0, 0]` pair is invalid;
  `fabric-call-server` exits on injected peer death before flushing; and
  `fabric-call-worker` lacks trace-sink headroom. The stream-broker sample is not
  full-holder coverage.
- Capability-slot evidence observes the stream broker below its declared ceiling;
  it proves neither saturation nor coverage of every instrumentable participant.
- QoS distinctness is asserted only on call and operation planes; the stream plane
  emits no `kind=qos` record.
- The injected fault is interposition-hop death only. Stalled-subscriber and
  genuinely faulting-participant injections remain unexercised; scripted
  peer-death settlement is not evidence for either injection.
- Normalized-schema determinism is host-qualified, not established by QEMU boot
  comparisons. Denial, stall, and malformed schedules run as aggregate arms, not
  separate boots; their own fault assertions still apply.
