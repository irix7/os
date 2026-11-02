// SPDX-License-Identifier: GPL-3.0-only
//! IRIX 7 x86_64 machine.
//!
//! The first slice of #266: QEMU loads this ELF through the Xen PVH path, the
//! assembly stub in `boot.S` enters long mode with the first 4 GiB identity
//! mapped, and this code brings up the serial console, the PVH memory map, the
//! IDT and the PIT.  A deliberate breakpoint proves the trap path reports
//! rather than hanging.  The shared kernel core is not linked in yet; the
//! machine layer is being built first so the core can move onto it without a
//! second copy of kernel logic.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use core::arch::asm;
use core::panic::PanicInfo;

mod config;
mod fetch;
mod heap;
mod idt;
mod memory;
mod module;
mod net;
mod pci;
mod port;
mod pvh;
mod serial;
mod services;
mod stack;
mod tcp;
mod timer;
mod virtio;
mod virtio_blk;
mod virtio_net;

core::arch::global_asm!(include_str!("boot.S"), options(att_syntax));

/// The 16550 debug port, used before anything else exists.
pub const QEMU_DEBUG_EXIT: u16 = 0x501;

/// Leave QEMU through `isa-debug-exit`.
///
/// The device encodes the guest's value as `(value << 1) | 1`, so a caller that
/// passes 0 makes QEMU exit with status 1.  Without the device the write is a
/// no-op and the machine halts instead, which keeps the image usable under a
/// plain `-kernel` invocation.
pub fn qemu_exit(value: u32) -> ! {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, eax", in("dx") QEMU_DEBUG_EXIT, in("eax") value, options(nomem, nostack));
    }
    halt()
}

/// Stop the machine until an interrupt arrives.
pub fn halt() -> ! {
    loop {
        // SAFETY: `hlt` is a ring-0 instruction; interrupts stay enabled so a
        // wake-up is possible.
        unsafe {
            asm!("hlt", options(nomem, nostack));
        }
    }
}

fn print_hex(value: u64) {
    serial::write(b"0x");
    irix7_fmt::hex(value, 1, false, &mut |b| serial::write_byte(b));
}

fn print_hex8(value: u8) {
    irix7_fmt::hex(u64::from(value), 2, false, &mut |b| serial::write_byte(b));
}

fn print_dec(value: u64) {
    irix7_fmt::decimal(value, &mut |b| serial::write_byte(b));
}

fn print_line(parts: &[&[u8]]) {
    for part in parts {
        serial::write(part);
    }
    serial::write_byte(b'\n');
}

