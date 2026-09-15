//! Line configuration and bounded polled transmit for a 16550-compatible UART.
//!
//! Pure register sequencing over a caller-supplied [`Regs`] and clock: the
//! driver owns the mapping and the register stride, and the platform owns the
//! line (input clock and divisor). Nothing here waits without a deadline, and
//! every deadline is time derived from the line's own character time, so the
//! bound holds at any baud rate and any register-access cost.

/// Register access by 16550 register index. The byte offset of index `n` is
/// the platform's stride, which the implementation applies.
pub trait Regs {
    fn read(&self, index: usize) -> u32;
    fn write(&mut self, index: usize, value: u32);
}

/// A platform's line: the UART's input clock, the divisor latch value that
/// selects the baud rate from it, and how its registers are laid out — the
/// stride as a shift and the access width in bytes (1 or 4). This module uses
/// only the clock and divisor; the layout is for the [`Regs`] implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Line {
    pub clock_hz: u32,
    pub divisor: u16,
    pub reg_shift: u8,
    pub reg_width: u8,
}

// Register indices. DLL and DLM alias RBR/THR and IER while LCR's DLAB bit is set.
pub const THR: usize = 0;
pub const IER: usize = 1;
pub const FCR: usize = 2;
pub const LCR: usize = 3;
pub const MCR: usize = 4;
pub const LSR: usize = 5;
pub const DLL: usize = 0;
pub const DLM: usize = 1;

pub const LSR_THRE: u32 = 1 << 5;
pub const LSR_TEMT: u32 = 1 << 6;
/// Enable the FIFOs and clear both.
pub const FCR_ENABLE_CLEAR: u32 = 0x07;
pub const LCR_DLAB: u32 = 0x80;
/// Eight data bits, no parity, one stop bit.
pub const LCR_8N1: u32 = 0x03;

/// Bits on the wire per 8N1 character: start, eight data, stop.
const BITS_PER_CHAR: u64 = 10;
/// A 16550 samples each bit sixteen times, so one bit is `16 × divisor` clock periods.
const OVERSAMPLE: u64 = 16;
/// How many character times one byte, or the final drain, may take.
const CHARS_PER_BUDGET: u64 = 4;
/// Status reads between clock reads, so the deadline costs few clock calls.
const READS_PER_CLOCK: u32 = 16;

/// The register that did not read back what was written, and what it held.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineFault {
    pub reg: &'static str,
    pub got: u32,
}

/// A transmit that ran out of time after `written` bytes reached the line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timeout {
    pub written: usize,
}

pub struct Uart<R: Regs> {
    regs: R,
    budget_ticks: u64,
}

/// Four character times at `line`, in ticks of a `tick_hz` clock; at least one tick.
pub const fn budget_ticks(line: Line, tick_hz: u64) -> u64 {
    let numerator = BITS_PER_CHAR
        .saturating_mul(OVERSAMPLE)
        .saturating_mul(line.divisor as u64)
        .saturating_mul(tick_hz)
        .saturating_mul(CHARS_PER_BUDGET);
    let ticks = numerator
        / if line.clock_hz == 0 {
            1
        } else {
            line.clock_hz as u64
        };
    if ticks == 0 { 1 } else { ticks }
}

impl<R: Regs> Uart<R> {
    /// Program `line` as 8N1 with FIFOs, interrupts and modem control off, and
    /// read every latch back. The transmitter is first allowed to drain, so a
    /// byte already on the line is not cut.
    pub fn configure(
        mut regs: R,
        line: Line,
        tick_hz: u64,
        now: &mut impl FnMut() -> u64,
    ) -> Result<Self, LineFault> {
        if line.clock_hz == 0 || line.divisor == 0 {
            return Err(LineFault {
                reg: "divisor",
                got: line.divisor as u32,
            });
        }
        let budget_ticks = budget_ticks(line, tick_hz);
        if !wait_for(&regs, LSR_TEMT, budget_ticks, now) {
            return Err(LineFault {
                reg: "lsr",
                got: regs.read(LSR),
            });
        }
        regs.write(IER, 0);
        regs.write(MCR, 0);
        regs.write(FCR, FCR_ENABLE_CLEAR);
        regs.write(LCR, LCR_DLAB | LCR_8N1);
        regs.write(DLL, (line.divisor & 0xff) as u32);
        regs.write(DLM, (line.divisor >> 8) as u32);
        expect(&regs, DLL, "dll", (line.divisor & 0xff) as u32)?;
        expect(&regs, DLM, "dlm", (line.divisor >> 8) as u32)?;
        regs.write(LCR, LCR_8N1);
        expect(&regs, LCR, "lcr", LCR_8N1)?;
        expect(&regs, MCR, "mcr", 0)?;
        expect(&regs, IER, "ier", 0)?;
        Ok(Self { regs, budget_ticks })
    }

