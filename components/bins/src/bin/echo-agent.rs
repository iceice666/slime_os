#![no_std]
#![no_main]

slime_rt::entry!(main);

fn main() {
    slime_rt::send(0, b"echo-agent{tool=echo,result=ok}\n", &[]);
}
