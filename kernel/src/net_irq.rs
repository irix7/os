// SPDX-License-Identifier: GPL-3.0-only
//! Interrupt-source policy for the CPU's external interrupt lines.
//!
//! The MIPS III general vector only knows two things about an `EXC_INT`: IP7
//! is the CP0 timer, and every other pending bit is an external line.  This
//! module holds the decode that the vector's Rust half, the Seeq driver and
//! the module scheduler share.  Nothing here touches MMIO or the scheduler,
//! so the policy runs on the host under `kernel/tests/net_irq.rs`.
//!
//! The Cause bits are the MIPS III layout: IP2 is bit 10 and IP7 is bit 15.
//! The HPC3 status bits are `hpc3.h`/`hpcreg.h`: Ethernet device bit 4,
//! Ethernet RX DMA bit 5, Ethernet TX DMA bit 6.  The HPC3 reports bits 4:0 in
//! `istat0` and bits 9:5 in `istat1`; `merge_status` is the documented
//! workaround for that split.
//!
//! Interrupt context may only do three things here: classify the cause,
//! acknowledge the device, and set one deferred flag.  Frame processing,
//! allocation, logging and module calls belong to the thread that drains.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Cause.IP2: IOC local 0 (Ethernet, SCSI, mappable interrupts).
pub const CAUSE_IP2: u32 = 1 << 10;
/// Cause.IP7: CP0 Count/Compare.
pub const CAUSE_IP7: u32 = 1 << 15;

/// HPC3 INTSTAT: the Seeq's own interrupt pin.
pub const INTSTAT_ENET_DEV: u32 = 1 << 4;
/// HPC3 INTSTAT: the Ethernet receive DMA engine completed a descriptor.
pub const INTSTAT_ENET_RX_DMA: u32 = 1 << 5;
/// HPC3 INTSTAT: the Ethernet transmit DMA engine completed a descriptor.
pub const INTSTAT_ENET_TX_DMA: u32 = 1 << 6;

/// Bits `istat0` is documented to report reliably (it is a split register).
pub const INTSTAT0_VALID: u32 = 0x0000_001f;
/// Bits `istat1` (at +0x3_000C) is documented to report reliably.
pub const INTSTAT1_VALID: u32 = 0x0000_03e0;

/// Every Ethernet bit that means "the Seeq has something for the kernel".
///
/// `INTSTAT_ENET_DEV` is the Seeq's own latched pin; the two DMA bits are the
/// HPC3's receive/transmit completion.  Any of them arms the handler.
pub const ENET_SOURCE: u32 = INTSTAT_ENET_DEV | INTSTAT_ENET_RX_DMA | INTSTAT_ENET_TX_DMA;

/// Bits the handler writes back (W1C) after servicing.
///
/// `INTSTAT_ENET_DEV` is deliberately **not** in this mask.  Its ack is
/// `CLRINT`, which drops the Seeq pin; writing the bit clears the status while
/// the pin is still asserted, and the next compare then sees IP2 high with no
/// readable source: a spurious storm that never re-acknowledges.  The DMA bits
/// have no such latch and are cleared by writing one.
pub const ENET_W1C: u32 = INTSTAT_ENET_RX_DMA | INTSTAT_ENET_TX_DMA;

/// The only source the handler for IP2 knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The Seeq or one of its DMA channels.
    Ethernet,
    /// IP2 is asserted but the HPC3 status names no source the kernel owns.
    Spurious,
}

/// What an `EXC_INT` cause word means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    /// IP7: the timer fast path owns this and must never reach Rust.
    Timer,
    /// IP2: an external device line.
    External,
    /// An `EXC_INT` with neither IP7 nor IP2 pending.
    Spurious,
}

/// Classify a `Cause` word with `ExcCode == 0`.
///
/// IP7 is tested first: a tick that shares the word with a device line must
/// still acknowledge Compare and count the tick, or every `wait` in the
/// system would be re-armed by the device line.  The level-triggered IP2
/// remains asserted and is taken immediately after the `eret`.
pub const fn classify(cause: u32) -> Cause {
    if cause & CAUSE_IP7 != 0 {
        Cause::Timer
    } else if cause & CAUSE_IP2 != 0 {
        Cause::External
    } else {
        Cause::Spurious
    }
}

/// Synthesise the 10-bit HPC3 status from its two split registers.
pub const fn merge_status(istat0: u32, istat1: u32) -> u32 {
    (istat0 & INTSTAT0_VALID) | (istat1 & INTSTAT1_VALID)
}

/// The source named by a merged HPC3 status word.
pub const fn source(status: u32) -> Source {
    if status & ENET_SOURCE != 0 {
        Source::Ethernet
    } else {
        Source::Spurious
    }
}

/// The status bits the handler writes back (W1C) after servicing `status`.
///
/// Never includes the device pin; see [`ENET_W1C`].
pub const fn ack_bits(status: u32) -> u32 {
    status & ENET_W1C
}

/// A module TCP slot created outside any module call.
pub const NO_OWNER: u8 = 0xFF;

