#![no_std]
#![no_main]
use slime_rt::console_line::Line;
use slime_rt::exit;
slime_rt::entry!(main);
fn main(_: u32) {
    let endpoint_bindings =
        u64::from(slime_rt::resolve_binding(b"network-service-link-device").is_ok());
    let protocol_operations = 0u64;
    Line::<256>::new()
        .bytes(b"[io-link-loopback] declared endpoint bindings=")
        .decimal(endpoint_bindings)
        .bytes(b" protocol operations=")
        .decimal(protocol_operations)
        .bytes(b"\n")
        .emit();
    exit(0)
}
