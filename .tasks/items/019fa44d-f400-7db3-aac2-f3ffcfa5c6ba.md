---
schema: work-item/v1
id: 019fa44d-f400-7db3-aac2-f3ffcfa5c6ba
key: C8.4
kind: milestone
state: done
created: 2026-07-28T00:00:00+08:00
closed: 2026-07-28T00:00:00+08:00
tags:
  - core-runtime
parent: 01a00b4d-2400-77ed-a3dd-39becc4893af
depends:
  - 019f9f27-9800-790e-b8f1-663928c35f8a
---

# Bounded many-to-many streams

## Context

Complete.

## Exit conditions

A generation-declared many-to-many stream moves bounded typed inline and shared samples under exact route authority; KEEP_LAST and BEST_EFFORT behavior is deterministic, and a stalled or faulting participant cannot grow or disturb unrelated state. On a real boot two publishers and two subscribers exchange both sample forms over `telemetry` while `diagnostics` carries an unrelated stream through the same service; one `>MAX_MSG` sample is counted at exactly one fabric copy and one quota-charged receiver-bound loan per subscriber. The eviction rule is pinned by host unit tests because a transcript can show samples arrived but not which one was dropped; a participant fault beyond a deliberate stall is C8.9's composition.

## Verification

Gates: `just fabric_stream_check`

## Evidence

[`devlog/2026-07-28-c8-4-bounded-streams/`](../devlog/2026-07-28-c8-4-bounded-streams/index.md)

## Notes

Migrated from `roadmap/02-core-runtime.md` line 370: `C8.4 — Bounded many-to-many streams`.
