//! The line behind the page the root binds: the UART's input clock, the
//! divisor that selects the baud rate from it, and the register stride and
//! access width. These are a platform's facts, so a tree carries the line of
//! each port it builds this driver for; the driver owns the protocol, the
//! register sequence (`slime_components::uart16550`), and the reply
//! discipline, and asks this module for nothing else.
//!
//! This tree builds for no platform with a port for this driver, so `config`
//! names no line and the driver answers `STATUS_NO_DEVICE` rather than program
//! a divisor it has no clock for. A platform that carries a port supplies its
//! line here, and nothing above it changes.

use slime_components::uart16550::Line;

/// The line for the bound port, or `None` when this build knows none.
pub fn config() -> Option<Line> {
    None
}
