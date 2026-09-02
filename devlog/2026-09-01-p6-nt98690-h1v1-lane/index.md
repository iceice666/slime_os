# P6 — opening a second physical AArch64 lane on the Novatek NT98690 H1V1

| Field | Value |
|---|---|
| Date | 2026-09-01 |
| Kind | Decision |
| Status | Verified |
| Scope | `roadmap/07-architecture-portability.md`, `roadmap/README.md`, `sel4/pins.toml`, `tools/nt98690/`, `scripts/build/build-nt98690-payload.py`, `scripts/check/check-nt98690-boot.py`, `scripts/lib/{arm64_image,uboot_console}.py`, `just/hardware.just` |
| Roadmap | P6, P6.A, P6.B, P6.C |
| Gates | `just nt98690_payload_check`, `just nt98690_boot_check`, `just sel4_gate_control_check` |
| Trigger | A vendor BSP and a working serial-driven firmware loop for the Novatek NT98690 (NS02201) H1V1 became available on the development host |
| Baseline | Two physical lanes: the Milk-V Duo, qualified through P3.F, and the Raspberry Pi 5, whose P4 build path is complete but whose boot is unobserved because its only console produces no bytes |

## Summary

P4 is blocked on evidence, not on engineering: the Raspberry Pi 5's seL4 image is
built and pinned, but the board's debug UART is the only console seL4 can have and
the available adapter produces nothing, so no transcript exists and none of P4's
ordered-marker exit conditions can be met. The Novatek NT98690 H1V1 does not have
that problem. Its vendor firmware — a Novatek loader, TF-A 2.2, and U-Boot 2021.10 —
keeps a 16550 UART0 console alive from BL31 onward, and the board's own flashing
tooling already drives that console programmatically to a `nvt: ` prompt. The
evidence path P4 is waiting for exists on this board today.

This entry opens **P6** as a second physical AArch64 lane and records the decisions
that shape it, before any claim is made about the board. The lane is sequenced the
way P3.D–P3.F sequenced the Duo: qualify the firmware handoff and *measure* the
board's facts first (P6.A), port seL4 and `slime-root` onto that qualified handoff
second (P6.B), and add the interactive shell only once the board is qualified
(P6.C). Nothing about the H1V1 is claimed here. P6.A's gate exists, its payload
builds deterministically, and its marker chain is proven to have teeth — but the
board has not been booted, so P6.A's status is in progress and its exit condition
is unobserved.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| `roadmap/07-architecture-portability.md` | New `## P6` with P6.A/P6.B/P6.C, a profile-table row for `aarch64-sel4-nt98690-h1v1`, and a sequencing item | A physical lane is defined by its acceptance conditions before code is written against it |
| `roadmap/README.md` | Track row, mermaid node, and a paragraph in *Physical bring-up sequencing* | The lane is visible where the other lanes are, with its non-substitutability stated |
| `sel4/pins.toml` | New `[ns02201_h1v1]` table of board facts, each with its vendor source named in the comment header | Board facts are sourced from the board or its firmware, never asserted by Slime |
| `tools/nt98690/payload/` | `probe.S` and `probe.ld`: a bare-metal AArch64 payload carrying an arm64 `Image` header | The handoff is measured by something small enough to be read in full |
| `scripts/build/build-nt98690-payload.py` | Deterministic payload build with link-address, memory-map, and header agreement checks | An image whose header disagrees with its link address cannot be produced |
| `scripts/lib/arm64_image.py` | The arm64 `Image` header as a readable format | A wire format this repository produces can be read back and asserted |
| `scripts/lib/uboot_console.py` | Serial/U-Boot driving over a tty or a TCP bridge | Physical gates fail closed on every absence, and a bridged console does not claim framing evidence it cannot see |
| `scripts/check/check-nt98690-boot.py` | The P6.A gate, registered in `check-sel4-gate-controls.py::GATES` | A physical marker chain is proven to reject deleted, transposed, and failure-marked evidence |
| `just/hardware.just` | `nt98690_payload_check`, `nt98690_boot_check`, `nt98690_serial_monitor` | Each recipe states whether it qualifies hardware or only aids bring-up |

## Decisions

- **Decision:** Open the H1V1 as a lane rather than continuing to wait on P4.
  **Rationale:** the blocker on P4 is a console that produces no bytes, and the
  blocker on this board is nothing — its firmware prints, its U-Boot is
  scriptable, and the vendor tooling that drives it is already proven on the
  hardware. Opening a lane where the evidence exists is cheaper than continuing to
  hold one where it does not.
  **Rejected alternative:** buying a different USB-UART adapter for the Pi 5. That
  remains the cheap unblock for P4 and is not foreclosed by this entry; it is
  simply not something this repository can schedule.

- **Decision:** This lane substitutes for no other board's gate, and no other
  board's evidence completes one of its own.
  **Rationale:** roadmap invariant 8 already says so, and a second AArch64 physical
  lane is exactly the situation where it could quietly be violated — the Pi 5 and
  the H1V1 share an architecture, a page granule, and a kernel configuration shape.
  They share no firmware, no memory map, no interrupt-controller addresses, and no
  handoff.
  **Rejected alternative:** treating the H1V1 as an instance of P4's target
  profile. It is a distinct platform contract and gets a distinct profile.

