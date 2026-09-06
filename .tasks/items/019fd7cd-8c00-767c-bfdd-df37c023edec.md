---
schema: work-item/v1
id: 019fd7cd-8c00-767c-bfdd-df37c023edec
key: B27
kind: bug
state: done
created: 2026-08-07T00:00:00+08:00
closed: 2026-08-07T00:00:00+08:00
tags:
  - backlog
---

# the manifest→flag table set and scrubbed in one pass, so two manifests could not share a flag

## Context

Resolved 2026-08-07.

`build_sel4_generation`'s manifest→flag loop set the selected manifest's flag and popped every other manifest's flag in the same iteration, so once two manifests declared the same flag a later table row could pop what an earlier row set, with the wrong manifest winning depending on table order.

## Exit conditions

the loop now collects selected manifests' flags into one set and every declared flag into another, setting the first and removing only the rest, so a flag two manifests share survives independent of row order; `just sel4_stream_check` passes with the `sel4-qos` row present and both flags in effect, and all nine seL4 plane gates pass with every image rebuilt.

## Verification

Gates: `just sel4_stream_check`

## Evidence

[`devlog/2026-08-07-p5-4-5-qos-clock/`](../devlog/2026-08-07-p5-4-5-qos-clock/index.md)

## Notes

Migrated from `roadmap/00-backlog.md` line 1064: `B27 — the manifest→flag table set and scrubbed in one pass, so two manifests could not share a flag — **resolved 2026-08-07**`.
