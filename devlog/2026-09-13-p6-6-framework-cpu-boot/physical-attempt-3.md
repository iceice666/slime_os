# Physical boot attempt 3 — operator report

Immutable record of the third boot attempt on the named Framework, with the
framebuffer leak fixed, `fatal!` reporting to the panel, and graph launch
instrumented. Frozen — corrections append.

## Reported observation

Attempt 2's four lines, plus three:

```
SLIME OS - ROOT RUNNING
TIMER MAPPED - AWAITING TICK
TIMER TICKING
GENERATION ADMITTED
IMAGES STAGED
ENDPOINTS WIRED
COMPONENTS RUNNING
```

## What this settles

**The framebuffer CSlot/provenance leak was the cause of attempt 2's stop.**
This was recorded as unconfirmed — the leak cost 65 slots of ~3000 free at
1280x800x32, which was not obviously fatal, and the Framework's real panel mode
could not be reproduced under QEMU. Attempt 3 confirms it: the only changes
affecting graph launch were the leak fix and the added markers, and launch now
runs to completion.

It also follows that the real panel is large enough for the per-granule leak to
have exhausted an allocator bound, which is indirect evidence that the mode is
considerably bigger than anything QEMU's std VGA or virtio-vga offered. The
exact geometry is still unrecorded, because the boot does not reach the
`DISPLAY` line.

Newly confirmed on hardware:

| Claim | Status |
|---|---|
| Child VSpace construction and ELF staging for all six components | **Confirmed** (`IMAGES STAGED`) |
| Peer endpoint materialization across the declared graph | **Confirmed** (`ENDPOINTS WIRED`) |
| Component threads and the console dispatcher start | **Confirmed** (`COMPONENTS RUNNING`) |

No `ROOT FATAL - BOOT ABANDONED`. Since a *required* instance faulting or
exiting non-zero calls `fatal!`, and `fatal!` now renders, neither happened.

## What this does not settle

`COMPONENTS RUNNING` was the **last marker present in those bytes**. The boot
did not necessarily stop there — it ran out of instrumentation. The distinction
matters: attempt 3 narrows the stop to "at or after the start of the service
loop", not "in graph launch".

## Change made in response

One further marker, and two for events that can stall certification silently:

- `SERVING - AWAITING GRAPH`, rendered immediately before the dispatcher's
  first blocking `seL4_Recv`. Without it, a graph whose components never send
  is indistinguishable from one that crashed entering the loop: both leave
  `COMPONENTS RUNNING` as the final line.
- `COMPONENT FAULTED` / `COMPONENT FAULT UNDECODABLE`, for a *non-required*
  component fault. That path is not fatal by design, so it printed only to
  serial, but it can leave the graph permanently uncertified.
- `COMPONENT EXITED NONZERO`, likewise, for a non-required component exiting
  with a failure status.

The product graph has no request-iteration limit — its authenticated action
declares a resident service — so an uncertified graph spins in `seL4_Recv`
forever rather than failing. That is the behavior these three markers make
visible.

The full chain is fourteen lines, verified rendered under QEMU by decoding a
`screendump` against the font table.

## Ruled out this round

seL4's `XSAVE_SIZE` is pinned at 576 with `XSAVE_FEATURE_SET = 3`, sized for
the Haswell model QEMU emulates, while the Ryzen AI 300 is a Zen 5 part with
much wider vector state. That looked like a plausible hardware-specific
divergence and is not one: `Arch_initFpu` writes `xcr0 = desired_features`
*before* reading `CPUID.0Dh:EBX`, and that leaf reports the save-area size for
the features currently *enabled* rather than the maximum supported. With only
x87+SSE enabled the area is 576 bytes on Zen 5 as well. A mismatch would also
fail `Arch_initFpu`, halting the kernel long before `ROOT RUNNING` rendered.

## Next attempt

| Last line on panel | Conclusion |
|---|---|
| `COMPONENTS RUNNING` | Stops between the last launch step and the dispatcher's receive — a fault inside `serve_instance_graph`'s entry, not a typed error |
| `SERVING - AWAITING GRAPH` | The dispatcher is blocked in `seL4_Recv` and no component ever sent. The graph never certifies because nothing arrives to trigger the accounting |
| `COMPONENT FAULTED` / `COMPONENT FAULT UNDECODABLE` | A non-required component faulted; the fault kind is on serial, which this machine does not have, so the next step is rendering the kind |
| `COMPONENT EXITED NONZERO` | A non-required component failed cleanly |
| `ROOT FATAL - BOOT ABANDONED` | A required instance faulted or exited; the preceding marker names the phase |
| `IDLE - SAFE TO POWER OFF` | P6.6's exit condition is met |

The image to boot is the rebuilt one; attempt 3's bytes are superseded.
