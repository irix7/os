//! ROM entry point: initialise the serial console and print the bring-up marker.
//!
//! Built for `mips2-sgi-irix7-o32` (o32/MIPS II) with `-Z build-std`. The PROM
//! (ARCS) or iris loads this ELF and jumps to `_start`; this crate takes over
//! from the first instruction and prints `hello from ROM` on serial0.

#![no_main]
#![no_std]

use core::panic::PanicInfo;
use rom::{serial_init, serial_write_str};

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // The PROM (ARCS) or iris loader sets $sp on entry to a valid stack in RAM.
    // The ROM's real work (fetch kernel, verify, jump) comes next; stack setup
    // is the loader's responsibility at this bring-up stage.

    serial_init();
    serial_write_str("hello from ROM\r\n");

    // Hang: the ROM's real work (fetch kernel, verify, jump) comes next.
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    rom::serial_write_str("ROM panic\r\n");
    loop {
        core::hint::spin_loop();
    }
}