- **Decision:** Boot from removable media through the unmodified vendor firmware,
  and never write eMMC.
  **Rationale:** the vendor loader, TF-A, and U-Boot on eMMC are the recovery path.
  Leaving them untouched means a failed experiment costs a power cycle rather than
  a reflash, which is what makes iterating on this board sustainable — and it
  preserves the same no-ambient-storage boundary P4 and RP3 state.
  **Rejected alternative:** replacing the vendor U-Boot with one built here. It
  would remove the `booti`-only constraint the payload works within, at the cost of
  making the board's recovery path something this project maintains.

- **Decision:** Measure the board's facts with a probe before porting seL4 to it.
  **Rationale:** the values a platform port needs are firmware behaviour, not
  documentation. This board's device tree states a 12 MHz timer, but its TF-A
  programs `CNTFRQ_EL0` on secondary cores only, so the primary core's value is
  unknown until read; the exception level, the address `booti` actually places an
  image at, the implemented physical-address range, and the interrupt line count
  are all the same kind of fact. Pinning them from documents and discovering the
  disagreement during a kernel bring-up is the failure this ordering prevents.
  **Rejected alternative:** porting seL4 first and reading the facts from its boot
  output. That conflates "the port is wrong" with "the assumption was wrong", which
  is precisely the ambiguity a bench session cannot afford.

- **Decision:** Pin the payload's load address, its linker base, and its `Image`
  header `text_offset` to one value, and treat U-Boot's `Moving Image` line as a
  gate failure.
  **Rationale:** this board's U-Boot carries a Novatek patch that sets
  `dst = gd->ram_base` unconditionally in `booti_setup`, ignoring the header's
  "place anywhere" flag, so every image is relocated to `text_offset`. Where that
  disagrees with the link address the image runs from the wrong place and prints
  nothing — indistinguishable, on a console, from a dead board. The three values
  are therefore checked against each other at build time and the relocation itself
  is a failure marker, because P6.B's seL4 image inherits the same contract.
  **Rejected alternative:** loading wherever is convenient and letting the
  relocation happen. It works only as long as `text_offset` and the link address
  agree, which is the thing that would silently stop being true.

- **Decision:** Build the payload with the dev shell's pinned `CROSS_COMPILER_PREFIX`
  rather than a bare-metal toolchain fetched by nixpkgs attribute.
  **Rationale:** this host resolves `nixpkgs` through a rolling channel, so an
  attribute reference would make the payload's bytes depend on the week it was
  built. `flake.nix` exports the cross compiler and `check-sel4-pins.py` asserts the
  exported path, so this way the probe is built by the same pinned toolchain as the
  seL4 product. The Milk-V Duo's payload builder does use an attribute; that is a
  difference between the two, recorded rather than smoothed over.
  **Rejected alternative:** `pkgsCross.aarch64-embedded` by attribute, which is the
  more obvious choice and which the plan originally specified.

- **Decision:** Accept a TCP endpoint as a console, and report framing errors as
  unobservable there rather than as zero.
  **Rationale:** the board may be attached to a different host than the one that
  builds. A `socat`/`ser2net` bridge makes that workable, but it does not forward
  line-discipline state, so no framing-error count exists to report. Reporting zero
  would be a claim about hardware that nothing measured.
  **Rejected alternative:** requiring a local tty, which would tie the lane to
  whichever machine happens to have the adapter.

- **Decision:** Put the shared U-Boot console machinery in `scripts/lib/` for the
  new gate, and leave the three Milk-V Duo gates on their own copies for now.
  **Rationale:** those gates are physically verified against a board that is not on
  this bench. A refactor whose regression test cannot be run is not an improvement,
  and the risk it carries is borne by the one lane that is currently qualified.
  **Rejected alternative:** migrating all four now, which would put P3.D–P3.F's
  observed evidence behind unverified code.

- **Decision:** Extract only the arm64 `Image` header into `scripts/lib/`, not the
  ELF flattening beside it.
  **Rationale:** the header has two consumers as of this entry — the builder that
  emits one and the gate that checks it — while `flatten`/`encode_branch` in
  `build-rpi5-media.py` still has exactly one. The repository's rule is to extract
  observed repetition; P6.B's loader image is what will create the second consumer,
  and the extraction belongs to that change.
  **Rejected alternative:** moving both now, which the session plan specified and
  which would have been speculative generality plus an untestable regression risk
  to the Pi 5 media build.

## Open risks and follow-ups

- [ ] **P6.A is unobserved.** The gate, the payload, and the marker chain exist and
      are proven against synthetic evidence; no H1V1 has been booted. The exit
      condition is not met and nothing about this board may be cited until it is.
