//! Register arithmetic for the Novatek NT98690 PWM block, as the `nvt-pwm-driver`
//! programs it and as the board gate reads it back.
//!
//! Pure functions over the block's documented layout (vendor `pwm-nvtivot.c`,
//! `nvt_pwm.c`): each channel has a `PERIOD` word carrying the low eight bits
//! of its rise, fall, and base counts, and an `EXT` word carrying the high
//! eight bits of the same three, so a count is sixteen bits wide and the two
//! words are always written together. Counts are in ticks of the channel's
//! count clock; at the driver's 1 MHz a tick is a microsecond.

/// Byte offset of channel `ch`'s `CTRL` word: `[15:0]` cycle count, zero for
/// free-running.
pub const fn ctrl_offset(ch: u32) -> usize {
    ch as usize * 8
}

/// Byte offset of channel `ch`'s `PERIOD` word.
pub const fn period_offset(ch: u32) -> usize {
    ch as usize * 8 + 4
}

/// Byte offset of channel `ch`'s `EXT_PERIOD` word (channels 0–7 only).
pub const fn ext_offset(ch: u32) -> usize {
    0x230 + ch as usize * 4
}

/// The shared `ENABLE` word: write-one-to-set, reads back the live enables.
pub const ENABLE_OFFSET: usize = 0x100;
/// The shared `DISABLE` word: write-one-to-clear.
pub const DISABLE_OFFSET: usize = 0x104;
/// The shared `LOAD` word: latches a channel's new period while it runs.
pub const LOAD_OFFSET: usize = 0x108;

/// The `PERIOD` and `EXT` words for a pulse high from `rise` to `fall` in a
/// frame of `base` ticks, `rise <= fall <= base`, each at most sixteen bits.
/// `None` when a count does not fit or the order does not hold: the caller's
/// bounds are the protocol's, this is the register's.
pub const fn period_words(rise: u32, fall: u32, base: u32) -> Option<(u32, u32)> {
    if rise > fall || fall > base || base > 0xFFFF {
        return None;
    }
    let period = (rise & 0xFF) | ((fall & 0xFF) << 8) | ((base & 0xFF) << 16);
    let ext = ((rise >> 8) & 0xFF) | (((fall >> 8) & 0xFF) << 8) | (((base >> 8) & 0xFF) << 16);
    Some((period, ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four servo widths the ESC lane pinned at 50 Hz and 1 MHz
    /// (`devlog/2026-09-07-h1v1-esc-lane/plan.md`, A2).
    #[test]
    fn the_pinned_servo_encodings_are_reproduced() {
        assert_eq!(
            period_words(0, 1500, 20000),
            Some((0x0020_DC00, 0x004E_0500))
        );
        assert_eq!(
            period_words(0, 1600, 20000),
            Some((0x0020_4000, 0x004E_0600))
        );
        assert_eq!(
            period_words(0, 1200, 20000),
            Some((0x0020_B000, 0x004E_0400))
        );
        assert_eq!(
            period_words(0, 1000, 20000),
            Some((0x0020_E800, 0x004E_0300))
        );
    }

    #[test]
    fn a_count_out_of_order_or_out_of_width_is_refused() {
        assert_eq!(period_words(1, 0, 20000), None);
        assert_eq!(period_words(0, 20001, 20000), None);
        assert_eq!(period_words(0, 0, 0x1_0000), None);
        assert_eq!(period_words(0, 0, 0), Some((0, 0)));
        assert_eq!(
            period_words(0xFFFF, 0xFFFF, 0xFFFF),
            Some((0x00FF_FFFF, 0x00FF_FFFF))
        );
    }

    #[test]
    fn channel_offsets_stay_inside_the_first_page_for_the_servo_channels() {
        assert_eq!(ctrl_offset(0), 0x00);
        assert_eq!(period_offset(0), 0x04);
        assert_eq!(ctrl_offset(5), 0x28);
        assert_eq!(period_offset(5), 0x2C);
        assert_eq!(ext_offset(0), 0x230);
        assert_eq!(ext_offset(5), 0x244);
    }
}