    /// Put `bytes` on the line, each once the holding register is empty, then
    /// wait for the transmitter to drain.
    pub fn transmit(&mut self, bytes: &[u8], now: &mut impl FnMut() -> u64) -> Result<(), Timeout> {
        for (written, byte) in bytes.iter().enumerate() {
            if !wait_for(&self.regs, LSR_THRE, self.budget_ticks, now) {
                return Err(Timeout { written });
            }
            self.regs.write(THR, *byte as u32);
        }
        if !wait_for(&self.regs, LSR_TEMT, self.budget_ticks, now) {
            return Err(Timeout {
                written: bytes.len(),
            });
        }
        Ok(())
    }

    /// The transmitter-empty bits of the line status. Receive-side bits are
    /// masked: an unconnected receive line reports framing and break noise.
    pub fn line_status(&self) -> u32 {
        self.regs.read(LSR) & (LSR_THRE | LSR_TEMT)
    }
}

fn expect<R: Regs>(regs: &R, index: usize, reg: &'static str, want: u32) -> Result<(), LineFault> {
    let got = regs.read(index) & 0xff;
    if got == want {
        Ok(())
    } else {
        Err(LineFault { reg, got })
    }
}

fn wait_for<R: Regs>(
    regs: &R,
    mask: u32,
    budget_ticks: u64,
    now: &mut impl FnMut() -> u64,
) -> bool {
    let deadline = now().saturating_add(budget_ticks);
    let mut reads = 0u32;
    loop {
        if regs.read(LSR) & mask != 0 {
            return true;
        }
        reads = reads.wrapping_add(1);
        if reads.is_multiple_of(READS_PER_CLOCK) && now() >= deadline {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::vec::Vec;

    /// A 16550 whose latches read back what was written, whose line status is
    /// scripted, and which logs every write in order.
    struct Fake {
        normal: [Cell<u32>; 8],
        latch: [Cell<u32>; 2],
        writes: core::cell::RefCell<Vec<(usize, u32, bool)>>,
        /// Line-status reads answered "busy" before the transmitter reports empty.
        busy_reads: Cell<u32>,
        thr_writes: Cell<usize>,
        /// After this many bytes the transmitter never reports empty again.
        stuck_after: Option<usize>,
        corrupt: Option<usize>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                normal: Default::default(),
                latch: Default::default(),
                writes: Default::default(),
                busy_reads: Cell::new(0),
                thr_writes: Cell::new(0),
                stuck_after: None,
                corrupt: None,
            }
        }
        fn dlab(&self) -> bool {
            self.normal[LCR].get() & LCR_DLAB != 0
        }
    }

    impl Regs for &Fake {
        fn read(&self, index: usize) -> u32 {
            if index == LSR {
                if self
                    .stuck_after
                    .is_some_and(|after| self.thr_writes.get() >= after)
                {
                    return 0;
                }
                if self.busy_reads.get() > 0 {
                    self.busy_reads.set(self.busy_reads.get() - 1);
                    return 0;
                }
                return LSR_THRE | LSR_TEMT | 0x1e;
            }
            let value = if self.dlab() && index < 2 {
                self.latch[index].get()
            } else {
                self.normal[index].get()
            };
            if self.corrupt == Some(index) {
                value ^ 1
            } else {
                value
            }
        }
        fn write(&mut self, index: usize, value: u32) {
            let dlab = self.dlab();
            self.writes.borrow_mut().push((index, value, dlab));
            if dlab && index < 2 {
                self.latch[index].set(value);
            } else if index != THR && index != FCR {
                self.normal[index].set(value);
            }
            if index == THR && !dlab {
                self.busy_reads.set(3);
                self.thr_writes.set(self.thr_writes.get() + 1);
            }
        }
    }

    const LINE: Line = Line {
        clock_hz: 1_843_200,
        divisor: 12,
        reg_shift: 0,
        reg_width: 1,
    };
    const TICK_HZ: u64 = 1_000_000;

    fn clock() -> impl FnMut() -> u64 {
        let mut ticks = 0u64;
        move || {
            ticks += 10;
            ticks
        }
    }

    #[test]
    fn the_budget_is_four_character_times_at_the_line_rate() {
        // 9600 baud: one character is 10 / 9600 s, 1041 µs; four are 4166.
        assert_eq!(budget_ticks(LINE, TICK_HZ), 4166);
        // A slow clock still gets one tick.
        assert_eq!(budget_ticks(Line { divisor: 1, ..LINE }, 1), 1);
    }

    #[test]
    fn configuration_writes_the_line_in_order_with_the_latches_under_dlab() {
        let fake = Fake::new();
        let uart = Uart::configure(
            &fake,
            Line {
                divisor: 0x0134,
                ..LINE
            },
            TICK_HZ,
            &mut clock(),
        );
        assert!(uart.is_ok());
        assert_eq!(
            *fake.writes.borrow(),
            [
                (IER, 0, false),
                (MCR, 0, false),
                (FCR, FCR_ENABLE_CLEAR, false),
                (LCR, LCR_DLAB | LCR_8N1, false),
                (DLL, 0x34, true),
                (DLM, 0x01, true),
                (LCR, LCR_8N1, true),
            ]
        );
    }

    #[test]
    fn a_latch_that_does_not_read_back_names_its_register() {
        let mut fake = Fake::new();
        fake.corrupt = Some(LCR);
        let fault = Uart::configure(&fake, LINE, TICK_HZ, &mut clock()).err();
        assert_eq!(
            fault,
            Some(LineFault {
                reg: "lcr",
                got: LCR_8N1 ^ 1
            })
        );
    }

    #[test]
    fn a_line_with_no_divisor_is_refused_before_any_write() {
        let fake = Fake::new();
        let fault =
            Uart::configure(&fake, Line { divisor: 0, ..LINE }, TICK_HZ, &mut clock()).err();
        assert_eq!(
            fault,
            Some(LineFault {
                reg: "divisor",
                got: 0
            })
        );
        assert!(fake.writes.borrow().is_empty());
    }

    #[test]
    fn a_transmitter_that_never_drains_refuses_configuration() {
        let mut fake = Fake::new();
        fake.stuck_after = Some(0);
        let fault = Uart::configure(&fake, LINE, TICK_HZ, &mut clock()).err();
        assert_eq!(fault.map(|fault| fault.reg), Some("lsr"));
        assert!(fake.writes.borrow().is_empty());
    }

    #[test]
    fn each_byte_waits_for_the_holding_register_and_the_line_drains_last() {
        let fake = Fake::new();
        let mut uart = Uart::configure(&fake, LINE, TICK_HZ, &mut clock())
            .ok()
            .unwrap();
        fake.writes.borrow_mut().clear();
        assert_eq!(uart.transmit(b"hb!", &mut clock()), Ok(()));
        let sent: Vec<u32> = fake
            .writes
            .borrow()
            .iter()
            .map(|(_, value, _)| *value)
            .collect();
        assert_eq!(sent, [b'h' as u32, b'b' as u32, b'!' as u32]);
        assert!(
            fake.writes
                .borrow()
                .iter()
                .all(|(index, _, dlab)| *index == THR && !dlab)
        );
        assert_eq!(
            fake.busy_reads.get(),
            0,
            "the drain wait consumed the last byte's busy reads"
        );
        assert_eq!(uart.line_status(), LSR_THRE | LSR_TEMT);
    }

    #[test]
    fn a_stuck_holding_register_times_out_with_the_bytes_already_written() {
        let mut fake = Fake::new();
        fake.stuck_after = Some(2);
        let mut uart = Uart::configure(&fake, LINE, TICK_HZ, &mut clock())
            .ok()
            .unwrap();
        assert_eq!(
            uart.transmit(b"abcd", &mut clock()),
            Err(Timeout { written: 2 })
        );
        assert_eq!(fake.thr_writes.get(), 2);
    }

    #[test]
    fn a_line_that_never_drains_after_the_last_byte_times_out_with_every_byte_written() {
        let mut fake = Fake::new();
        fake.stuck_after = Some(3);
        let mut uart = Uart::configure(&fake, LINE, TICK_HZ, &mut clock())
            .ok()
            .unwrap();
        assert_eq!(
            uart.transmit(b"abc", &mut clock()),
            Err(Timeout { written: 3 })
        );
    }
}