- [ ] The gate's U-Boot marker wording is derived from the vendor U-Boot source in
      `/srv/novatek/sdk/worktrees/h1v1-dev/BSP/u-boot` rather than from a
      transcript. `just nt98690_serial_monitor` exists to observe the real wording
      first; the banner, the `mmc` device line, and the device-tree line should be
      confirmed against it before the first scored run.
- [ ] `cnt_hz_est` is a coarse estimate by construction — a character burst timed
      against the line rate, biased 0.17% low — and it is meaningless under the
      QEMU variant, which does not throttle its console. It is there to tell 12 MHz
      from a stopped counter, not to pin a frequency. P6.B must not treat it as one.
- [ ] Three markers (`parange`, `cntfrq`, `gicd_typer`) match any 16-digit value
      because the board has not reported its own yet. They should be tightened to
      the observed values once a transcript exists, which is what makes them
      evidence rather than shape.
- [ ] The Milk-V Duo gates still duplicate `scripts/lib/uboot_console.py`. Migrate
      them in a session that has a Duo attached to re-verify.
- [ ] Nix was installed on this host to build any of this. That is a host state
      change, not a repository one, and it is recorded here because the lane cannot
      be reproduced without it.

## Artifacts and provenance

- Plan of record, carrying the verified board facts and the P6.B/P6.C outlines: [`plan.md`](plan.md)
- Implementation and its verification: [`devlog/2026-09-01-p6a-nt98690-probe/`](../2026-09-01-p6a-nt98690-probe/index.md)
- Vendor BSP consulted for every board fact: `/srv/novatek/sdk/worktrees/h1v1-dev` (Novatek NS02201 SDK; U-Boot 2021.10, TF-A 2.2, Linux 5.10)
- Vendor serial/flash tooling that established the console loop is scriptable: `~/nt98690-ubuntu/_recovery_h1v1/raw_flash/flash_emmc_raw.py`
- Related roadmap items: [P6](../../roadmap/07-architecture-portability.md#p6-novatek-nt98690-ns02201-h1v1-physical-lane), and [P4](../../roadmap/07-architecture-portability.md#p4-raspberry-pi-5-physical-architecture-qualification), whose blocked evidence path motivated this lane

## Corrections

Appended 2026-09-02, when Session 2 (P6.B) was planned against the board's
measurements and the forks' actual shape. `plan.md` beside this entry is the
plan of record and carries the corrected Part A in full; these are the
corrections themselves.

1. **Line numbers and one claim in the fork survey were wrong.** The hypervisor
   `DEPENDS` list is `src/arch/arm/config.cmake:78-84`, not 109; the cache-line
   list is lines 238-244, not 142-144; and `src/plat/bcm2712/overlay-rpi5.dts`
   contains no `seL4,boot-cpu`. Adding a Cortex option touches seven sites, not
   three: the three above plus `configs/seL4Config.cmake`'s default-off list,
   `config_set`, and CPU-name chain, and a `constants_cortex_a73.h`. A platform
   also needs `libsel4/sel4_plat_include/<KernelPlatform>/`, whose name must
   equal the `declare_platform` name.
2. **The seL4 platform is named `ns02201-h1v1`, not `ns02201`**, for the reason
   in point 1 and the fork's own `cv1800b-duo` precedent.
3. **The root-driven reset moved from Session 3 into Session 2.** The Duo's
   three-boot gate is autonomous because its root resets the board; the H1V1's
   analogue is the watchdog sequence its TF-A performs, and P6.B's roadmap text
   already assumed autonomy. Session 3 keeps only the terminator's use of it.
4. **Two risks closed the other way.** `CNTFRQ_EL0` is programmed on the primary
   core (12 MHz, corroborated to 0.33%), so no pinned override exists; and the
   vendor `booti` places an image at `ALIGN(0, 2 MiB) + text_offset` with no
   alignment requirement on `text_offset` itself, so the loader's 4 KiB-aligned
   link base is loaded directly.
5. **Fork pushes are not this host's to make.** Its GitHub identity has no push
   access to `iceice666/{seL4,rust-sel4}`; fork commits are local on
   `slime-ns02201-h1v1` branches and pinned by hash until the operator pushes.
6. **Session 3 was planned on 2026-09-02 against the landed P6.B tree.**
   `plan.md`'s Part D supersedes Part A's `## A8` outline: the board-neutral
   rename is wider than `build.rs` (the builder's Duo-only serial parse and the
   H1V1's unguarded post-graph reset are the real blockers), fork-push risk 9
   closed the good way (forks at `CG-AA`, pushed and pinned), and the P6.C gate
   is written to enter the shared tamper control the Duo's slisp gate never had.
7. **The lane closed on 2026-09-02.** All three sessions ran on the named
   board within two days: P6.A (25 markers), P6.B (three byte-identical seL4
   boots with autonomous watchdog recovery), and P6.C (one resident Slisp
   session over the declared `InputRead` path, ended by the gate-only
   terminator). The decision's exit conditions were observed as proposed; the
   one structural deviation from this entry's plan — the reset moving from
   Session 3 into Session 2 — is correction 3, and Session 3's remaining
   deviations are correction 6. Evidence lives with the three Change entries.
