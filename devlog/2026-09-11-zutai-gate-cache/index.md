# The zutai gate passes: memoized by content, prefetched in parallel

| Field | Value |
|---|---|
| Date | 2026-09-11 |
| Kind | Change |
| Status | Verified |
| Scope | `scripts/lib/zutai_cli.py`, `scripts/lib/system_spec.py`, `scripts/lib/component_spec.py`, `scripts/lib/interface_schema.py`, `scripts/lib/system_image_closure.py`, `scripts/check/check-system-spec.py`, `scripts/check/check-system-image-builder.py`, `scripts/generate/generate-generation-from-spec.py`, `scripts/generate/generate-system-image-closures.py` |
| Work items | 01a090af-08a4-763b-8a9f-76b7d6376dd4 |
| Gates | `just system_spec_check`, `just contracts_check` |
| Trigger | Wall times measured on 2026-09-11 while landing IO11: `system_spec_check` 21 min, `system_image_closure_check` 25 min, `system_image_builder_check` 63 min, every one from a serial `zutai-cli` pass over the whole corpus |
| Baseline | Every gate evaluated every spec serially and from scratch, once per script, with byte-identical outputs and identity refusals on a stale input |

## Summary

Every contract gate shelled out to `zutai-cli` once per spec, serially, and each script in a stacked recipe repeated the whole pass. Measured per evaluation, only the system-spec checker is expensive — about eight seconds and 1.6 GB of RSS, against tens of milliseconds for the component, interface, and closure checkers — so a gate's cost was 43 system evaluations per script, and a one-line component change paid several such passes before its one-second QEMU plane could boot. `zutai_cli.evaluate` now memoizes each evaluation's stdout on disk under a key built from the content of everything that determines it, and `prefetch` fills that cache for a whole corpus through a thread pool before each unchanged serial loop runs. The loops, their order, and their refusal text are untouched, so outputs are byte-identical by construction; the serial-versus-cached hash comparison over every corpus, the `--check` generators, and the stale-input refusals confirm it. Measured on this 16-core, 14 GB host while another session ran the serial generators alongside and the pool was held to two workers: `check-system-spec.py` fell from 664 s serial to 3.1 s warm, `generate-generation-from-spec.py --check` from 512 s to 1.7 s, and the closure generator's `--check` from 598 s to 7.7 s. The item's recipe-level bound could not be timed here: `just system_spec_check` transitively runs `contracts_check`, which needs a built seL4 prefix and takes over two minutes on its own, and the closure generator's comparison fails in any git worktree for a pre-existing reason filed as `01a09118-1086-78e6-b8aa-f25920b8992d`.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `scripts/lib/zutai_cli.py` | `evaluate(command, target, input_path=…, env_var=…)` keys an evaluation on the binary's content, the stdlib files, every `.zt` and `zutai.zti` package manifest under `contracts/` (the CLI discovers its package graph from those manifests on every run, so the checker's siblings alone are not the closure), this module's own source, the checker's path, the command, the env var name, and the input bytes; stores a zero-exit stdout under `build/zutai-cache/<key>` by tmp-and-rename; a store that cannot be read or written is skipped, never fatal; `ZutaiError` carries a non-zero exit; `SLIME_ZUTAI_CACHE=0` bypasses the store; `binary()` is checked once per process | A cached result is a pure function of its inputs: the input's path is never in the key, so a temporary file rewritten with other contents is another entry, and a non-zero exit is never stored, so a malformed input is refused on every run; a cache failure degrades to the uncached evaluation |
| `scripts/lib/zutai_cli.py` | `prefetch(jobs)` computes every key on the calling thread, drops hits and duplicates, then evaluates the misses on a `ThreadPoolExecutor` sized `min(cpu_count, MemAvailable // 2 GiB)` or `SLIME_ZUTAI_JOBS`, swallowing only `ZutaiError` | Parallelism never changes what a gate reports: a failed job is re-run by the serial pass, which keeps today's refusal order and text |
| `scripts/lib/{system_spec,component_spec,interface_schema,system_image_closure}.py` | `_run_zutai` routes through `evaluate` with signatures, pre-checks, and messages unchanged; `system_spec.prefetch_systems(paths)` warms a set of systems; `interface_schema`'s `json`-only catalog loader keeps the direct binary | Byte identity of every derivation: only the source of the stdout string changed |
| `scripts/check/check-system-spec.py`, `scripts/check/check-system-image-builder.py`, `scripts/check/check-system-image-scenario.py`, `scripts/generate/generate-generation-from-spec.py`, `scripts/generate/generate-system-image-closures.py` | One `prefetch_systems(...)` call before each existing corpus-scale serial loop | The loops, their order, and their error attribution are unchanged |
| `scripts/lib/system_image_closure.py` | `resolve_closure` admits the component corpus once and derives `components` from it, instead of twice | Same corpus, same rules, half the evaluations per closure |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A cached result masks a stale input | `just system_spec_check` | `generate-generation-from-spec.py --check` reports a stale fixture and `check-system-spec.py` reports the diverging field after a spec edit, with a warm cache; `check-system-image-closure.py` adds `identity mismatch` |
| A cached result masks a malformed input | `just system_spec_check` | its negative arms rewrite one temporary file per mutation; a path-keyed cache would accept the second arm with the first's verdict |
| Parallel evaluation changes an output | `just system_spec_check`, `just contracts_check` | any `--check` generator reports drift against the committed, serially produced fixtures |

## Verification

All commands ran through `nix develop --command` from the worktree `/space/slime_os-zutai` on `perf/zutai-parallel`, 16 cores and 14 GB, with `SLIME_ZUTAI_JOBS=2` because another session was running the same serial generators in `/space/slime_os` throughout; every wall time below is therefore an upper bound. Raw stage lines with `MemAvailable`, load, and entry counts are in [timings.txt](timings.txt).

| Command/scenario | Result | Evidence class |
|---|---|---|
| Serial-versus-cached stdout: sha256 of every `_run_zutai` result over 43 systems, 82 components, 4 interfaces, 50 closures, and the test runs (224 rows), once with `SLIME_ZUTAI_CACHE=0`, once cold, once warm | all three listings identical ([identity-serial.txt](identity-serial.txt), [identity-warm.txt](identity-warm.txt)); 0 entries after the disabled run, 448 after cold and after warm; 8 min 19 s, 7 min 10 s, 1.8 s | Direct |
| `env SLIME_ZUTAI_CACHE=0 python3 scripts/check/check-system-spec.py` | rc 0, 664.1 s; `contracts/` unmodified | Direct |
| `python3 scripts/check/check-system-spec.py`, cold cache | rc 0, 518.3 s (the 22 negative arms still serial) | Direct |
| `python3 scripts/check/check-system-spec.py`, warm cache | rc 0, 3.1 s; entry count unchanged, so the arms hit | Direct |
| `env SLIME_ZUTAI_CACHE=0 python3 scripts/generate/generate-generation-from-spec.py --check` | rc 0, 512.0 s | Direct |
| `python3 scripts/generate/generate-generation-from-spec.py --check` after the checker, the recipe's order | rc 0, 1.7 s | Direct |
| `python3 scripts/generate/generate-system-image-closures.py --check` serial, cold, warm | 598.1 s, 315.8 s, 7.7 s; rc 1 in all three with the same 50 stale closures, the worktree gitlink defect `01a09118-1086-78e6-b8aa-f25920b8992d` and not this change (the primary checkout's loader digest equals the committed identity; both checkouts agree with `.git` excluded) | Direct |
| Stale input on a warm cache: `bootAttempts` 3 to 4 in `sel4-channel.zti` | `generate-generation-from-spec.py --check` names only `sel4-channel.zti` stale; `check-system-spec.py` reports `manifest.health.bootAttempts: 4 != 3`; `check-system-image-closure.py` reports `systemSpec: identity mismatch for … sel4-channel.zti`; reverted, tree clean ([refusal.txt](refusal.txt)) | Direct |
| Malformed input: a spec copy with `garbage {{{` appended, compiled twice | refused both times with `malformed Zutai input`; entry count unchanged | Direct |
| `just ruff` | passed | Direct |
| `just contracts_check` in the fresh worktree before the seL4 build | rc 1 after 136.7 s: `check-generation-v5.py` needs `build/sel4-prefix`; unrelated to this change ([timings-recipe-attempt.txt](timings-recipe-attempt.txt)) | Direct |
| `just sel4_qemu_image_check` to install the prefix, then `just contracts_check` | rc 0, 825.0 s on first run with cold build outputs; `contracts/` unmodified | Direct |
| `just system_spec_check`, warm cache | rc 0, 190.4 s end to end, of which the `contracts_check` and `component_spec_check` prefix is nearly all; the item recorded 21 min for this recipe before the change | Direct |
| `just devlog_check`, `just tasks_check`, `just typos` | passed | Direct |
| After review fixes (key widened to `contracts/**/*.zt` + manifests + this module; tolerant store; memoized `binary()`), four workers, host quieter: identity over 5 systems, 82 components, 4 interfaces, 50 closures (141 rows) serial vs cold vs warm | identical; 0 entries after the disabled run, 282 after cold and warm; 66.6 s, 69.0 s, 0.1 s ([reverify.txt](reverify.txt)) | Direct |
| Store made unreadable and unwritable (`chmod 000 build/zutai-cache`), one component evaluation | rc 0, `#valid` returned; restored | Direct |
| `check-system-spec.py` cold / warm; `generate-generation-from-spec.py --check` warm | 388.0 s / 1.1 s; 0.3 s | Direct |
| Warm-cache refusals repeated after the `bootAttempts` edit | generator rc 1 naming only `sel4-channel.zti`; checker rc 1 with `manifest.health.bootAttempts: 4 != 3`; reverted | Direct |
| `python3 scripts/check/check-system-image-scenario.py` | rc 1 at once with `loader.implementation: identity mismatch for deps/rust-sel4`, the worktree gitlink defect, before its prefetch runs | Direct |
| `just system_spec_check` warm, `just devlog_check`, `just tasks_check` | rc 0 in 157.9 s; rc 0; rc 0 | Direct |

## Decisions

- Decision: prefetch into an on-disk cache and keep every serial loop, rather than compiling specs inside the pool.
- Rationale: the eight seconds are entirely in the child process, which releases the GIL, so a pool of evaluations captures the whole win; the Python side of a compile is milliseconds, its first call warms unlocked `lru_cache` readers that a pool would duplicate, and a pool would have to re-sequence exceptions to keep refusal order. With the loops untouched, byte identity is a property of the change, not a proof obligation. Only a disk cache carries across the separate processes a stacked recipe runs.
- Rejected alternative: a batch mode or result cache inside `zutai-cli`. The CLI takes one path per invocation and its semantic cache is in-process; changing the submodule is out of this item's scope and would not carry across scripts either.
- Decision: key on every `.zt` and package manifest under `contracts/` rather than the checker's directory alone.
- Rationale: `zutai-cli` discovers its package graph from `contracts/zutai.zti`, `contracts/_shared/zutai.zti`, and `contracts/generation/v5/vocab/zutai.zti` on every run and a checker may import across packages, so a key limited to the checker's siblings would serve a stored `#valid` after a manifest broke. Over-inclusion costs one cold pass per contract edit; under-inclusion costs a masked refusal.
- Decision: hash the binary's content rather than its mtime, and never garbage-collect the store.
- Rationale: `binary()` rebuilds by mtime, so a fresh worktree rebuilds identical bytes with a new mtime; an mtime key would discard every entry for nothing. Entries are about 1.4 KB and a full corpus is about 250 of them; `rm -rf build/zutai-cache` is the reset, and the format tag in the key orphans old entries when the recipe changes.
- Decision: a store that cannot be read or written is skipped rather than fatal.
- Rationale: the evaluation has already produced a correct result when the store fails; a gate that dies on `PermissionError` from a cache directory reports nothing about the specs.
- Rejected alternative: restructuring `check-system-spec.py`'s 22 negative arms to write every mutation first and prefetch them. Their contents are deterministic, so they warm on the first run and hit after; the item's exit condition is the warm run, and tabulating 22 `rejected(...)` calls is a gate reshape for its own entry if cold time matters.

## Open risks and follow-ups

- [ ] A cold `check-system-spec.py` still pays the 22 negative arms serially (about three minutes); a follow-up may write all arms first and prefetch them.
- [ ] `just system_spec_check` transitively runs `contracts_check` (model checks, `check-generation-v5.py`, and nine `--check` generators), whose time this change does not touch; the recipe passed in 190 s warm here while its own two scripts took under 5 s, so the item's two-minute bound holds for the scripts and not for the recipe. The prefix is follow-up `01a0915e-d8a1-722b-91e4-cb3ec57c6632`.
- [ ] Every closure digests `deps/rust-sel4` including its `.git` gitlink, so `generate-system-image-closures.py --check` and `just system_image_closure_check` fail in any git worktree; filed as `01a09118-1086-78e6-b8aa-f25920b8992d`.
- [ ] `_key` hashes every `.zt` in the checker's directory; a checker that one day imports from another directory would need that directory added to the key.

## Artifacts and provenance

- Focused report: none.
- Raw transcript: none; the hash lists and timings below are the observed outputs.
- Serial/debugger/model output: [identity-serial.txt](identity-serial.txt), [identity-warm.txt](identity-warm.txt), [timings.txt](timings.txt), [timings-recipe-attempt.txt](timings-recipe-attempt.txt), [refusal.txt](refusal.txt), [reverify.txt](reverify.txt).
- Related work items: `01a090af-08a4-763b-8a9f-76b7d6376dd4` (this change); `01a09118-1086-78e6-b8aa-f25920b8992d` (the worktree gitlink defect found while verifying it); `01a0915e-d8a1-722b-91e4-cb3ec57c6632` (the recipe prefix that remains).
