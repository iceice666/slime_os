# Channels

All communication in Slime OS is typed IPC over explicitly granted
endpoints. There are exactly two paths, and knowing which one you are on
answers most questions before they are asked.

The authoritative surface — operation labels, operand packing, reply
convention, error model, bounds — is [`../syscall-abi.md`](../syscall-abi.md).
This page is the model.

## Path 1: native — component to component

Component-to-component traffic is direct seL4 IPC on Endpoint and
Notification objects the generation declared as edges. The root creates
these objects while decoding the generation and mints each side's capability
at exactly the declared direction (send-only, receive-only, or both) — then
gets out of the way. Rendezvous, blocking, backpressure, and atomic
call/reply pairing are the kernel's; the root neither sees nor mediates a
single message.

Consequences:

- An edge that the generation does not declare **cannot be constructed at
  runtime** by the components themselves. The topology is a boot-time fact.
- Naming is authentication. A service knows who is calling because the
  request arrived on a generation-provisioned endpoint — never because the
  request body claims an identity. Fields like route names in a request are
  data to validate, not authority.
- Messages are small and bounded; anything larger crosses as a shared-buffer
  loan plus a descriptor message, never by widening the message bound.

## Path 2: root-served — component to mechanism

Everything the root owns as mechanism — lifecycle, spawn, supervision,
capability transfer, shared buffers, directories, the clock, hardware I/O
resources, input, debug output — crosses as one bounded `seL4_Call` on a badged
endpoint, carrying an operation label. The badge authenticates the caller; the
label selects the operation; the generation's service bindings decide whether
this caller may name that operation at all, before any argument is read.

Storage is deliberately *not* in that list. A block device is reached through a
supervised userspace driver over shared-memory rings, so bulk sector traffic
never crosses the root at all — the root's part is granting the driver its
device, MMIO, interrupt, and DMA authority, and copying the generation's
per-ring rights table to it.

Two endpoints, two threads: a noisy console must not queue behind lifecycle
traffic, and a console defect must not share the system dispatcher's fault
domain.

## Capabilities travel on channels

A channel can carry authority, not just bytes — that is what makes
"tool call = channel" literal rather than metaphor. The consuming transfer
protocol is deliberately multi-step (reserve, cross, finalize-or-cancel) so
that a failed transfer restores the source instead of losing the capability,
and the root authenticates what moved from the request registers, never from
descriptor bytes a component wrote.

Only some kinds can move at all; an executable or a factory reaches a child
as a spawn grant or a declared binding, never as a transfer. The matrix in
[`../capability-matrix.md`](../capability-matrix.md) says which is which.

## Error model

Every root-served operation answers a small frozen set of negative statuses.
Two properties are load-bearing:

- **Refusals are uninformative on purpose.** A missing capability, a wrong
  kind, and insufficient rights all answer the same code, so a probing
  component cannot map its own table by measuring error differences.
- **Unknown labels are refused, not guessed.** A component built against a
  newer ABI meeting an older root is denied one call — never mis-served a
  different operation. This refusal, not migration, is the compatibility
  mechanism.

## Related

- Schema-first message types: [Contracts](contracts.md).
- Fabric routes, brokered streams, and QoS are userspace policy built on
  these two paths — see `contracts/fabric-graph/v1/` and the C8 milestones in
  `roadmap/02-core-runtime.md`.
- Request/completion rings over shared memory plus Notifications are the third
  shape built on these two paths, for drivers and their clients — see
  `contracts/io-queue/v1/` and the IO milestones in
  `roadmap/11-io-substrate.md`.
