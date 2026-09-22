# Rubik Pi 3 bring-up

Where the QCS6490 port stands, what is proven, and what the next person should
do first. The canonical record is work item
`01a0afe7-fb86-7693-857f-a68bd97f0e71`; this page is the operating summary.

## What is true

Slime OS boots on the board. `slime-root` has run to `SLIME_ROOT READY` on real
hardware with timer, IPC, shared-buffer rights, private-memory quota, fault
isolation, and reclamation markers all observed, and an idle run has stayed up
for a quarter of an hour.

That is a CPU and root-mechanism claim about one machine. No component has
reported ready on the board, no cold boot has been repeated, and no device of
any kind is supported.

## How to reach the board

The board runs stock Ubuntu. Control passes to Slime by `kexec` from that
Linux; the firmware loads no Slime file, so nothing on internal storage is
modified and a reset always returns to Ubuntu.

```
just sel4_rubikpi3_image_check          # kernel, loader, packaged image, pins
scp build/rubikpi3/slime-sel4-rubikpi3.img ubuntu@<board>:/tmp/
ssh ubuntu@<board> sudo kexec -c --type=Image --load /tmp/slime-sel4-rubikpi3.img \
    --command-line='' --mem-min=0xa0400000 --mem-max=<the image metadata's kexecMemMax>
ssh ubuntu@<board> sudo kexec -e
```

The exact bounds are not folk knowledge: the packager writes them beside the
image as `requiredKexecArguments`, derived from that image's own size. Use
those, not the ones in this paragraph.

Two properties of the loading path are deliberate and worth not re-deriving.
The loader is linked at an absolute address, so the arm64 header does **not**
set the placement flag and `--mem-min` equal to the image base is what pins it:
the legacy loader searches bottom-up, so one candidate remains. And the window
is derived per image rather than fixed, because `kexec` places the device tree
and purgatory *after* the image and validates every segment against `mem-max`
after rounding each to a page.

Serial is the GENI debug UART at 115200. `capture-timed.py` in the operator's
recovery tree records a receive timestamp per chunk; use it rather than a plain
capture, because transcript length is not elapsed time on a polled console —
that mistake produced a wrong conclusion once already.

## The open problem

The board raises a PMIC `PSHOLD` warm reset **10.13-10.15 seconds after kernel
entry** when the system is still working. An idle system survives indefinitely.

The measurement that matters: two workloads sharing no components — the product
component graph and `sel4-sample` — reset at 10.13 s and 10.15 s from kernel
entry, within twenty milliseconds of each other, while their offsets from first
spawn differ by nearly a factor of two (5.83 s and 3.44 s). The deadline is
anchored at kernel entry, not at anything the workload does.

Four explanations are refuted by measurement, not argument:

| Hypothesis | Refutation |
| --- | --- |
| Platform watchdog | A bare payload ran 1,475 s; the APSS watchdog enable register reads zero throughout, under Linux and under the payload |
| Idle entry | A probe sat in a bare `wfi` loop and did not reset |
| Memory access | A probe wrote and read back every 4 KiB page of the declared window with no mismatch |
| The console | A bare payload emitted 9,040 lines over sixty continuous seconds and then reset itself on schedule |

One observation sits awkwardly against any simple story and should be kept in
view: a bare payload spinning with the MMU **off** ran twenty-five minutes. What
the bare probes never do, and seL4 always does, is run with stage-1 translation
and caches enabled, program the interrupt controller, and take periodic timer
interrupts.

### What to try next

A progression of bare-metal probes, each staying busy past fifteen seconds and
each resetting itself through PSCI on a deadline: add the MMU and caches, then
the GIC, then a periodic timer interrupt. Whichever step first reproduces a
reset at ten seconds identifies the mechanism.

Make every diagnostic payload self-reset. The first two probes looped forever
and cost the operator three manual power cycles; the later ones reset themselves
and recovered the board unattended.

## Decisions already made, and why

**The board is a distinct target profile.** `aarch64-sel4-rubikpi3` shares the
AArch64 seL4 userspace ABI with QEMU `virt` and nothing else. An executable
built for one is refused on the other, which is the point.

**The loader lives on its own branch.** `deps/rust-sel4-rubikpi3` diverges
directly from the shared base, carrying only the platform arm and its console —
the same shape the Pi 5 and the Duo use, so no board's patch reaches another.

**The declared RAM window is conservative.** `0xa0200000..0xb9700000`, about
407 MiB. The fragmented low firmware and subsystem ranges are withheld until
they are separately qualified. Expanding it is a deliberate change with its own
evidence, not a tuning knob.

**The composition is closure-exempt.** A closure resolves against a committed
seL4 prefix snapshot, and only the two QEMU reference platforms commit one —
the Pi 5 and the Duo have no closure either. Both the generator and the
independent builder check carry that declaration.

**The kernel has no SMC capability.** `KernelAllowSMCCalls` is off, so a root
task cannot invoke PSCI and a Slime image cannot reset the board itself. Turning
it on would make physical iteration self-recovering; it also widens initial root
authority, so it is a decision rather than a convenience.

## Traps

- `just contracts_check` **emptied the worktree it ran in** — `flake.nix`,
  `.git`, the `Justfile`, and several directories were gone afterwards. The
  commits survived in the shared object store. This is unrelated to this port
  and deserves its own investigation; until then, do not run it in a worktree
  whose uncommitted state matters.
- The pc99 kernel ELF does not reproduce on every host from clean `main`. It is
  left at its existing pin on purpose; re-pinning it from a machine that
  disagrees would bury someone else's finding.
- Declaring a platform in the seL4 fork moves `kernel_config_sha256` for every
  platform, by one line recording the new platform as disabled. That is expected
  and must be re-pinned in the same change; anything else moving is a finding.
- A freestanding C component compiles against the selected platform's prefix.
  Two AArch64 platforms once shared one architecture-keyed output path; they no
  longer do.

## Evidence

Serial captures live in the operator's recovery tree, outside this repository,
with their hashes recorded in the work item: the first seL4 boot, the first
`slime-root` boot through `READY`, the truncated component-graph runs, the
memory walk, and the console burn.
