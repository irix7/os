// SPDX-License-Identifier: GPL-3.0-only
//! External interrupts.
//!
//! The machine's interrupt controller is the Heathrow block at the start of
//! the Mac I/O window; the external interrupt exception is vector 0x500.  The
//! deterministic source is timer 1 in the MOS 6522 VIA, whose line number and
//! register window both come from the device tree.  The VIA is a CUDA on a
//! beige G3 and QEMU's `g3beige`, and the PMU interface on the PowerBook G3
//! M4753; both program the same timer registers.
//!
//! The handler reads the controller's pending-event register, clears the
//! timer flag that raised the event, writes the event bit back to the
//! acknowledge port and counts the interrupt.  Nothing else is unmasked, so a
//! pending bit other than the timer's is a bug and stops the boot.

use core::sync::atomic::{AtomicU32, Ordering};

use crate::pic::Controller;
use crate::via::Timer;
use crate::{console, halt, print_dec, print_hex, print_line};

/// Timer 1 period in VIA clocks: one millisecond minus the timer's two
/// reload counts.  Both the CUDA and the PMU VIA divide the 4.7 MHz clock by
/// six; Linux's `via_calibrate_decr` uses the same rate for either node.
const PERIOD_TICKS: u16 = (crate::via::T1_HZ / 1000 - 2) as u16;

/// Interrupts to collect before the gate reports success.
pub const TARGET: u32 = 10;

static PIC_BASE: AtomicU32 = AtomicU32::new(0);
static TIMER_BASE: AtomicU32 = AtomicU32::new(0);
static LINE: AtomicU32 = AtomicU32::new(0);
static COUNT: AtomicU32 = AtomicU32::new(0);

pub fn count() -> u32 {
    COUNT.load(Ordering::Relaxed)
}

fn pic() -> Controller {
    Controller::new(PIC_BASE.load(Ordering::Relaxed))
}

fn timer() -> Timer {
    Timer::new(TIMER_BASE.load(Ordering::Relaxed))
}

fn line() -> u32 {
    LINE.load(Ordering::Relaxed)
}

/// Mask the controller, program the timer and leave the line disabled.
pub fn prepare(pic_base: u32, timer_base: u32, line: u32) {
    assert!(line < 64, "interrupt line outside the Heathrow controller");
    PIC_BASE.store(pic_base, Ordering::Relaxed);
    TIMER_BASE.store(timer_base, Ordering::Relaxed);
    LINE.store(line, Ordering::Relaxed);

    let controller = Controller::new(pic_base);
    controller.reset();
    if controller.mask(line) != 0 {
        print_line(b"ppc: interrupt controller did not reset");
        halt();
    }
    Timer::new(timer_base).program(PERIOD_TICKS);

    console::write(b"ppc: irq controller=");
    print_hex(pic_base);
    console::write(b" timer=");
    print_hex(timer_base);
    console::write(b" line=");
    print_hex(line);
    console::write(b" period=");
    print_dec(u32::from(PERIOD_TICKS));
    console::write_byte(b'\n');
}

/// Start the timer and unmask its controller line.
pub fn start() {
    timer().start(PERIOD_TICKS);
    pic().enable(line());
}

/// Stop the source, mask the line and return the count with the state left
/// pending at the controller and the timer.
pub fn stop() -> (u32, u32, u32) {
    let controller = pic();
    controller.disable(line());
    let source = timer();
    source.stop();
    let latched = controller.events(line());
    controller.acknowledge(line(), latched);
    let pending = controller.events(line());
    (count(), pending, u32::from(source.ifr()))
}

/// Handle one external interrupt and return the value for the interrupted r3.
///
/// SRR0 is the instruction the interrupt was taken before, not a faulting
/// instruction, so the caller resumes there unchanged.
pub fn dispatch(srr0: u32, srr1: u32, call: u32) -> u32 {
    let line = line();
    let bit = 1 << (line & 31);
    let controller = pic();
    let events = controller.events(line);

    if events & bit == 0 {
        console::write(b"ppc: irq without a pending event events=");
        print_hex(events);
        console::write_byte(b'\n');
        print_line(b"ppc: halted");
        halt();
    }

    // Drop the timer's flag first so the source line falls, then clear the
    // event latched at the controller.  That write is the end of interrupt.
    // The timer runs continuously, so it can raise a fresh event between the
    // acknowledge and any later read of the pending register.  That is the
    // next interrupt, not a failed acknowledge; leave it for the vector.
    timer().acknowledge();
    controller.acknowledge(line, bit);

    let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    console::write(b"ppc: irq vector=0x500 line=");
    print_hex(line);
    console::write(b" count=");
    print_dec(count);
    console::write(b" srr0=");
    print_hex(srr0);
    console::write(b" srr1=");
    print_hex(srr1);
    console::write_byte(b'\n');

    // The probe is a fixed count, and drawing the report on the framebuffer
    // makes the handler slower than the one-millisecond timer.  Stop the
    // source as soon as the target is reached so the count cannot run past
    // it while the main context waits to observe it.  `stop` repeats the
    // disable and acknowledge, so its final pending-event check still holds.
    if count >= TARGET {
        controller.disable(line);
        timer().stop();
    }
    call
}
