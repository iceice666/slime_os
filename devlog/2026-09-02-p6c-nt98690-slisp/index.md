# P6.C — Interactive Slisp over UART0 on the H1V1

| Field | Value |
|---|---|
| Date | 2026-09-02 |
| Kind | Change |
| Status | Proposed |
| Scope | `slime-root` (`build.rs`, `src/main.rs`, `src/device.rs`), `scripts/build/{build-sel4,build-nt98690-payload,build-duo-payload}.py`, `scripts/check/{check-nt98690-slisp,check-duo-slisp,check-sel4-pins,check-sel4-gate-controls}.py`, `sel4/pins.toml`, `just/hardware.just`, `roadmap/07-architecture-portability.md` |
| Roadmap | P6.C |
| Gates | `just nt98690_slisp_check`, `just sel4_gate_control_check`, `just sel4_component_graph_check`, `just slisp_core_check` |
| Trigger | [P6.B](../2026-09-02-p6b-sel4-nt98690-h1v1/index.md) closed with the root resetting the board through its watchdog, and the roadmap's P6.C unblocked on it |
| Baseline | The root's product-UART build inputs were named for the Milk-V Duo (`SLIME_DUO_UART_PADDR`, `--duo-test-terminator`) and its build script refused them for any other profile; the H1V1's post-graph reset was unconditional, so a resident product image could not exist for it; no interactive gate, product boot file, or terminator existed for the board |

## Summary

