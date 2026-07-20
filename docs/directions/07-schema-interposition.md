# 7. Schema-driven interposition toolchain

| | |
| --- | --- |
| Status | parked |
| Route | determinism; secondary: interposition |
| Depends on | M5.2a contract tooling (complete) |
| Enables | [entry 11](11-flight-recorder-replay.md) (recording machinery), agent dry-runs claimed in README's agentic direction |
| Now | Legal today as userspace/host tooling: a generated membrane over the existing block schema (`contracts/block/`). Named as an M5.3 follow-up in ROADMAP. |

## Motivation

Membranes and dry-run proxies are already claimed in README's agentic
direction: because no component holds ambient authority, every capability
can be transparently interposed. What is missing is the toolchain: because
all IPC is schema-first, a membrane can be *generated* from `contracts/` —
recording, throttling, sanitizing, and fault injection for any endpoint
with zero hand-written protocol code. Each new schema then comes with its
interposition machinery for free.

## What exists today

- M5.2a (complete): the versioned Zutai block schema generates both
  kernel Rust and component assembler bindings; `contracts_check`
  rejects stale bindings. The generator infrastructure a membrane
  generator would extend already exists.
- M5.3 (complete) is the hand-written instance of the general
  mechanism: deterministic request failure, timeout, reset, flush
  failure, interrupted write, and bounded rejection, plus flight-recorder
  replay, all verified by `storage_fault_check` — which is therefore the
  natural replay fixture for a generated membrane.
- Contracts live in `../../contracts/` with per-protocol versioned
  directories (`block/v1`, `store/v1`, `bootstate/v1`, ...), so the
  generator has one uniform input shape.

## Design sketch

The generator consumes a schema and emits an interposition component:
for each endpoint, a proxy that holds the real capability, exposes the
same schema version to the client, and applies a policy declared at
spawn — record every message, inject a declared failure at message N,
throttle to a rate, or sanitize fields. The policy is data (manifest or
spawn-time grant), keeping the kernel policy-free.

Recording format matters beyond this entry: [entry 11](11-flight-recorder-replay.md)
defines a bug report as generation hash plus IPC trace, so the trace
format the membrane emits should be the one 11 standardizes — bounded,
canonical, and storable as an object in the M5.4 store.

Fault-injection parity with M5.3 is the acceptance bar: the generated
membrane must reproduce a `storage_fault_check` failure class from a
recorded trace deterministically, proving the generated code matches the
hand-written semantics before any new protocol trusts it.

## Open questions

- Membrane placement: a userspace proxy component per interposed
  endpoint (current membrane model), or a shared interposition service
  multiplexing many endpoints?
- How does the client receive the membrane's endpoint instead of the
  real one — spawn-time wiring by the supervisor, preserving the
  manifest's declared graph shape?
- Trace format ownership: does this entry define it, or does entry 11
  (the consumer) define it and this entry conform?
- Sanitization policies are schema-specific logic — expressed how, if
  "zero hand-written protocol code" is the goal? (A declared field
  constraint language, or escape hatch to hand-written filters?)

## Exit-condition sketch

A generated membrane records and replays the block protocol; replay
reproduces a `storage_fault_check` failure deterministically.

## Probe guidance

Legal today: generate a record/replay membrane for `contracts/block/v1`
only, drive it against the existing storage slice, and replay a captured
`storage_fault_check` failure. The probe's output is the membrane
generator skeleton plus the trace-format proposal that entry 11 then
consumes; success promotes both entries' recording halves together.
