#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main(_startup_arg: u32) {
    if slime_rt::notification_wait(0).is_err() {
        slime_rt::debug_write(b"[large-image-witness] FAIL completion notification\n");
        slime_rt::exit(1);
    }
}