/// The PVH entry point, called from `start64` with the physical address of
/// `hvm_start_info`.
#[no_mangle]
pub extern "C" fn kernel_main(start_info: u64) -> ! {
    serial::init();
    print_line(&[b"x86: booted"]);

    serial::write(b"x86: pvh start_info=");
    print_hex(start_info);
    serial::write_byte(b'\n');

    let ram = pvh::describe(start_info);
    serial::write(b"x86: ram=");
    print_dec(ram / (1024 * 1024));
    serial::write(b" MiB\n");

    memory::init(start_info);
    let (heap_base, heap_size) = heap::init();
    serial::write(b"x86: frames total=");
    print_dec(memory::total_ram() / (1024 * 1024));
    serial::write(b" MiB free=");
    print_dec(memory::free_frames() as u64);
    serial::write(b" frames\n");
    serial::write(b"x86: heap base=");
    print_hex(heap_base as u64);
    serial::write(b" size=");
    print_dec(heap_size as u64);
    serial::write(b"\n");

    // Prove both layers before anything depends on them: one frame must come
    // back page-aligned and above the kernel image, and a growing collection
    // must sum correctly.
    let frame = memory::alloc_frame().expect("x86: frame allocation");
    if !frame.is_multiple_of(4096) || frame < heap_base as u64 + heap_size as u64 {
        print_line(&[b"x86: frame allocation outside the free range"]);
        qemu_exit(1);
    }
    serial::write(b"x86: frame alloc=");
    print_hex(frame);
    serial::write_byte(b'\n');

    let mut values: Vec<u64> = Vec::new();
    for value in 0..1000u64 {
        values.push(value);
    }
    let sum: u64 = values.iter().sum();
    serial::write(b"x86: heap sum=");
    print_dec(sum);
    serial::write_byte(b'\n');
    if sum != 499_500 {
        print_line(&[b"x86: heap sum mismatch"]);
        qemu_exit(1);
    }

    idt::init();
    timer::init();
    print_line(&[b"x86: timer enabled"]);

    let mut spins = 0u64;
    while timer::ticks() < 10 {
        if spins >= 500_000_000 {
            print_line(&[b"x86: timer stalled"]);
            qemu_exit(1);
        }
        spins += 1;
        core::hint::spin_loop();
    }
    serial::write(b"x86: timer ticks=");
    print_dec(timer::ticks() as u64);
    serial::write_byte(b'\n');

    let _ = net::init();
    if !virtio_blk::init() {
        print_line(&[b"x86: blk: no virtio-blk device"]);
        qemu_exit(1);
    }
    let disk = virtio_blk::disk().expect("x86: block device");
    virtio_blk::selftest(disk);
    let disk = virtio_blk::disk().expect("x86: block device");
    config::init(disk);

    let mut value = [0u8; 64];
    let n = config::get(b"test.key", &mut value);
    if n > 0 {
        serial::write(b"x86: config test.key=");
        serial::write(&value[..n as usize]);
        serial::write_byte(b'\n');
    }

    // Exercise the block service a module uses: write and read back through
    // the store mapping, not the raw driver.
    let mut pattern = [0u8; virtio_blk::SECTOR];
    for (i, byte) in pattern.iter_mut().enumerate() {
        *byte = b"X86-STORE"[i % 9];
    }
    let mut readback = [0u8; virtio_blk::SECTOR];
    // SAFETY: the buffers are local and the calls copy one sector each.
    // LBA 64 is inside the reserved store region but away from the epoch
    // record at LBA 8; a self-test must not destroy persisted state.
    const TEST_LBA: u32 = 64;
    let wrote = unsafe { services::block_write(3, TEST_LBA, pattern.as_ptr()) };
    let read = unsafe { services::block_read(3, TEST_LBA, readback.as_mut_ptr()) };
    if wrote != 0 || read != 0 || readback != pattern {
        print_line(&[b"x86: block service read/write failed"]);
        qemu_exit(1);
    }
    print_line(&[b"x86: block service read/write ok"]);
    // Fetch both modules, then load them.  The MCP module initialises on the boot
    // stack, which is small and only runs its init.  The wasm policy module runs
    // on the large stack, because the Wasm runtime is deep.
    let mcp_bytes = fetch::image(
        virtio_blk::disk().expect("x86: block device"),
        "mcp",
        fetch::MCP_URL_SECTOR,
        module::MCP_HEADER_SECTOR,
        fetch::SLOT_MCP,
    );
    let wasm_bytes = fetch::image(
        virtio_blk::disk().expect("x86: block device"),
        "wasm",
        fetch::WASM_URL_SECTOR,
        module::WASM_HEADER_SECTOR,
        fetch::SLOT_WASM,
    );
    let mcp = module::load(&module::MCP, mcp_bytes);
    let mut wasm = core::ptr::null();
    stack::on_stack(&mut || {
        wasm = module::load(&module::WASM, wasm_bytes);
    });
    if wasm.is_null() {
        print_line(&[b"x86: wasm: no vtable"]);
        qemu_exit(1);
    }
    module::serve(mcp);

    print_line(&[b"x86: deliberate fault: int3"]);
    // SAFETY: the IDT installs a handler for vector 3.
    unsafe {
        asm!("int3", options(nomem, nostack));
    }
    unreachable!("int3 returned");
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    print_line(&[
        b"x86: panic: ",
        info.message().as_str().unwrap_or("?").as_bytes(),
    ]);
    qemu_exit(2)
}
