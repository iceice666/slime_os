use std::io::{BufWriter, Write};
use std::path::PathBuf;

const REGION_BYTES: usize = 36 * 1024 * 1024;

pub(crate) fn generate() {
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let mut assembly = String::new();
    for (name, flags, seed) in [
        ("rodata", "a", 0x524f_4441_5441_0000_u64),
        ("data", "aw", 0x4441_5441_0000_0000_u64),
    ] {
        let path = out.join(format!("{name}.bin"));
        let mut bytes = BufWriter::new(std::fs::File::create(&path).expect("region file"));
        for word in 0..REGION_BYTES / 8 {
            bytes
                .write_all(&(seed ^ word as u64).to_le_bytes())
                .expect("region word");
        }
        bytes.flush().expect("region flush");
        assembly.push_str(&format!(
            ".section .large_image_{name},\"{flags}\",%progbits\n.balign 4096\n.global large_image_{name}\nlarge_image_{name}:\n.incbin \"{}\"\n",
            path.display()
        ));
    }
    assembly.push_str(
        ".section .large_image_bss,\"aw\",%nobits\n.balign 4096\n.global large_image_bss\nlarge_image_bss:\n.skip 25165824\n",
    );
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("aarch64") {
        for page in 0..3 {
            assembly.push_str(&format!(
                ".section .large_image_text_{page},\"ax\",%progbits\n.balign 4096\n.global large_image_text_{page}\nlarge_image_text_{page}:\nmov x0, #{}\nret\n.space 4088\n",
                page + 1
            ));
        }
    }
    std::fs::write(out.join("regions.S"), assembly).expect("generated regions");
    let linker = out.join("regions.ld");
    std::fs::write(
        &linker,
        "SECTIONS { .large_image_text ALIGN(4096) : { KEEP(*(.large_image_text_0)) *(.text .text.*) ASSERT(. <= ADDR(.large_image_text) + 65536, \"probe ordinary text exceeds first half\"); . = ADDR(.large_image_text) + 65536; KEEP(*(.large_image_text_1)) . = ADDR(.large_image_text) + 131072; KEEP(*(.large_image_text_2)) } } INSERT AFTER .rodata;\nSECTIONS { .large_image_rodata ALIGN(4096) : { KEEP(*(.large_image_rodata)) } } INSERT BEFORE .rodata;\nSECTIONS { .large_image_data ALIGN(4096) : { KEEP(*(.large_image_data)) } } INSERT BEFORE .bss;\nSECTIONS { .large_image_bss ALIGN(4096) (NOLOAD) : { KEEP(*(.large_image_bss)) } } INSERT BEFORE .bss;\n",
    )
    .expect("linker fragment");
    println!("cargo:rustc-link-arg=-T{}", linker.display());
    println!("cargo:rerun-if-changed=build.rs");
}
