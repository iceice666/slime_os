#![no_std]
#![no_main]

slime_rt::entry!(main);

/// A child that ends, and nothing else.
///
/// The supervision gate spawns more than `MAX_RECORDS` of these over one boot
/// to cross a bound that only a task's *lifetime* count can reach. Every
/// resource it holds must therefore be returned when it is reclaimed, or the
/// loop exhausts some other table first and the gate fails for the wrong
/// reason. This holds exactly one — the transfer window `slime_rt::entry!`
/// binds — and `WindowTable::release` frees that on the cleanup record.
///
/// In particular it takes no channel. `launch_context::receive` would need one,
/// and `ChannelTable` never reclaims (backlog B22), so a child that used the
/// launch context would run out of channels one iteration before the loop
/// reached the bound it exists to cross.
fn main(_startup_arg: u32) {
    slime_rt::debug_write(b"[supervision-child] ran\n");
}
