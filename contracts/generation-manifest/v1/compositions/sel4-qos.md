# `sel4-qos.zti` — the C8.5 timed-QoS generation

This composition exercises timed QoS over the native typed fabric. The
[`sel4-qos.zti`](sel4-qos.zti) manifest owns its graph, grants, slot bindings,
and `bootAction = "qos"`; the [fabric architecture](../../../../docs/architecture/typed-data-fabric.md)
owns the shared protocol and authority boundaries.

## Clock and retained data

`fabric-publisher-b-clock` is a generation-declared native Endpoint between
`fabric-publisher-b` and `fabric-service`, separate from the publisher's
participant-control endpoint. Its bindings pin slot 3 on the publisher and
slot 11 on the broker. The root installs both halves before either task runs;
init does not mint or distribute this clock endpoint.

The publisher advances simulated time so the broker can exercise retry,
deadline, liveliness, and lifespan policy deterministically. The retained
diagnostics route supplies an inline retained head independently of the
telemetry publisher's timing. This is not a claim about physical-clock latency.

## Image and verification boundary

`just sel4_qos_check` runs `scripts/check/check-sel4-qos-plane.py`. The gate
builds the `sel4-qos-death` image closure, which selects `streamEarlyExit` for
the publisher so the peer-death arm is reachable. The base `sel4-qos` closure
does not provide that executable-changing scenario; authenticated boot-action
data and the selected implementation identity have separate roles.

The checker requires ordered marker chains for graph admission, participant
matching, simulated-time advancement, reliable retry accounting and exhaustion,
liveliness loss, deadline miss, lifespan expiry, publisher death, and plane
completion. Independent chains may interleave; one total serial ordering is
not the contract. These are gate requirements, not a fresh execution record.