/// TCP readiness bits that require the owning module to run: RX data or a
/// peer close.
///
/// `EV_TX` is deliberately excluded.  It means "a send was short and the
/// socket has room now", which stays true until the next send: a level wake
/// there makes an idle node spin at the short-wait cadence forever.  The
/// peer's acknowledgment arrives as a received frame, so the receive bit
/// already wakes the owner when transmit space actually returns.
/// `EV_LISTEN`/`EV_ESTABLISHED` are state, not work.
pub const WAKE_BITS: u32 = (1 << 0) | (1 << 2);

/// Whether a slot's readiness mask should wake its owner.
pub const fn slot_ready(mask: u32) -> bool {
    mask & WAKE_BITS != 0
}

/// Bit `n` set when module region `n` owns a ready slot.
///
/// `owners[slot]` is [`NO_OWNER`] for a socket the kernel cannot attribute to
/// a module call.  An unowned ready slot wakes every region: the scheduler
/// cannot know which module needs it, and staying conservative costs one
/// extra tick rather than a stalled socket.  `regions` above 64 is clamped.
pub fn ready_regions(owners: &[u8], ready: &[bool], regions: usize) -> u64 {
    let regions = regions.min(64);
    let mut mask = 0u64;
    for (slot, &owner) in owners.iter().enumerate().take(ready.len()) {
        if !ready[slot] {
            continue;
        }
        if owner == NO_OWNER {
            if regions == 64 {
                mask = u64::MAX;
            } else {
                mask = (1u64 << regions) - 1;
            }
            continue;
        }
        let region = owner as usize;
        if region < regions {
            mask |= 1u64 << region;
        }
    }
    mask
}

/// Backstop for an interrupt the acknowledge cannot clear.
///
/// A receive-buffer overflow is the documented case: the HPC3 re-asserts the
/// Seeq line after `CLRINT` by design, so returning from the handler would
/// trap again before any thread ran.  [`kernel::net`] detects that overflow
/// directly and parks at once; this bound catches any other source that
/// survives its ack so a bug cannot livelock the machine.  Every thread-side
/// poll resets the count, so real traffic never reaches it.
pub const MAX_IRQ_STREAK: u32 = 64;

/// Whether the handler must park the line instead of returning to it.
pub const fn must_park(streak: u32) -> bool {
    streak > MAX_IRQ_STREAK
}

static RX_PENDING: AtomicBool = AtomicBool::new(false);
static RX_IRQS: AtomicU32 = AtomicU32::new(0);
static SPURIOUS: AtomicU32 = AtomicU32::new(0);
static UNEXPECTED: AtomicU32 = AtomicU32::new(0);
static PARKED: AtomicU32 = AtomicU32::new(0);
static PARKED_LINE: AtomicBool = AtomicBool::new(false);
static STREAK: AtomicU32 = AtomicU32::new(0);

/// Counters read by `status::text` and the gate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// External interrupts that found the Ethernet source.
    pub rx: u32,
    /// IP2 interrupts whose HPC3 status named no owned source.
    pub spurious: u32,
    /// `EXC_INT` causes with neither IP7 nor IP2 pending.
    pub unexpected: u32,
    /// Times the handler parked `Status.IM2` because the source survived it.
    pub parked: u32,
    /// Whether the interrupt is currently unmasked.
    pub armed: bool,
}

/// Deferred-work flag: set in interrupt context, taken by the drain path.
pub fn set_pending() {
    RX_PENDING.store(true, Ordering::Release);
}

/// Take the deferred flag.
pub fn take_pending() -> bool {
    RX_PENDING.swap(false, Ordering::AcqRel)
}

/// Whether a wake is already deferred.
pub fn pending() -> bool {
    RX_PENDING.load(Ordering::Acquire)
}

/// Record one serviced Ethernet interrupt.
pub fn note_rx() {
    RX_IRQS.fetch_add(1, Ordering::Relaxed);
    set_pending();
}

/// Record one IP2 with no owned HPC3 source.
pub fn note_spurious() {
    SPURIOUS.fetch_add(1, Ordering::Relaxed);
}

/// Record one `EXC_INT` that named neither IP7 nor IP2.
pub fn note_unexpected() {
    UNEXPECTED.fetch_add(1, Ordering::Relaxed);
}

/// Record that the handler parked the line.
pub fn note_parked() {
    PARKED.fetch_add(1, Ordering::Relaxed);
    PARKED_LINE.store(true, Ordering::Release);
}

/// Whether the handler parked the line and the drain has not re-armed it.
pub fn take_parked() -> bool {
    PARKED_LINE.swap(false, Ordering::AcqRel)
}

