# `sel4.zti` — the `aarch64-sel4-qemu-virt` generation manifest (P5.2)

Rationale for the sibling fixture. It lives here rather than in the manifest
because `.zti` data files are comment-free: the parser rejects `--` both at top
level and inside a record, so a fixture cannot explain itself.

## Why a sibling fixture, not a boot profile in `valid.zti`

`valid.zti` describes the graph the retired custom kernel boots. Its
`bootProfiles` mechanism resolves a component set by **subtraction** —
`resolve_boot_profile` in `scripts/build/build-generation.py` computes

```python
kept = (declared - scaffolding_everywhere) | set(scaffolding)
```

so every component no profile claims is product surface. Naming a component in a
new profile there would *remove* it from `default`, changing the frozen 45-slot
product generation that `just product_boot_check` and the nineteen
`just boot_layout_check` fixture pairs guard as the regression oracle.

A separate manifest leaves `valid.zti` byte-for-byte untouched. The precedent is
`recovery_manifest()` in the same builder, which likewise derives a second,
narrower generation rather than adding a mode to the first.

## Why these five components

`slime-root` mediates the task, IPC, supervision, and shared-buffer planes. It
deliberately does **not** own the storage, directory, input,
generation-management, or recovery planes — `slime-root/src/ipc.rs`'s
`Operation::mediation` answers `Mediation::Unavailable` for each, with the
comment that they *"have no seL4 mechanism owner in this cutover"*.

A component invoking one of those cannot run here yet, so declaring it would
promise authority the root cannot honour. These five are exactly the components
whose entire operation surface the root answers:

| Component | Role |
| --- | --- |
| `init` | bootstrap; spawns the graph and wires its channels |
| `console` | drains a channel to the debug log |
| `spawn-service` | spawns declared executables with role capabilities |
| `sysinfo` | spawned application; reports its launch context |
| `echo-agent` | spawned application; echoes its launch context |

Deferred components and the plane each waits on are recorded in
`roadmap/07-architecture-portability.md` under P5.2.

## Two deliberate differences from `valid.zti`

- **`console-output` is retargeted.** In `valid.zti` the channel's producer is
  `dango`, which this profile does not declare. `spawn-service` is the component
  that reports here, so the grant is retargeted rather than dropped; the right
  is unchanged, so `console` still holds exactly a receive end.
- **`kernelObject` names a payload that is never mapped.** seL4 is the kernel,
  pinned and built separately under `sel4/pins.toml`. The object is declared
  because the generation format requires exactly one and the root task re-checks
  that closure at admission — but nothing loads it.

## What makes the grant claim behavioural

`init` does not spawn `sysinfo` and `echo-agent`. It grants their executables to
`spawn-service` (`exec | spawn`), which spawns them with role capabilities. So
"launches its declared components with their declared grants" is observable —
`spawn-service` can start exactly the two executables the generation names, and
nothing else — rather than a self-report.
