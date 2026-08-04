# P5.2 — Native component images on seL4

| Field | Value |
|---|---|
| Date | 2026-08-04 |
| Kind | Change |
| Status | Verified |
| Scope | `slime-root/src/{main,generation,graph,transfer_window,child_vspace,task}.rs`, `components/runtime/src/{runtime,syscall}.rs`, `components/bins/{build.rs,Cargo.toml}`, `components/component-aarch64.ld`, `components/bins/src/bin/init.rs`, `contracts/component/v2/`, `contracts/target-profile/v1/`, `contracts/generation/v1/fixtures/sel4.{zti,md}`, `boot-contracts/src/component_image.rs`, `scripts/build/build-{generation,sel4}.py`, `scripts/check/check-sel4-component-graph.py`, `Justfile` |
| Roadmap | P5.2, P5.3, P5 |
| Gates | `just sel4_component_graph_check`, `just sel4_root_boot_check`, `just contracts_check`, `just generation_check`, `just boot_layout_check` |
| Trigger | P5.1 closed with the note that no legacy component image runs and that rebuilding them as native ELF is P5.2 |
| Baseline | P5.1: `slime-root` boots and proves its mechanism against a native fixture; the generation's 25 payloads are `SLIMECM` images admitted but never activated |

## Summary

P5.1 proved `slime-root`'s mechanism against a fixture the root task embedded at
compile time. This entry makes the generation's own payloads loadable: components
are rebuilt as native AArch64 ELF against the `sel4` transport, wrapped in a
target-qualified image revision, and launched from a generation that declares
them. Five components run with the grants their generation declares, and the
root answers the operation surface they invoke.

