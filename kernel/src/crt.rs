//! Minimal C runtime shims.
//!
//! `core` emits calls to `memset`, `memcpy`, `memmove`, and `memcmp` for
//! aggregate initialization, slice ops, and struct copies. On a hosted
//! target libc provides them; on `x86_64-unknown-none` nothing does, so we
//! define them ourselves. `compiler_builtins` supplies the same symbols
//! when built with `build-std`, but providing them explicitly keeps the
//! kernel self-contained and avoids any hidden dependency on a particular
//! `build-std` configuration.
//!
//! Each is `#[inline(never)]` + `volatile` loops so the compiler cannot
//! turn them back into a self-call.

#![allow(unsafe_op_in_unsafe_fn)]

use core::ffi::c_int;

/// Fill `size` bytes at `dest` with `val`.
///
/// # Safety
///
/// `dest` must be valid for `size` consecutive writable bytes.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memset(dest: *mut u8, val: c_int, size: usize) -> *mut u8 {
    let val = val as u8;
    for i in 0..size {
        unsafe { dest.add(i).write_volatile(val) };
    }
    dest
}

/// Copy `size` bytes from `src` to `dest`. Equivalent to `memmove`.
///
/// # Safety
///
/// `src` must be valid for `size` readable bytes and `dest` for `size`
/// writable bytes. The regions may overlap.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcpy(dest: *mut u8, src: *const u8, size: usize) -> *mut u8 {
    unsafe { memmove(dest, src, size) }
}

/// Copy `size` bytes from `src` to `dest`, handling overlap correctly.
///
/// # Safety
///
/// `src` must be valid for `size` readable bytes and `dest` for `size`
/// writable bytes. The regions may overlap.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memmove(dest: *mut u8, src: *const u8, size: usize) -> *mut u8 {
    if (dest as *const u8) < src {
        for i in 0..size {
            unsafe { dest.add(i).write_volatile(src.add(i).read_volatile()) };
        }
    } else {
        for i in (0..size).rev() {
            unsafe { dest.add(i).write_volatile(src.add(i).read_volatile()) };
        }
    }
    dest
}

/// Compare `size` bytes at `a` and `b` lexicographically.
///
/// # Safety
///
/// Both `a` and `b` must be valid for `size` readable bytes.
#[inline(never)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn memcmp(a: *const u8, b: *const u8, size: usize) -> c_int {
    for i in 0..size {
        let (a, b) = unsafe { (a.add(i).read_volatile(), b.add(i).read_volatile()) };
        if a != b {
            return a as c_int - b as c_int;
        }
    }
    0
}
