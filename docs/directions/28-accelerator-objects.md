# 28. Accelerator compute objects

| | |
| --- | --- |
| Status | parked |
| Route | hardware |
| Depends on | M7 hardware bring-up and IOMMU-enforced DMA; a capability-matrix row for the new object kind |
| Enables | agent inference authority and budget as manifest data |
| Now | Paper: the Accelerator object shape, rights strings, and budget model are a matrix amendment exercise legal today. |

## Motivation

The agentic direction makes a language model a userspace service, but
the Framework target's NPU and GPU have no corresponding authority
story: [entry 17](17-energy-accounting.md) accounts energy, not
compute. Introduce an `Accelerator` object kind with queue-submission
rights and generation-declared compute budgets, so an agent component's
inference authority and budget are manifest data like every other
grant — "which components may run inference, and how much" answered
statically, not discovered at runtime.

## What exists today

- The authority shape is proven by storage: BlockDevice gating
  (M5.1, `storage_cap_check`) is the template — a device object, split
  rights, unprivileged components cannot acquire it.
- The matrix grammar (`../capability-matrix.md`) defines what a new
  object kind must satisfy; Accelerator would be the first compute
  class in it.
- The audit consumer exists in design: [entry 9](09-grant-graph-introspection.md)
  answers "which components hold accelerator authority" from the
  manifest.
- Missing: M7 hardware bring-up (no NPU/GPU drivers), and
  IOMMU-enforced DMA — without it, an accelerator's memory access is
  outside the capability model entirely, which is why the register
  names it as a dependency rather than a detail.

## Design sketch

An Accelerator object represents one compute device (or queue class on
it). Rights split submission authority — at minimum a SUBMIT right to
place work on a queue — from management rights (queue creation,
firmware or mode control). The budget model rides on
[entry 25](25-resource-accounts.md)'s account pattern or declares
compute quantities in the manifest directly: tokens, work items, or
queue-time per window; exhaustion is a structured error or throttling,
declared per component.

DMA containment is the safety floor: the accelerator's memory access
must be IOMMU-constrained to buffers the submitting component holds,
or the SUBMIT right is an ambient-memory-read backdoor. The buffer
handoff should reuse the SharedBuffer path (and its horizon quota
question) rather than invent a parallel mechanism.

The agent story composes with [entry 18](18-network-authority.md):
local inference authority (this entry) and remote model destinations
(entry 18) are two disjoint rights, so a manifest can express "local
model only, no network" or its inverse — the two most important agent
deployment postures — as enumerable data.

## Open questions

- Budget unit: tokens, work items, queue time, or energy-proxy — and
  is the budget an entry-25 account quantity or a manifest scalar?
- One Accelerator row per device, or per queue class (NPU inference
  versus GPU compute as distinct objects)?
- Preemption: may a higher-priority component's queue evict another's
  work, and is that authority a right?
- Firmware loading for the accelerator: whose authority, and does it
  interact with generation verification?

## Exit-condition sketch

A component without the accelerator capability cannot submit work; a
component past its declared budget is rejected or throttled with a
structured error; the manifest lists every component holding
accelerator authority.

## Probe guidance

Paper: the matrix amendment (object shape, rights strings, budget
model) plus the IOMMU containment requirements list for M7 bring-up —
the driver work should discover zero authority questions mid-flight.
Validation of the exit condition waits for hardware.
