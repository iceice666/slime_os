//! Selects whether `slime-root` boots a compile-time fixture or the immutable
//! disk-backed selector path used by the product image.

fn main() {
    println!("cargo:rerun-if-env-changed=SLIME_GENERATION");
    println!("cargo::rustc-check-cfg=cfg(slime_generation_supplied)");
    println!("cargo:rerun-if-env-changed=SLIME_BOOT_SELECTOR");
    println!("cargo:rerun-if-env-changed=SLIME_ROOT_FIXTURE");
    println!("cargo:rerun-if-env-changed=SLIME_BOOT_BUNDLE_IDENTITY");
    println!("cargo:rerun-if-env-changed=SLIME_TARGET_PROFILE");
    println!("cargo:rerun-if-env-changed=SLIME_QEMU_KEYBOARD");
    println!("cargo::rustc-check-cfg=cfg(slime_boot_selector)");
    println!("cargo::rustc-check-cfg=cfg(slime_b38_force_unwind)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_missing)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_extra)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_aliased)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_wrong_slot)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_wrong_type)");
    println!("cargo::rustc-check-cfg=cfg(slime_b40_mutate_wrong_rights)");
    println!("cargo::rustc-check-cfg=cfg(slime_root_fixture)");
    println!("cargo::rustc-check-cfg=cfg(slime_qemu_keyboard)");
    let target_profile = std::env::var("SLIME_TARGET_PROFILE")
        .unwrap_or_else(|_| "aarch64-sel4-qemu-virt".to_owned());
    println!("cargo:rustc-env=SLIME_TARGET_PROFILE={target_profile}");
    if std::env::var("SLIME_BOOT_SELECTOR").as_deref() == Ok("1") {
        let identity = std::env::var("SLIME_BOOT_BUNDLE_IDENTITY")
            .expect("selector build requires SLIME_BOOT_BUNDLE_IDENTITY");
        if identity.len() != 64 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            panic!("SLIME_BOOT_BUNDLE_IDENTITY must be 32-byte lowercase hex");
        }
        println!("cargo:rustc-cfg=slime_boot_selector");
        println!("cargo:rustc-env=SLIME_BOOT_BUNDLE_IDENTITY={identity}");
    }
    if std::env::var("SLIME_ROOT_FIXTURE").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=slime_root_fixture");
    }
    if std::env::var("SLIME_QEMU_KEYBOARD").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=slime_qemu_keyboard");
    }
    if std::env::var("SLIME_BOOT_SELECTOR").as_deref() != Ok("1")
        && let Ok(path) = std::env::var("SLIME_GENERATION")
    {
        println!("cargo:rerun-if-changed={path}");
        println!("cargo:rustc-cfg=slime_generation_supplied");
    }
}
