---
schema: work-item/v1
id: 019fd7cd-8c00-7f99-8e9b-a8dafb1ecbd4
key: P5.4.4
kind: milestone
state: done
created: 2026-08-07T00:00:00+08:00
closed: 2026-08-07T00:00:00+08:00
tags:
  - architecture
parent: 01a0724c-5400-7d48-8f10-a5383e7c21d9
depends:
  - 019fdcf3-e800-786a-ab69-2ae8b32874e9
---

# C8.2 aggregate fabric-graph admission

## Context

Complete.

## Exit conditions

The stream plane's real two-publisher/two-subscriber graph is admitted against `slime-root`'s ceilings and the plane runs unchanged, asserted as `SLIME_ROOT fabric graph=admitted` and fault-injected by removing the wiring; the other eight planes report `absent`, distinguishing "checked" from "nothing to check". Not closed by this: the oracle's `kernel/tests/fabric_manifest.rs` also asserts route-authority tuples, interposition-chain termination, and per-pair QoS compatibility over the booted graph — those stay with P5.4.10's partials.

## Verification

Gates: `just sel4_stream_check`

## Evidence

[`devlog/2026-08-07-p5-4-4-fabric-graph-admission/`](../devlog/2026-08-07-p5-4-4-fabric-graph-admission/index.md)

## Notes

Migrated from `roadmap/07-architecture-portability.md` line 602: `P5.4.4 — C8.2 aggregate fabric-graph admission`.
