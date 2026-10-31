// SPDX-License-Identifier: GPL-3.0-only
//! The 16550 serial console, polled.
//!
//! This is the x86 machine's first console: it exists before the IDT, before
//! the heap and before any driver.  Interrupts stay off on the UART and output
//! waits for the transmitter to drain, so a panic can still write.

use core::arch::asm;

const COM1: u16 = 0x3F8;

fn outb(port: u16, value: u8) {
    // SAFETY: an I/O port write, valid in ring 0.
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack));
    }
}

fn inb(port: u16) -> u8 {
    let value: u8;
    // SAFETY: an I/O port read, valid in ring 0.
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack));
    }
    value
}

/// Bring COM1 up at 115200 8N1 with the FIFO enabled.
pub fn init() {
    outb(COM1 + 1, 0x00); // no UART interrupts
    outb(COM1 + 3, 0x80); // divisor latch
    outb(COM1, 0x01); // divisor 1
    outb(COM1 + 1, 0x00);
    outb(COM1 + 3, 0x03); // 8 bits, no parity, one stop bit
    outb(COM1 + 2, 0xC7); // FIFO on, cleared, 14-byte threshold
    outb(COM1 + 4, 0x0B); // DTR, RTS, OUT2
}

pub fn write_byte(byte: u8) {
    if byte == b'\n' {
        write_byte(b'\r');
    }
    while inb(COM1 + 5) & 0x20 == 0 {
        core::hint::spin_loop();
    }
    outb(COM1, byte);
}

pub fn write(bytes: &[u8]) {
    for byte in bytes {
        write_byte(*byte);
    }
}

/// A `core::fmt` sink, used where a panic message needs its arguments.
struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        write(text.as_bytes());
        Ok(())
    }
}

pub fn print_fmt(args: core::fmt::Arguments<'_>) {
    let _ = core::fmt::Write::write_fmt(&mut Writer, args);
}