The scope is narrower than "the real service graph", deliberately. `slime-root`
mediates the task, IPC, supervision, and shared-buffer planes and does not own
storage, directory, input, generation management, or recovery —
`Operation::mediation` answers `Unavailable` for each, by design in this cutover.
A component invoking one of those cannot run here yet, so declaring it would
promise authority the root cannot honour. The five declared components are
exactly those whose whole surface the root answers; the rest are deferred with
the plane each waits on recorded in the roadmap.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Runtime | `components/runtime/src/runtime.rs` binds a transfer window at startup; `sel4_transport` gains the setter | `recv`/`spawn`/`wait` stage through a window and refuse to truncate — nothing bound one, so no component could receive a message |
| Loader | `child_vspace` maps one granule per child above its IPC buffer as that window | A component granted no `SharedBufferFactory` still has a window, without being handed allocation authority its generation never declared |
| Contract | `contracts/component/v2`: ELF-carrying revision under a distinct `SLIMECME` magic, same 56-byte qualification header | Which decoder owns the body is a decode-time fact, not an inferred convention (the v1→v2 reasoning) |
| Contract | `contracts/target-profile/v1`: `aarch64-sel4-qemu-virt` (id 5), ABI `SLIME_AARCH64_SEL4_V1`, feature `AARCH64_SEL4` | A distinct ABI because operations reach their implementation by `seL4_Call`, not `svc` into a Slime kernel |
| Admission | `component_image::admit_elf` yields the payload only after target admission | Invariant 9: a wrong-target executable is refused before mapping, by a check the caller cannot skip to reach the bytes |
| Manifest | `contracts/generation/v1/fixtures/sel4.zti` + `sel4.md` | A sibling manifest, so `valid.zti` and the frozen 45-slot product generation are untouched |
| Builder | `build-generation.py`: seL4 build path, JSON-target handling, ELF image emission | The three silent JSON-target hazards — output dir by stem, `-Z` flags and environment, rustflags `.cargo/config.toml` cannot supply |
| Root task | `slime-root/src/graph.rs`: per-task logical capability tables | Grants are logical slot numbers the root resolves, so a child CSpace stays at four slots and a component cannot forge a slot it was not granted |
| Root task | `slime-root/src/transfer_window.rs`: per-task window registry | A bind is checked against what the loader mapped, not accepted on the caller's word |
| Root task | `main.rs` launches `admission.loadable_plans()` and serves their operation surface | Components are built from their own generation objects, not from one embedded fixture |
| Harness | `--component-graph` builds a second image; `just sel4_component_graph_check` | Each gate boots the artifact it asserts about, so neither invalidates the other's evidence |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A component cannot invoke the root at all | `Attempted to invoke a read-only endpoint` in `FAILURE_MARKERS` | Otherwise silent from the Slime side — the component simply never speaks |
| Grants widen to everything for everyone | Per-component `executables=` counts | `console`'s `executables=0` |
| A payload reaches the loader unqualified | `wrong_target=0 unrecognized=0` with `elf=5` | Marker mismatch |
| The window stops being bound | `[slime-rt] transfer window bind failed`, and the bind marker itself | Windowed operations return `ERR_INVALID_ARG` for reasons the component cannot see |
| The shared-buffer plane regresses | `[spawn-service] shared-buffer quota live` | spawn-service exits non-zero at startup |
| P5.1's evidence is disturbed | `just sel4_root_boot_check`, separate image | Its own ordered markers |
| The frozen x86 oracle regresses | `just test`, `just product_boot_check`, `just generation_check` | 191 assertions, 45-slot slice, identity `c181cc25…` |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| `just sel4_component_graph_check` | Pass | Direct |
| `just sel4_root_boot_check` | Pass — P5.1 unchanged | Direct |
| `just sel4_pin_check` | Pass | Direct |
| `just test` | Pass, 191 assertions — same as the P1 baseline | Direct |
| `just product_boot_check` | Pass — healthy 45-slot slice, none of the 17 scaffolding components | Direct |
| `just generation_check` | Pass — two byte-identical builds, x86 identity unchanged | Direct |
| `just contracts_check` | Pass | Direct |
| `just boot_layout_check` | Pass — 19 profile/layout pairs | Direct |
| `cargo test -p boot-contracts --lib` | Pass, 97 tests (95 prior + 2 for the ELF revision) | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff` | Pass | Direct |
| seL4 generation built twice | Byte-identical, same identity | Direct |

Observed serial evidence, abridged to the load-bearing markers:

```text
SLIME_ROOT generation admitted number=1 components=5 grants=7 health=3 kernel=1 bootstrap=1
SLIME_ROOT graph admitted; legacy SLIMECM images not activated components=5 slimecm=0 elf=5 unrecognized=0
SLIME_GRAPH staged task=0 component=console       grants=0 executables=0 window=0x236000 entry=0x2112f0
SLIME_GRAPH staged task=3 component=spawn-service grants=4 executables=2 window=0x237000 entry=0x2118b8
SLIME_GRAPH staged components=5 loadable=5 slimecm=0 wrong_target=0 unrecognized=0
SLIME_GRAPH activated components=5
SLIME_GRAPH window bound task=3 base=0x237000 len=4096
[spawn-service] ready
SLIME_GRAPH buffer created task=3 slot=3 id=1 pages=1
[spawn-service] shared-buffer quota live
SLIME_GRAPH unsupported operation task=3 operation=2 result=-4 caller_survives=1
[init] launching component graph
SLIME_GRAPH spawn refused task=2 slot=1 ungranted
[init] spawn failed slot=1 error=-4
SLIME_GRAPH served live=0 unsupported=4 buffers=5 windows=0 tables=0
```

### Fault injection

Both halves of the gate were confirmed to bite, then restored and re-verified:

| Injection | Result |
|---|---|
| Root endpoint rights derived from grants again (the pre-fix behaviour) | Fails on `Attempted to invoke a read-only endpoint` |
| Every executable granted to every component | Fails on `console staged with its declared grants` — `executables=0` |

## Decisions

- **Decision:** declare only the components whose entire operation surface the
  root mediates, and record the rest as deferred.
  **Rationale:** the other components invoke planes `Operation::mediation`
  answers `Unavailable`; building those planes is scoped nowhere in P5 and would
  put device and filesystem policy in the root task, against AGENTS.md.
  **Rejected alternative:** declare the full 25-component graph and let the
  unmediated components fail at runtime — that would make the gate assert a
  graph that cannot run.

- **Decision:** a sibling manifest rather than a boot profile of `valid.zti`.
  **Rationale:** `resolve_boot_profile` narrows by subtraction
  (`kept = (declared - scaffolding_everywhere) | set(scaffolding)`), so naming a
  component in a new profile removes it from `default`, changing the frozen
  product generation that `product_boot_check` and nineteen `boot_layout_check`
  pairs guard. Precedent: `recovery_manifest()` in the same builder.

- **Decision:** keep the `SLIMECM`-family wrapper and add an ELF-carrying
  revision, rather than emitting bare ELF as the object payload.
  **Rationale:** bare ELF carries no architecture, ABI, page profile, or feature
  mask, so invariant 9 would have no data to reject a wrong-target image with.

- **Decision:** the root service endpoint carries send unconditionally.
  **Rationale:** it is the component's *transport* — how it reaches `exit`,
  `debug_write`, and the window bind every other operation stages through — not
  one of its grants. Deriving its rights from inbound grants gave three
  components a read-only endpoint, and seL4 refuses a call on one, so they could
  not speak at all. Conveying no authority is what makes this safe: what a
  component may ask for is decided when the root dispatches, against the logical
  capability table holding exactly its declared grants.

## Open risks and follow-ups

- **`spawn` resolves but does not construct.** A resolved grant is answered with
  `UnsupportedOperation` rather than a child. The authority half is proven — the
  gate asserts both the grant and the refusal of an ungranted slot — but
  `spawn-service` cannot yet start `sysinfo` or `echo-agent`. That, and the
  `recv`/`send` channel plane behind it, is P5.3.
- **`ipc.rs`'s `Channel`, `WaitSet`, `send_atomic`, and `CapabilityTransfer`
  remain unwired.** They are written and unit-tested; P5.3's channel plane is
  what will use them. The crate keeps `#![allow(dead_code)]` until then.
- **`components/.cargo/config.toml` carries a stale `--remap-path-prefix`**
  naming `/home/iceice666/projects/slime_os` while the checkout is
  `slime_os-sel4-cutover`. It is a prefix of the real path, so it mangles rather
  than misses. The seL4 build passes its own rustflags and is unaffected; the
  x86 oracle's determinism claim rests on a path that no longer exists. Left
  untouched here because changing the frozen oracle's build inputs for a
  tangential defect is the larger risk — worth a backlog item.
- **Each component maps a whole granule for a window it may barely use.** Fine
  at five components; worth revisiting if a graph grows large.
- **The `MAX_COMPONENT_ELF_BYTES` staging buffer is 512 KiB of `.bss`** against
  a largest current component of ~44 KiB. Bounded and static (B3's lesson), but
  sized by guess rather than by measurement.

## Artifacts and provenance

- Generation: `build/sel4-generation/generation.bin`, identity
  `84b4963d87e1ccb8e36cfa663a39adb305789b7434d37ef8abfead94492476c3`,
  target `aarch64-sel4-qemu-virt`, five `SLIMECME` images.
- Image: `build/slime-sel4-graph.elf` with
  `build/slime-sel4-graph.identity.json` (`component_graph: true`).
- P5.1's artifacts are unchanged at `build/slime-sel4.elf` and
  `build/slime-sel4.identity.json`.
- Rationale for the manifest's shape, which `.zti` cannot carry inline because
  the parser rejects comments: `contracts/generation/v1/fixtures/sel4.md`.
