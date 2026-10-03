#![no_std]
#![no_main]

slime_rt::entry!(main);

fn fail() -> ! {
    slime_rt::debug_write(b"[large-image-owner] FAIL lifecycle\n");
    slime_rt::exit(1)
}

fn main(_startup_arg: u32) {
    for incarnation in 1..=4 {
        let child = slime_rt::spawn(0, &[]).unwrap_or_else(|_| fail());
        slime_rt::lifecycle_parameter_write(child.supervision_slot, 0, incarnation)
            .unwrap_or_else(|_| fail());
        let outcome = loop {
            match slime_rt::supervision_status(child.supervision_slot) {
                Ok(Some(outcome)) => break outcome,
                Ok(None) => slime_rt::yield_now(),
                Err(_) => fail(),
            }
        };
        match (incarnation, outcome) {
            (3, slime_rt::Termination::Fault(_)) => {}
            (1 | 2 | 4, slime_rt::Termination::Exit(0)) => {}
            _ => fail(),
        }
        slime_rt::debug_write(b"[large-image-owner] incarnation=");
        slime_rt::debug_write(&[b'0' + incarnation as u8]);
        slime_rt::debug_write(if incarnation == 3 {
            b" outcome=fault\n"
        } else {
            b" outcome=exit\n"
        });
    }
    slime_rt::debug_write(b"[large-image-owner] complete incarnations=4 faults=1\n");
    if slime_rt::notification_signal(1) != slime_rt::ERR_SUCCESS {
        fail();
    }
}
