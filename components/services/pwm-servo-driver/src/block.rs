//! The register model of the PWM block behind the page the root binds: how a
//! channel's frame and pulse become the block's words, how a running channel
//! latches a new frame, and how each word is read back. A model is a
//! platform's, so a tree carries one for each block it builds this driver
//! for; the driver owns the protocol, the admission order, and the failsafe,
//! and asks the model for nothing beyond `attach`, `program`, and `disable`.
//!
//! This tree builds for no platform with a PWM block, so `attach` refuses
//! every page and the driver answers `STATUS_NO_DEVICE` rather than write
//! words it has no model of. A platform that carries a block supplies its
//! model in this module, keeping the same three operations, and nothing above
//! it changes.

use slime_proto::pwm_servo::STATUS_DEVICE_ERROR;

use crate::Page;

/// The model of no block: nothing attaches, so no value of this type is ever
/// asked to program a channel.
#[derive(Clone, Copy)]
pub struct Model;

impl Model {
    /// The model for `page`, or `None` when the page is not a block this
    /// model knows; the driver then refuses requests as it does with no
    /// device. The page's first word has already been checked against a
    /// virtio transport's magic.
    pub fn attach(_page: Page) -> Option<Self> {
        None
    }

    /// Drive `channel` free-running with `pulse_us` high in each `period_us`
    /// frame and read every word back: a status and the reply's detail. The
    /// bounds are the protocol's and were checked before this call; the
    /// register's own limits answer `STATUS_BAD_PULSE`.
    pub fn program(self, _channel: u32, _period_us: u32, _pulse_us: u32) -> (i32, u32) {
        (STATUS_DEVICE_ERROR, 0)
    }

    /// Stop `channel` so its pad idles low, and read back that it stopped.
    pub fn disable(self, _channel: u32) -> (i32, u32) {
        (STATUS_DEVICE_ERROR, 0)
    }
}
