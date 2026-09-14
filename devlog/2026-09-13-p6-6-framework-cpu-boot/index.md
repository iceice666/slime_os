# P6.6: Slime OS boots on the Framework, and the boot leaves checkable evidence

| Field | Value |
|---|---|
| Date | 2026-09-13 |
| Kind | Change |
| Status | Monitoring |
| Scope | `deps/sel4` (`src/arch/x86/multiboot.S`), `sel4/config/qemu-pc99.cmake`, `sel4/pins.toml`, `scripts/check/check-sel4-pins.py`, `scripts/lib/pc99_media.py`, `slime-root/src/{framebuffer,glyph_font,boot_record,device}.rs`, `slime-root/src/{lib,main}.rs`, `slime-root/src/graph_runtime{,/services}.rs`, `contracts/cpu-boot-observation/v1/schema.zt`, `scripts/lib/cpu_boot_observation.py`, `scripts/check/check-framework-cpu-boot.py`, `scripts/generate/generate-cpu-boot-observation-bindings.py`, `scripts/check/check-contracts.py`, `just/{product,generate,quality}.just`, `_typos.toml`, `AGENTS.md`, `evidence/framework-cpu-boot/` |
| Work items | 01a09e12-1274-7a7f-b757-f86b5854f667 |
| Gates | `just framework_cpu_boot_check`, `just framework_media_check`, `just contracts_check`, `just test_sel4_root`, `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` |
| Trigger | P6.6 requires readiness evidence through GOP on a machine whose only other channel does not exist, and a gate that can judge a physical observation; then six physical attempts, four of which exposed defects no emulated plane could see |
| Baseline | P6.5's identity-bound raw GPT/FAT32 image, booting under pinned q35/OVMF to the resident Slisp graph with serial as its only evidence channel |

## Summary

P6.6's exit condition is a physical boot, and this entry closes it: the named
Framework cold-booted the QEMU-proven removable image twice, reached the
resident product graph's declared terminal state both times, and left the
protected region of its internal NVMe byte-identical. The observation is
recorded in `evidence/framework-cpu-boot/slime-cpu-boot.observation.json`.

Getting there took six physical attempts, and the work divides in two. First,
two reasons the milestone could not even be *attempted*. The Framework 13 has
**no serial port**, so the ordered marker chain every emulated plane reads had
no channel on the target machine and a blank panel could not be distinguished
from a hang; the root now renders a bounded readiness record onto the
firmware's linear framebuffer, verified pixel-by-pixel under QEMU. And a
physical claim had no judge: there was no contract for the observation record,
no pre/post digest of a protected internal region, and no gate. There are now
all three, and the gate's validator refuses nineteen distinct forgeries over
the approved medium before reporting the recorded claim.

Second, four defects that only a real machine could expose — every one of them
in this milestone's own code, and every one invisible to the emulated planes.
They are worth reading as a set, because they share a shape: each was a
behavior the emulator could not distinguish from correct.

**First physical attempt (same day).** The image was booted on the machine and
stopped in GRUB, showing `Booting slime os` then `Trying to terminate EFI
services again` — both GRUB's own strings, reached before seL4 runs. That
attempt is recorded verbatim in
[`physical-attempt-1.md`](physical-attempt-1.md). It exposed a real defect in
the work above: readiness rendered only at the *end*, so any earlier stop left
the panel showing whatever the bootloader last printed and was
indistinguishable from a bootloader hang. The panel is now claimed immediately
after the allocator and each phase reports as it passes, so the next attempt
names where it stops. It also showed that GRUB's messages reached the panel
*by fallback* — the configuration named serial as the only terminal, and the
machine has no 16550 — so the console is now declared as both.