/// Advance the without-a-drain streak and return it.
pub fn bump_streak() -> u32 {
    STREAK.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

/// Clear the streak; the drain path calls this after it has taken frames.
pub fn reset_streak() {
    STREAK.store(0, Ordering::Relaxed);
}

/// Current counters.
pub fn stats(armed: bool) -> Stats {
    Stats {
        rx: RX_IRQS.load(Ordering::Relaxed),
        spurious: SPURIOUS.load(Ordering::Relaxed),
        unexpected: UNEXPECTED.load(Ordering::Relaxed),
        parked: PARKED.load(Ordering::Relaxed),
        armed,
    }
}

/// Clear every counter and the deferred flag.  The boot self-test uses this
/// so the first counted interrupt is the first real one.
pub fn reset_stats() {
    RX_IRQS.store(0, Ordering::Relaxed);
    SPURIOUS.store(0, Ordering::Relaxed);
    UNEXPECTED.store(0, Ordering::Relaxed);
    PARKED.store(0, Ordering::Relaxed);
    PARKED_LINE.store(false, Ordering::Relaxed);
    STREAK.store(0, Ordering::Relaxed);
    RX_PENDING.store(false, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_wins_a_shared_cause() {
        assert_eq!(classify(CAUSE_IP7), Cause::Timer);
        assert_eq!(classify(CAUSE_IP7 | CAUSE_IP2), Cause::Timer);
    }

    #[test]
    fn ip2_alone_is_external() {
        assert_eq!(classify(CAUSE_IP2), Cause::External);
        assert_eq!(classify(CAUSE_IP2 | (1 << 12)), Cause::External);
    }

    #[test]
    fn a_bare_external_interrupt_is_spurious() {
        assert_eq!(classify(0), Cause::Spurious);
        assert_eq!(classify(1 << 11), Cause::Spurious);
    }

    #[test]
    fn the_split_hpc3_status_merges_both_halves() {
        // istat0 only reports bits 4:0, istat1 only bits 9:5.
        assert_eq!(merge_status(0xffff_ffff, 0), 0x1f);
        assert_eq!(merge_status(0, 0xffff_ffff), 0x3e0);
        assert_eq!(
            merge_status(INTSTAT_ENET_DEV, 0),
            INTSTAT_ENET_DEV,
            "device bit lives in istat0"
        );
        assert_eq!(
            merge_status(0, INTSTAT_ENET_RX_DMA | INTSTAT_ENET_TX_DMA),
            INTSTAT_ENET_RX_DMA | INTSTAT_ENET_TX_DMA,
            "DMA bits live in istat1"
        );
    }

    #[test]
    fn a_status_without_an_owned_source_is_spurious() {
        assert_eq!(source(0), Source::Spurious);
        assert_eq!(source(INTSTAT_ENET_DEV), Source::Ethernet);
        assert_eq!(source(INTSTAT_ENET_RX_DMA), Source::Ethernet);
        assert_eq!(source(INTSTAT_ENET_TX_DMA), Source::Ethernet);
    }

    #[test]
    fn the_device_pin_is_acknowledged_only_through_clrint() {
        // A W1C of the device bit would clear the status while the Seeq pin
        // is still asserted, which is the spurious-storm bug.
        assert_eq!(ack_bits(0xffff_ffff), ENET_W1C);
        assert_eq!(ack_bits(INTSTAT_ENET_DEV), 0);
        assert_eq!(
            ack_bits(INTSTAT_ENET_DEV | INTSTAT_ENET_RX_DMA),
            INTSTAT_ENET_RX_DMA
        );
        assert_eq!(source(INTSTAT_ENET_DEV), Source::Ethernet);
    }

    #[test]
    fn only_receive_and_close_wake_a_region() {
        assert!(!slot_ready(1 << 3), "listen is state, not work");
        assert!(!slot_ready(1 << 4), "established is state, not work");
        assert!(!slot_ready(1 << 31), "no socket is not work");
        // A recovered-TX level would stay set until the next send.
        assert!(!slot_ready(1 << 1), "recovered TX must not be a level wake");
        assert!(slot_ready(1 << 0));
        assert!(slot_ready(1 << 2));
    }

    #[test]
    fn only_the_ready_slot_owner_is_woken() {
        let owners = [0u8, 1, NO_OWNER];
        let ready = [true, false, false];
        assert_eq!(ready_regions(&owners, &ready, 5), 1 << 0);
        let owners = [0u8, 3, NO_OWNER];
        let ready = [false, true, false];
        assert_eq!(ready_regions(&owners, &ready, 5), 1 << 3);
    }

    #[test]
    fn an_unowned_ready_slot_wakes_every_region() {
        let owners = [NO_OWNER];
        let ready = [true];
        assert_eq!(ready_regions(&owners, &ready, 3), 0b111);
        assert_eq!(ready_regions(&owners, &ready, 64), u64::MAX);
    }

    #[test]
    fn an_owner_outside_the_table_is_ignored() {
        let owners = [9u8];
        let ready = [true];
        assert_eq!(ready_regions(&owners, &ready, 3), 0);
    }

    #[test]
    fn only_a_long_undrained_streak_parks_the_line() {
        assert!(!must_park(1));
        assert!(!must_park(MAX_IRQ_STREAK));
        assert!(must_park(MAX_IRQ_STREAK + 1));
    }
}
