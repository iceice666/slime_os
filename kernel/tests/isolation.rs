#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(slime_os_kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::arch::global_asm;
use core::sync::atomic::{AtomicU64, Ordering};
use slime_os_kernel::serial_println;
use slime_os_kernel::{capability, gdt, interrupts, ipc, memory, task, time};

global_asm!(
    r#"
    .section .rodata
    .global user_sender_start
    user_sender_start:
        mov rdi, 0x1000
        mov rsi, 1
        mov rax, 5
        int 0x80
        cmp rax, -4
        jne sender_bad_exit
        lea rdi, [rip + str_a]
        mov rsi, 2
        mov rax, 5
        int 0x80
        mov rdi, 0
        lea rsi, [rip + payload]
        mov rdx, 2
        xor r10, r10
        xor r8, r8
        mov rax, 1
        int 0x80
        jmp sender_fault
    sender_bad_exit:
        mov rdi, 1
        mov rax, 3
        int 0x80
    sender_fault:
        xor rax, rax
        mov qword ptr [rax], 1
        mov rdi, 0
        mov rax, 3
        int 0x80
    str_a:
        .ascii "A:"
    payload:
        .ascii "hi"
    .global user_sender_end
    user_sender_end:

    .global user_receiver_start
    user_receiver_start:
        sub rsp, 96
        lea rdi, [rip + str_b]
        mov rsi, 2
        mov rax, 5
        int 0x80
    recv_retry:
        mov rdi, 0
        mov rsi, rsp
        lea rdx, [rsp + 64]
        mov rax, 2
        int 0x80
        cmp rax, 0
        jge recv_done
        mov rax, 0
        int 0x80
        jmp recv_retry
    recv_done:
        mov rdi, rsp
        mov rsi, 2
        mov rax, 5
        int 0x80
        mov r8, 3
    yield_loop:
        mov rax, 0
        int 0x80
        dec r8
        jnz yield_loop
        mov rdi, 0
        mov rax, 3
        int 0x80
    str_b:
        .ascii "B:"
    .global user_receiver_end
    user_receiver_end:
    "#,
);

unsafe extern "C" {
    static user_sender_start: u8;
    static user_sender_end: u8;
    static user_receiver_start: u8;
    static user_receiver_end: u8;
}

static SEND_ID: AtomicU64 = AtomicU64::new(0);
static RECV_ID: AtomicU64 = AtomicU64::new(0);

/// Wrap a position-independent code blob in a single-segment executable
/// component image (`contracts/component/v1`).
fn rx_image(code: &[u8]) -> Vec<u8> {
    use slime_os_kernel::component::*;
    let mut image = Vec::new();
    image.extend_from_slice(
        &WireImageHeader {
            magic: IMAGE_MAGIC,
            format_version: FORMAT_VERSION,
            header_size: HEADER_LEN as u32,
            kernel_abi: KERNEL_ABI_VERSION,
            entry_offset: 0,
            segment_count: 1,
            reserved: 0,
            stack_bytes: DEFAULT_STACK_BYTES,
        }
        .encode(),
    );
    image.extend_from_slice(
        &WireSegmentRecord {
            vaddr_offset: 0,
            mem_len: code.len() as u32,
            file_offset: 0,
            file_len: code.len() as u32,
            flags: SEGMENT_FLAG_EXEC,
            reserved: 0,
        }
        .encode(),
    );
    image.extend_from_slice(code);
    image
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start() -> ! {
    slime_os_kernel::limine::ensure_linked();
    gdt::init();
    interrupts::init();
    memory::init();
    time::init();
    test_main();
    slime_os_kernel::hlt_loop()
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    slime_os_kernel::test_panic_handler(info)
}

#[test_case]
fn two_components_ipc_and_fault_isolation() {
    let (send_ep, recv_ep) = ipc::channel();
    let sender_code = unsafe {
        core::slice::from_raw_parts(
            &user_sender_start,
            &user_sender_end as *const _ as usize - &user_sender_start as *const _ as usize,
        )
    };
    let receiver_code = unsafe {
        core::slice::from_raw_parts(
            &user_receiver_start,
            &user_receiver_end as *const _ as usize - &user_receiver_start as *const _ as usize,
        )
    };

    let receiver_image = rx_image(receiver_code);
    let sender_image = rx_image(sender_code);

    let recv_id = task::spawn_with_caps(
        &receiver_image,
        vec![capability::Capability {
            object: capability::KernelObject::Endpoint(recv_ep),
            rights: capability::RIGHT_RECV,
        }],
    )
    .unwrap();
    let send_id = task::spawn_with_caps(
        &sender_image,
        vec![capability::Capability {
            object: capability::KernelObject::Endpoint(send_ep),
            rights: capability::RIGHT_SEND,
        }],
    )
    .unwrap();

    SEND_ID.store(send_id, Ordering::Relaxed);
    RECV_ID.store(recv_id, Ordering::Relaxed);

    extern "C" fn on_all_terminated() {
        let sender = task::termination_summary(SEND_ID.load(Ordering::Relaxed));
        let receiver = task::termination_summary(RECV_ID.load(Ordering::Relaxed));
        let sender_faulted = matches!(
            sender,
            Some(task::TermReason::Fault(task::UserFaultReason::PageFault))
        );
        let receiver_exited_ok = matches!(receiver, Some(task::TermReason::Exit(0)));
        if sender_faulted && receiver_exited_ok {
            serial_println!("[iso] ok: sender faulted (PF), receiver survived and exited 0");
            slime_os_kernel::exit_qemu(slime_os_kernel::QemuExitCode::Success);
        } else {
            serial_println!("[iso] FAIL sender={:?} receiver={:?}", sender, receiver);
            slime_os_kernel::exit_qemu(slime_os_kernel::QemuExitCode::TestFailed);
        }
        slime_os_kernel::hlt_loop()
    }

    task::set_on_idle(on_all_terminated);
    task::run();
}