**Second physical attempt.** With the staged markers, the panel reported four
lines — `SLIME OS - ROOT RUNNING`, `TIMER MAPPED - AWAITING TICK`, `TIMER
TICKING`, `GENERATION ADMITTED` — then stopped. Recorded in
[`physical-attempt-2.md`](physical-attempt-2.md). This is the x86-64 lane's
first physical evidence of anything above the bootloader, and it settles a
great deal: the GRUB Multiboot2 handoff completes on this machine (refuting
attempt 1's looping hypothesis outright), upstream seL4 pc99 boots natively on
a Ryzen AI 7 350, the firmware negotiates a linear framebuffer that the kernel
exports and the root decodes and renders at the panel's real mode, **the pc99
HPET assumption holds** — the predicted most-likely failure, and it is not
one — and generation admission succeeds against the physical target profile.
The boot stops in graph launch.

It also exposed three defects, all in this change's own code: the framebuffer
walk leaked a CSlot and a physical-provenance record per granule (baseline
live slots 54 → 119 at 1280x800x32, against a provenance table sized 448 for
the live DMA population); `fatal!` printed only to serial, so all 56 fatal
sites in graph launch were silent stops on this machine; and the first fix for
the leak broke rendering outright, because deleting the last capability
derived from a device untyped makes seL4 reset its free index — the invariant
the allocator states at the one other place it walks one. That third defect
was caught only by an injected-fatal control, which the success path could not
see. Graph launch is now instrumented with three further markers so the next
attempt names a phase instead of a gap.

**Third physical attempt.** Three further lines — `IMAGES STAGED`, `ENDPOINTS
WIRED`, `COMPONENTS RUNNING` — recorded in
[`physical-attempt-3.md`](physical-attempt-3.md). This **confirms the leak was
the cause of attempt 2's stop**, which had been recorded as unconfirmed: the
only changes touching graph launch were the leak fix and the markers, and
launch now runs to completion. It also implies the real panel is large enough
for a per-granule leak to exhaust an allocator bound, so the physical mode is
considerably larger than anything QEMU offered — the exact geometry is still
unrecorded, because the boot does not reach the `DISPLAY` line. Child VSpace
construction, ELF staging for all six components, peer endpoint
materialization, and component and console-dispatcher start are now all
confirmed on hardware.

What attempt 3 does *not* settle is where it stops: `COMPONENTS RUNNING` was
the last marker those bytes contained, so the boot ran out of instrumentation
rather than necessarily stopping there. One marker now precedes the
dispatcher's first blocking `seL4_Recv` (`SERVING - AWAITING GRAPH`), and two
more report the events that can leave the graph permanently uncertified
without being fatal — a *non-required* component faulting or exiting non-zero,
both of which previously printed to serial alone. The product graph has no
request-iteration limit, so an uncertified graph waits forever instead of
failing; these markers make that visible.

**Fourth physical attempt: the boot ran the whole chain.** Every stage marker
and every readiness line rendered on the Framework, ending at `IDLE - SAFE TO
POWER OFF`. Recorded with the operator's photograph in
[`physical-attempt-4.md`](physical-attempt-4.md). The service loop reaches its
receive, the graph certifies healthy, and the root reaches its declared
terminal state — all first-time physical evidence. The panel's real mode is on
record for the first time: **`2256x1504x32`**, the Framework 13's native
resolution, which no QEMU boot reproduced (std VGA gives 24 bpp, virtio-vga
32 bpp only at 1280x800). At 3314 granules that mode is 7.4x the 448-entry
provenance bound, which retrospectively confirms why attempt 2's per-granule
leak stalled graph launch.

It also exposed a fourth defect in this change's own code, and a
characteristic one: the record rendered **illegibly**. `write_line` wrote only
the *set* glyph bits, so GRUB's own console text remained in the gaps —
`Trying to terminate EFI services again` overlapping the record, `TIMER
TICKING` unreadable, and the generation digest photographing as
`41?1?19329F39BF5FA1F` instead of `0F1929F39BF5FA1F`. Every QEMU framebuffer
starts black, so thirteen prior emulated captures decoded perfectly and the
defect was structurally invisible to them. A digest that can be misread cannot
bind a boot to a generation, which is exactly what the observation contract
requires of it. Each line now paints its full cell band, background included,
in the same ascending-offset pass. The operator's follow-up showed that
incomplete: the glyph backgrounds were correct but GRUB residue survived in
the screen *corners*, because painting the cell band leaves the margins and
every row below the record untouched. Each line now clears whole screen rows,
the first also clears the top margin, and the terminal line clears the tail —
all inside the one ascending pass the monotonic retype permits, which is why
the tail is cleared last rather than the screen cleared first. An emulated
800x600 capture now has 0 of 480000 pixels that are neither black nor white.

**Fifth physical attempt: the record is legible.** The corners are clean and
independent inspection of the photograph reads the generation digest character
by character as `0F1929F39BF5FA1F` with no ambiguous position — the field that
attempt 4 rendered as `41?1?19329F39BF5FA1F`, and the one that binds a boot to
a generation. No proportional or lowercase text remains anywhere on the panel.
Recorded in [`physical-attempt-5.md`](physical-attempt-5.md). `prepare` ran
against this exact image, so image digest, USB read-back, recorded generation
prefix, and the panel now all agree, and the machine and firmware facts were
read from the host's own DMI rather than typed.

P6.6 nevertheless remains open on one input: the contract fixes
`requiredBoots = 2`, and only one cold boot has been observed. A draft record
built from this observation was run through the validator to confirm it
refuses — `observation records 1 boots, but the contract requires exactly 2` —
with every other check passing on the way to that refusal. Two is fixed rather
than a minimum so a run retried until it passed cannot be recorded as a
success. Attempt 5's image is not superseded: nothing in the root changed
after it, so the next boot needs no reflash.

**Sixth physical attempt: the observation.** The operator booted the same image
a second time, saw the same fourteen lines, and ran `verify`. The protected
region's post-boot digest came back `a20de77e945f02d3…` — byte for byte the
pre-boot value, with the page cache evicted on both reads so a cached read
cannot report a match the disk does not support. The record is
`evidence/framework-cpu-boot/slime-cpu-boot.observation.json`, identity
`2ab15f9a63b7e1eb…`, recomputed independently from its own stored fields and
matching. Recorded in [`physical-attempt-6.md`](physical-attempt-6.md).

`just framework_cpu_boot_check` now refuses its nineteen forgeries and then
reports `2 cold boots of image 95a0d213c1249cc7… observed on Framework Laptop
13 (AMD Ryzen AI 300 Series) (firmware 03.04), internal region
a20de77e945f02d3… unchanged`. P6.6's exit condition is met, which unblocks H1.

What that does *not* claim is set out in the sixth attempt's own record: no
device is qualified, the display is not a driver, the pc99 kernel
configuration is observed to work here rather than shown correct for this
machine, Secure Boot was disabled for an unsigned image, internal storage was
protected rather than used, and two boots on one unit say nothing about
another.

The enabling defect was upstream and in our own fork: seL4's Multiboot2 header
carried only an end tag, so its existing `MULTIBOOT2_TAG_FB` handling and
`SEL4_BOOTINFO_HEADER_X86_FRAMEBUFFER` export were unreachable on this boot
route no matter what the bootloader was asked to do.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `deps/sel4` `src/arch/x86/multiboot.S` | Emit the Multiboot2 framebuffer request tag (type 5) when `KernelMultibootGFXMode` is `linear`, reusing the width/height/depth options the Multiboot1 header already encodes | A kernel that handles and re-exports a framebuffer record actually asks for one; a machine with no serial port has a console |
| `sel4/config/qemu-pc99.cmake` | Select `linear` at depth 32 with no geometry preference | The mode is requested by the same kernel config both x86-64 profiles share, so QEMU proves the path the Framework takes |
| `scripts/lib/pc99_media.py` | `insmod all_video`, `gfxmode=auto`, `gfxpayload=keep` instead of `gfxpayload=text` | The bootloader negotiates a mode through EFI GOP; the header tag alone does not load GRUB's video drivers |
| `sel4/pins.toml`, `check-sel4-pins.py` | Pin the five video modules, the new module-set digest, the fork commit, the kernel/config digests, and declare the four graphics overrides | No undeclared kernel configuration or unpinned bootloader behavior reaches a boot claim |
| `slime-root/src/framebuffer.rs` | Decode and range-check the extra-BootInfo record, then map the mode one granule at a time through a claimed scratch page | Firmware-supplied geometry is validated, never trusted; an unsupported mode disables rendering instead of writing into whatever lives there |
| `slime-root/src/glyph_font.rs`, `boot_record.rs` | An 8x8 `const` font and a fixed-capacity line builder rendering profile, generation, mode, and terminal state | The panel carries the same facts as the serial chain, in bounded form, with truncation instead of overflow |
| `slime-root` wiring | `RuntimeDevices.display` threaded to the readiness site; rendered after the serial markers and reported rather than fatal | A display failure cannot suppress the chain an emulated plane reads, and cannot kill a healthy graph |
| `contracts/cpu-boot-observation/v1` | Zutai schema for the persisted observation: medium binding, write target, machine/firmware, protected region, and per-boot panel lines | A physical evidence format has one versioned source of truth rather than ad-hoc JSON |
| `scripts/lib/cpu_boot_observation.py` | Canonical identity, sysfs disk description, cache-evicting region hashing, and record validation | The checkable parts of a physical claim are checked; the operator-supplied parts are bounded and recorded verbatim |
| `scripts/check/check-framework-cpu-boot.py` | `prepare`/`verify`/`controls` and a default gate, reusing the existing guarded writer | The milestone has a fail-closed judge that cannot fabricate the boot it judges |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| A future kernel or GRUB change silently returns the boot to text mode, leaving the Framework unobservable | `just framework_media_check` | `SLIME_DISPLAY absent …` instead of `SLIME_DISPLAY ready rendered` in the media gate's boot transcript |
| Undeclared kernel graphics configuration or an unpinned GRUB module set | `just sel4_pin_check` (via `framework_media_check`) | `qemu-pc99 CMake config is incomplete` or a module-digest mismatch |
| A forged, self-inconsistent, or medium-mismatched observation record is accepted as physical evidence | `just framework_cpu_boot_check` | The reference record is refused, or a mutation is accepted and the gate fails with "the observation gate has no teeth" |
| A recorded boot names a generation the approved medium does not carry | `just framework_cpu_boot_check` | `boot N shows generation …, but the approved medium carries …` |
| An internal-storage write during the boots goes unnoticed | `just framework_cpu_boot_check` `verify` | Pre/post digests of the 16 MiB region differ and `verify` refuses |
| Rendering regressions in the record builder or mode validation | `just test_sel4_root` | The asserted 225-test count fails, or one of the eight new tests fails |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Multiboot2 header decode of the rebuilt `kernel.elf` | pass — valid checksum, tag type 5 (w=0 h=0 depth=32), then end tag | Direct |
| Pinned q35/OVMF boot of the exact raw image, with a display attached | pass — `Got framebuffer info in multiboot2 … 800x600@24 type=1`, then `SLIME_DISPLAY mode=800x600x24 paddr=0x80000000`, `SLIME_ROOT READY target_profile=x86_64-sel4-framework13-ai300`, `SLIME_DISPLAY ready rendered`, `slisp>` | Direct |
| QEMU `screendump` of the panel, decoded pixel-by-pixel | pass — six legible lines; the rendered `GENERATION 1 ID 0F1929F39BF5FA1F` matches the build manifest's `0f1929f39bf5fa1f`, an independent cross-check that the panel names the generation the medium carries | Direct |
| `just framework_media_check` | pass — two 68157440-byte builds byte-identical, image `dc19e39fbf38fa69…`, identity `aad1baa0caa778e0…`; all seven media and six writer negatives refused; raw image boots to `slisp>` | Direct |
| `just framework_cpu_boot_check` | pass — reference record accepted, 19 mutations refused, then **reports P6.6 OPEN** because no observation is recorded | Direct |
| `python3 scripts/build/write-removable-image.py … /dev/sda --dry-run` | pass — `/dev/sda` accepted as a safe removable whole disk (USB Flash Drive, 15518924800 bytes) against image `dc19e39f…` | Direct |
| `just contracts_check` | pass — including the new `cpu-boot-observation/v1` schema and binding freshness | Direct |
| `cargo test -p slime-root --lib` against the pc99 prefix | pass — 225 tests, including the eight new `framebuffer`/`boot_record` tests | Direct |
| `just fmt_check_all`, `just lint_all`, `just ruff`, `just typos` | pass | Direct |
| `just test_sel4_root` | not run to completion — the recipe requires `build/sel4-prefix`, which holds a stale AArch64 install; it fails identically at `HEAD` with these changes stashed, so the failure is pre-existing and unrelated | Direct (pre-existing failure, confirmed by bisect against a clean tree) |

## Decisions

- **Decision:** Fix the missing framebuffer request tag in `deps/sel4` rather than working around it in the bootloader configuration.
  **Rationale:** The kernel already parsed `MULTIBOOT2_TAG_FB` and already exported it as an extra-BootInfo record; only the request was absent, so every consumer of that record was dead code on this boot route. Two empirical probes confirmed that neither `gfxpayload=keep` nor an explicit `gfxmode=1024x768x32` produces the tag without it. The fork already carries platform work, and the change is gated behind an existing upstream config option.
  **Rejected alternative:** Reading the VBE record, or having GRUB set a mode and passing the address out of band — both would let the root consume geometry no contract describes.

- **Decision:** Enable the linear mode for both x86-64 profiles, not just the Framework one.
  **Rationale:** The Framework build deliberately shares the pc99 kernel and config. A Framework-only graphics setting would ship a display path no gate had ever executed, which inverts the point of P6.4's parity corpus.
  **Rejected alternative:** A separate Framework kernel configuration, which would add an unobserved machine claim that H1 owns.

- **Decision:** Render strictly in ascending framebuffer offset, scanline-major across a whole text line.
  **Rationale:** Not style — correctness. A device untyped's retype is monotonic, so `allocate_device_frame` only walks forward. The first implementation drew glyph-by-glyph, returned to a lower offset at each new character, and was correctly refused with `DeviceFramePassed`. This was caught by the QEMU run, not by reasoning.
  **Rejected alternative:** Mapping the whole mode at once, which would need megabytes of standing window for pixels written once.

- **Decision:** The terminal state is a resident idle graph that says `IDLE - SAFE TO POWER OFF`, not an ACPI S5 poweroff.
  **Rationale:** The panel is the only evidence this boot produces, and powering off would clear it — a cleared display is indistinguishable from one that never rendered. P6.5's proven artifact is a graph that stays resident and serves; booting a variant that exits would not be booting those bytes. The roadmap asks for a bounded halt *or* shutdown. ACPI RSDP and a usable FADT are exported, so S5 remains available to a later milestone that wants it.
  **Rejected alternative:** Parsing `_S5_` out of DSDT AML to write `PM1a_CNT`, which would mean writing a guessed SLP_TYP to a hardware register for no evidentiary gain.

- **Decision:** Split the gate into `prepare`, `verify`, and `controls`, with the default target running the controls and reporting the claim's absence.
  **Rationale:** The gate cannot perform a cold boot from removable media, so pretending to be a single command would either block forever or quietly pass. Reporting `P6.6 is OPEN` keeps the milestone honestly unfinished while still proving the judging mechanism on any host, exactly as the Duo lane's `duo_gate_control_check` replays a committed transcript.
  **Rejected alternative:** A single command that skips when hardware is absent, which is the failure mode the Duo checkers explicitly refuse.

- **Decision:** Re-stamp each mutated control record's canonical identity before requiring refusal.
  **Rationale:** Without re-stamping, every control would fail on the digest alone and would prove only that the digest works — not that each individual invariant is enforced. An adversary with this tooling can re-stamp too, so that is the adversary each case must be refused against.
  **Rejected alternative:** Mutating without re-stamping, which yields nineteen vacuous passes.

- **Decision:** Add one new top-level checker rather than extending `check-framework-media.py`.
  **Rationale:** Verification discipline reserves new top-level checkers for genuinely new mechanisms. This is one: its inputs are operator-supplied evidence and real block devices rather than QEMU, and its execution spans a power cycle. It reuses `framework_media.validate_image` and the existing guarded writer rather than growing a second copy of either.
  **Rejected alternative:** A fourth mode inside the media gate, which would make a QEMU gate's success depend on physical hardware.

## Open risks and follow-ups

- [x] ~~P6.6 remains open on exactly one input: a second cold boot.~~ **Closed by attempt 6:** two cold boots of image `95a0d213c1249cc7…` observed, the protected region's post-boot digest read as `a20de77e945f02d3…` — identical to pre-boot — and the record written and validated. `just framework_cpu_boot_check` reports the claim rather than the milestone's absence.
- [ ] The observation record and its directory are written by `sudo`, so they land root-owned. They belong in the repository as committed evidence, alongside the `evidence/framework-inventory.jsonl` H1 expects, which needs a `chown` before staging. The gate reads them fine either way.
- [x] ~~Whether the framebuffer leak caused attempt 2's stop is unconfirmed.~~ **Resolved by attempt 3:** with the leak fixed, graph launch runs to completion, and the only other change touching launch was the markers themselves. It follows that the physical panel is large enough for a per-granule leak to exhaust an allocator bound, which is indirect evidence the real mode is much larger than QEMU's.
- [x] ~~The pc99 `XSAVE_SIZE = 576` pin is sized for Haswell and the Ryzen AI 300 is a Zen 5 part.~~ **Ruled out by reading `Arch_initFpu`:** it writes `xcr0 = desired_features` before reading `CPUID.0Dh:EBX`, and that leaf reports the area for *enabled* features, so 576 is correct on Zen 5 too. A mismatch would fail `Arch_initFpu` and halt the kernel long before `ROOT RUNNING` rendered.
- [ ] The `ExitBootServices` retry is not reproducible under the pinned emulator: OVMF 202605 succeeds first try, with zero occurrences of the message across every captured QEMU boot. No QEMU pass can refute it, so this is firmware behavior the gate structurally cannot cover.
- [ ] Removing the GOP mode switch to avoid perturbing the EFI memory map is **not** available as a workaround: with GRUB's video modules absent and only the kernel's header tag present, the kernel reports no framebuffer at all. The mode switch and the readiness channel are the same thing.
- [x] ~~If the next attempt stops at `TIMER MAPPED - AWAITING TICK`, the pc99 HPET assumption is wrong for this machine.~~ **Resolved by attempt 2:** `TIMER TICKING` rendered, so the assumed HPET delivers real interrupts on this machine. H1 still owns the actual ACPI/APIC/timer inventory; what is settled is only that this profile's timer works here.
- [x] ~~The Framework panel's physical mode is unmeasured.~~ **Recorded by attempt 4: `2256x1504x32`**, the native resolution, and the first hardware confirmation of the 32-bpp path. No QEMU configuration reproduced it: std VGA offers 24 bpp and virtio-vga 32 bpp only at 1280x800.
- [ ] `just test_sel4_root` cannot run until `build/sel4-prefix` is reinstalled for the host's architecture. Pre-existing, unrelated to this change, and worth a backlog entry if it is not already known.
- [ ] The Framework target still reuses the pc99 kernel configuration, including its HPET assumption and `KernelSupportPCID OFF`. H1 owns the real ACPI/APIC/timer inventory; the Ryzen AI 300 does implement PCID.
- [ ] Secure Boot is recorded as a field but not exercised. The image is unsigned, so a machine with Secure Boot enabled will refuse it; the record's `secureBoot` field exists to make that fact explicit rather than to claim support.

## Artifacts and provenance

- Panel evidence: [`qemu-panel.txt`](qemu-panel.txt) — the success path's fourteen lines decoded against the font table, with the whole-panel purity count (0 pixels neither black nor white) that the corner-residue fix is measured by; PPM `d07046f00533f9d8…`. The injected-fatal control's four lines are PPM `b6b689108a1c9fb3…`.
- Physical panel photographs: [`physical-attempt-4-panel.jpg`](physical-attempt-4-panel.jpg), SHA-256 `2ad9b8839a73b258b12e4ce3fd1f9816a00babe7e789464754a3b1585e98d1f5` — the complete chain, and the artifact the legibility defect was diagnosed from; and [`physical-attempt-5-panel.jpg`](physical-attempt-5-panel.jpg), SHA-256 `d3e34b99f0f15b179f2313ee4c4160348eee45878be0dd5bb2ffd8f62376b180` — the same chain rendered legibly, with the generation digest readable.
- Raw boot transcript: [`qemu-gop-boot.log`](qemu-gop-boot.log) — the full serial output of the raw-image boot that produced that panel.
- Physical attempts: [`physical-attempt-1.md`](physical-attempt-1.md) through [`physical-attempt-5.md`](physical-attempt-5.md) — the operator's reports from the named machine and what each settles. Attempt 2 carries the lane's first physical evidence above the bootloader, attempt 3 confirms graph launch completes, attempt 4 runs the whole chain and records the panel's real mode, attempt 5 makes the record legible.
- Fatal control transcript: [`qemu-fatal-control.log`](qemu-fatal-control.log) — the serial output of the boot whose generation magic was corrupted on the medium, producing `SLIME_ROOT FATAL generation rejected: BadMagic` and the panel's `ROOT FATAL - BOOT ABANDONED`.
- seL4 fork commit: `fea36b20f5b410288631c9cf1c6b090a4beac88e` (`feat(x86/boot): preserve framebuffer request`), pinned in `sel4/pins.toml`. The observed boot was built from the identical 17-line `src/arch/x86/multiboot.S` change carried by the pre-cleanse commit `d2d0b3d5`; the blob is byte-identical (`90219d6bb2ccf667…`), only its commit identity moved.
- Generated artifact: `build/framework-media/slime-framework.img` (not committed), SHA-256 `95a0d213c1249cc73e462d5bed9b9462bea577f63fcb33093c83e025b8a14303`, identity `104b5959704a69ed…`. This supersedes every earlier image in this entry; the full-screen clearing fix changes the root, so the bytes a next attempt must boot are these.
- Generation identity prefix, unchanged by this work: `0f1929f39bf5fa1f`.
- Related roadmap item: [P6.6](../../roadmap/07-architecture-portability.md#p66--framework-removable-media-cpu-boot).
- Corrects an identity digest recorded in [P6.5's entry](../2026-09-13-p6-5-framework-media/index.md#corrections).

## Corrections

- **2026-09-14 — the physical observation no longer binds to a buildable image.**
  The repository source was re-established as a clean baseline, which changed
  `slime-root.elf`, so the Framework media image this entry's evidence was
  observed against can no longer be rebuilt. Measured after the rebuild: the
  medium's `kernel.elf` is unchanged at `1ae6fd038c8f24ba…` — still equal to the
  pinned pc99 `kernel_sha256` — while the rebuilt media identity is
  `9a552d7350bbc633cfeaaf692f5273f51aa6cab69bbb3a2da60f811af8212c85` against the
  `104b5959704a69ed…` named by
  `evidence/framework-cpu-boot/slime-cpu-boot.observation.json`. The delta is in
  the root, not the kernel.
  `just framework_cpu_boot_check` therefore fails with `observation does not
  name the approved boot-media identity`, which is the gate enforcing its own
  rule that a record binding to changed bytes is not evidence.
  The recorded boot is not retracted — it happened, on the named machine, and
  the raw evidence in this folder stays frozen and unedited — but it is now
  evidence about a superseded image. Re-closing the physical claim requires a
  new operator boot of the rebuilt medium; no QEMU plane can substitute for it,
  and the observation JSON was not re-stamped to the new digests, because
  editing it would assert a boot nobody performed.