The product-UART build inputs go board-neutral — `SLIME_PRODUCT_UART_PADDR`,
`SLIME_PRODUCT_TEST_TERMINATOR`, cfgs `slime_product_uart` and
`slime_product_test_terminator`, builder flag `--test-terminator`, identity key
`test_terminator` — with genuinely Duo-specific inputs keeping their Duo names.
The H1V1 gains a resident product-graph image over the existing `sel4.zti`
composition: the root maps UART0 (`0x2f0130000`, register-identical to the
Duo's DW-APB at `reg-shift 2`/`reg-io-width 4`, so `DwApbInput` serves both)
and feeds it through the declared `InputRead` path; the gate-only `0x1d`
terminator routes into P6.B's watchdog reset through a new
`request_ns02201_test_reset`, and the post-graph reset is suppressed exactly
when the product UART is compiled in, the Duo's own rule. A new gate,
`check-nt98690-slisp.py`, drives one bounded session — three typed commands
answered at the resident prompt, the 32768-iteration checkpoint crossed, a
fourth answer after it, then the terminator — and is pinned at 34 markers in
the shared tamper control, which the Duo's own slisp gate never entered.

## Changes

| Area | Change | Restored invariant |
|---|---|---|
| Root build inputs | `SLIME_DUO_UART_PADDR` → `SLIME_PRODUCT_UART_PADDR`, `SLIME_DUO_TEST_TERMINATOR` → `SLIME_PRODUCT_TEST_TERMINATOR`, cfgs renamed to match; the guard admits both physical board profiles; `SLIME_DUO_TIMEBASE_HZ` and `SLIME_DUO_EARLY_FAULT` stay Duo-named | The roadmap's rule that product-UART build inputs are board-neutral rather than named for one board |
| Builder | `--duo-test-terminator` → `--test-terminator`, valid for any physical UART platform with `--component-graph`; the product-UART env branch covers both boards with a per-platform serial kind (`dw-apb` / `ns16550a`) asserted against the pinned `serial` string; identity key `duo_test_terminator` → `test_terminator` | A swapped board profile fails loudly instead of parsing the wrong UART address |
| Root | `request_ns02201_test_reset() -> !` prints `SLIME_NT98690 test terminator accepted` and calls the P6.B reset; the terminator install selects the board's reset entry point; both post-graph resets carry `not(slime_product_uart)`; Duo-only statics and callbacks are explicitly Duo-qualified now that the terminator cfg is board-neutral | A resident product image never resets itself; every plane image still does |
| Payload | `[ns02201_h1v1].boot_files` gains `slime-sel4-ns02201-h1v1-test-terminator.bin`; `build-nt98690-payload.py` propagates `variant` and `test_terminator` into the payload identity | The gate artifact is distinct and self-identifying, the P3.F rule |
| Gate | `check-nt98690-slisp.py`: P6.A staging, one scored boot, the session typed character by character, exactly-once assertions on the resident wait and the healthy certification, framing checked before the terminator, recovery by the silent banner window; registered as `("nt98690_slisp", …, 34)` | The P3.F session claim, with the tamper control and the shared console library the Duo gate lacks |
| Recipe | `just nt98690_slisp_check serial=""` depends on `slisp_core_check` and `sel4_component_graph_check` | The QEMU references pass before a board boot is spent |
| Roadmap | P6.C gains `#### Verification target` and `#### Exit condition`; the stale header and P6-umbrella status lines now name P6.A/P6.B complete | Every milestone is scored against a written exit condition |

## Regression guards

| Risk | Guard | Failure signal |
|---|---|---|
| The rename silently changes what a Duo image does | Deterministic rebuild comparison (below) plus the QEMU product-graph, root-boot, and sample gates and the Duo transcript control re-run green after the rename | Any of those gates fails, or the rebuild comparison shows an unexplained delta |
| The H1V1 product image resets itself out of the session | `not(slime_product_uart)` on the post-graph reset; the gate's failure set includes `SLIME_NT98690 reset request` reached before the terminator (ordered contract places it after) | The reset marker before the session completes fails the ordered chain |
| A marker the tamper control cannot instantiate | Every regex in the `literal_for` vocabulary; multi-line markers spell `\n` and are matched against a CR-stripped view | `sel4_gate_control_check` hard-errors on the marker |
| The terminator leaks into ordinary products | The byte is installed only under `slime_product_test_terminator`, which requires the `--test-terminator` build, which marks every identity it touches | `check_identity` fails a gate run whose artifact lacks the explicit key |

## Verification

| Command/scenario | Result | Evidence class |
|---|---|---|
| Pre-rename references: `just sel4_component_graph_check`, `just slisp_core_check`, `just sel4_sample_check`, `just sel4_root_boot_check`, `just test_sel4_root` | PASS (2026-09-02) | Direct QEMU/host |
| Pre-rename Duo graph images built and hashed (`d3ce1086…`, `edd79d7e…`) | Recorded | Build determinism |
| Post-rename `just sel4_component_graph_check` | PASS | Direct QEMU |
| Rename-neutrality: pre-rename tree re-stashed and rebuilt, reproduced `edd79d7e…` bit-for-bit; post-rename stripped root ELF same size (988,624 bytes), 16,466 differing bytes attributed to `-Cmetadata` symbol salt and the reworded shared diagnostic string; child ELF byte-identical | Explained | Build determinism |
| `just sel4_gate_control_check` (48 gates, `nt98690_slisp` at 34 markers) | PASS | Host control |
| QEMU rehearsal of the full P6.C session against the product-graph image: three commands answered, sysinfo spawn chain, resident checkpoint crossed 311s after boot, post-boundary answer; the gate's 23 non-board markers matched in order on the transcript and no failure marker matched | PASS | Direct QEMU |
| `check-nt98690-slisp.py --serial tcp:…` against a board-shaped stand-in (staging dialogue answering `md.l` from the real payload bytes, echoed session, terminator/reset/banner) — from the repository and again standalone from the operator bundle with system Python | PASS | Host rehearsal |
| Post-rename host suite: `just ruff`, `typos`, `fmt_check_all` (after one rustfmt wrap of a lengthened cfg), `lint_all`, `test_sel4_root`, `contracts_check`, `generation_check`, `sel4_pin_check`, `sel4_nt98690_image_check`, `riscv64_qemu_check`, `slisp_core_check`, `devlog_check` | PASS | Host/QEMU |

## Decisions

- **Decision:** Rename the shared inputs to `SLIME_PRODUCT_*` and generalize the
  builder flag, rather than adding parallel `SLIME_NT98690_*` names.
  **Rationale:** The roadmap deliverable says board-neutral, and two parallel
  name sets would leave a third board with three. **Rejected alternative:**
  Duo-named inputs admitted for the H1V1 profile — the names would lie.
- **Decision:** Keep `DwApbInput`'s name and generalize its doc comment.
  **Rationale:** Both boards' device trees pin `reg-shift 2`/`reg-io-width 4`,
  so the adapter is the same 16550 layout; renaming the type would churn the
  Duo lane for no behavioral difference. **Rejected alternative:** a
  `Ns16550Input` duplicate — two identical adapters to keep in sync.
- **Decision:** One scored board boot, not three. **Rationale:** P3.F's
  precedent: the session gate proves interactivity and residency; the
  three-boot byte-identity claim belongs to P6.B, which owns it. Each failed
  attempt costs the operator a manual power cycle. **Rejected alternative:**
  three interactive sessions — typed input timing makes byte-identity across
  sessions a fiction.
- **Decision:** The rename-neutrality check compares deterministic rebuilds
  rather than requiring byte-identity. **Rationale:** the cfg names enter
  rustc's `-Cmetadata` symbol salt and one shared diagnostic string was
  deliberately reworded, so the image hash must move; the pre-rename rebuild
  reproducing its own hash bit-for-bit proves the movement is exactly the
  source diff. **Rejected alternative:** asserting byte-identity — it would
  have failed for reasons unrelated to behavior.

## Open risks and follow-ups

- The board session has not yet run; the roadmap keeps `In progress` until the
  observed exit condition is recorded here.

## Artifacts and provenance

- Roadmap: [P6.C](../../roadmap/07-architecture-portability.md#p6c--interactive-slisp-over-uart0-on-the-h1v1)
- Plan of record: [Part D](../2026-09-01-p6-nt98690-h1v1-lane/plan.md)
