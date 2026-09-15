# `sel4-stream.zti` — bounded stream composition

The [`sel4-stream.zti`](sel4-stream.zti) manifest owns `bootAction = "stream"`,
two publishers, two subscribers, the fabric service, and an intruder with a real
control endpoint but no route edge. The shared protocol and authority model live
in the [fabric architecture](../../../../docs/architecture/typed-data-fabric.md).

## Activation and authority

Init has `spawnBudget = 6`. The
[current launcher](../../../../components/system/init/src/fabric_planes.rs)
starts the four ring participants and the intruder before the service. It passes
the service its buffer factory and four supervision capabilities, in ascending
declared destination-slot order: publisher at 7, subscriber at 8, publisher B at
9, subscriber B at 10. The handles name ring-loan receivers and expose participant
termination; an Endpoint alone does not report peer death. Publisher B also
receives a buffer factory at slot 1.

Control Endpoints are declared grants installed by the root, not pairs created
or handed out by init. The service's controls occupy slots 2–6. Separate narrowed
publisher/subscriber probe grants occupy service slots 11 and 12; they are not
route edges. Their exact masks and transfer rights belong to the manifest, not
an inferred endpoint-kind ceiling. No init executable grant is transferable.

## Data and budgets

Telemetry carries inline and larger-than-inline samples. Publisher B also
publishes diagnostics; subscriber B exercises a telemetry stall and bounded loss
while diagnostics remains independent. Telemetry subscriber history depths are 8
and 4; the diagnostics endpoints use depth 2. These are declared policy inputs,
not measured queue capacity or a throughput claim.

The service declares 28 pages and 14 buffers/mappings/loans, plus 16 private-memory
pages. Publisher B declares 8 pages, one buffer and loan, and four mappings.
Publisher A and both subscribers each declare four pages and four mappings but
no buffer or loan allocation quota. `capabilitySlots = 32`; notification grants
and bindings declare the ring ready/credit paths separately from controls.

## Verification boundary

The [stream checker](../../../../scripts/check/check-sel4-stream-plane.py) requires
causal chains for authority denials, matching, inline delivery, one broker copy
of a large sample with subscriber loan verification, bounded loss, and completion.
Independent chains may interleave. This document records those gate requirements,
not a fresh run or timed-QoS qualification; see [timed QoS](sel4-qos.md).
