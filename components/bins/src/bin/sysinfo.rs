#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main() {
    slime_rt::send(0, b"sysinfo{arch=x86_64,target=qemu}\n", &[]);
}
