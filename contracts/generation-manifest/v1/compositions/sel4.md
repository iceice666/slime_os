# `sel4.zti` — the `aarch64-sel4-qemu-virt` generation manifest (P5.2)

Rationale for the sibling fixture. It lives here rather than in the manifest
because `.zti` data files are comment-free: the parser rejects `--` both at top
level and inside a record, so a fixture cannot explain itself.

## Why a sibling fixture, not a boot profile in `valid.zti`

`valid.zti` describes the graph the retired custom kernel booted, and it remains
the frozen regression manifest: `components/build-support/src/lib.rs` and
`scripts/lib/interface_schema.py` still read it for build-time command and
fabric profiles. Its `bootProfiles` mechanism resolves a component set by
**subtraction** — `resolve_boot_profile` in `scripts/build/build-generation.py`
computes

```python
kept = (declared - scaffolding_everywhere) | set(scaffolding)
```

so every component no profile claims is product surface. Naming a component in a
new profile there would *remove* it from `default`, changing the frozen product
generation that `just product_boot_check` and the 25 `just boot_layout_check`
plane layouts guard as the regression oracle.

A separate manifest leaves `valid.zti` byte-for-byte untouched. The precedent is
`recovery_manifest()` in the same builder, which likewise derives a second,
narrower generation rather than adding a mode to the first. Every seL4 plane
fixture in this directory follows the same rule, which is why they are per-scenario
siblings rather than profiles of one graph.

## Why these six components

`slime-root` mediates the task, IPC, supervision, input, directory, and
shared-buffer mechanisms this first resident product graph uses. The graph is:

| Component | Role |
| --- | --- |
| `init` | bootstrap; launches the resident services and supervises them |
| `console` | receives Slisp output on a native endpoint |
| `spawn-service` | owns the executable catalogue entries the shell may later invoke |
| `slisp` | resident shell; waits for input and evaluates pure S-expressions |
| `sysinfo` | spawned application; reports its launch context |
| `echo-agent` | spawned application; echoes its launch context |

The product input source is intentionally empty until a hardware input driver
supplies events. Empty input reports `WouldBlock`, so Slisp remains at its prompt
instead of treating boot as a completed scripted session. The component-graph
gate terminates on Slisp's first blocked input wait after the root certifies that
`init`, `console`, `spawn-service`, and `slisp` are all live and supervised.

Deferred components and the plane each waits on are recorded in
`roadmap/07-architecture-portability.md`.


## Authority path

`init` receives executable authority for `console`, `spawn-service`, and
`slisp`. It launches them and retains their supervision handles. Native
generation-owned endpoints connect Slisp to console and spawn-service; Slisp
also receives its declared input authority.

`spawn-service` receives executable grants for `sysinfo` and `echo-agent`, so a
shell command still resolves through the generation's command profile and can
start exactly those two executables. The resident product does not infer command
authority from the executable catalogue.
