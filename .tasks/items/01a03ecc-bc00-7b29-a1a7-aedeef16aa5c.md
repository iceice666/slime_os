---
schema: work-item/v1
id: 01a03ecc-bc00-7b29-a1a7-aedeef16aa5c
key: B78
kind: bug
state: done
created: 2026-08-27T00:00:00+08:00
closed: 2026-08-27T00:00:00+08:00
tags:
  - backlog
---

# CI could not pass: a prefix gate on a prefixless runner, a job with no runner, and a stale child path

## Context

Resolved 2026-08-27. **Class:** Defect (three independent breakages that together made the suite structurally incapable of concluding).

`contracts_check` ran on an ordinary runner but builds all 36 seL4 manifests and so needs `build/sel4-prefix`, failing every run; the `sel4_builder` job requested a `self-hosted` label the repository has no runner for and queued forever; and `lint_sel4_root` named a root child ELF path `0dd7d0c` stopped writing, so it resolved only on a checkout holding a stale artifact.

Class: Defect (three independent breakages that together made the suite structurally incapable of concluding).

## Exit conditions

CI run 33002668719 is green across all ten jobs, including the three hosted `ubuntu-24.04-arm` jobs that build the seL4 prefix inside the repository's Nix dev shell before consuming it.

## Evidence

[`devlog/2026-08-27-ci-hosted-arm64-cutover/`](../devlog/2026-08-27-ci-hosted-arm64-cutover/index.md)

## Notes

Migrated from `roadmap/00-backlog.md` line 172: `B78 — CI could not pass: a prefix gate on a prefixless runner, a job with no runner, and a stale child path`.
