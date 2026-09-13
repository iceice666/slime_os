---
schema: work-item/v1
id: 01a03ecc-bc00-708d-a6fe-fffc759caf66
key: P5.2
kind: milestone
state: done
created: 2026-08-27T00:00:00+08:00
closed: 2026-08-27T00:00:00+08:00
tags:
  - architecture
parent: 01a0724c-5400-7208-b5a1-7008c0e2bdc3
depends:
  - 019fc334-1c00-7f76-a730-e0cd1f70b706
---

# Native component images on seL4

## Context

Complete.

## Exit conditions

`just sel4_component_graph_check` boots six ELF payloads, observes `SLIME_GRAPH healthy ... required=4 live=4 idle=4 failed=0`, init's resident-supervision marker, the Slisp prompt, and its first blocked input wait. `just slisp_core_check` independently exercises the same C evaluator through persistent definition, lexical use, refusal, and clean exit.

## Verification

Gates: `just contracts_check`, `just generation_check`, `just sel4_component_graph_check`, `just slisp_core_check`

## Evidence

[`devlog/2026-08-04-p5-2-native-component-images/`](../devlog/2026-08-04-p5-2-native-component-images/index.md), [`devlog/2026-08-27-resident-product-graph/`](../devlog/2026-08-27-resident-product-graph/index.md), [`devlog/2026-08-27-slisp-product-cutover/`](../devlog/2026-08-27-slisp-product-cutover/index.md)

## Notes

Migrated from `roadmap/07-architecture-portability.md` line 458: `P5.2 — Native component images on seL4`.
