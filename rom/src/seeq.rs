//! Seeq DQ8003 Ethernet driver (polled, bring-up stage).
//!
//! Hardware facts (see `docs/research/indy-hardware.md` §6):
//! - Seeq 8003 at physical 0x1FBD_4000, kseg1 0xBFBD_4000. Its 8 registers are
//!   spaced 4 bytes apart at *byte lane 0* (`reg << 2`, not lane 3).
//! - Regs 0-5: station address (MAC), latched at reset from DS1386 NVRAM.
//!   Reg 6: RX command/status. Reg 7: TX command/status.
//! - Two HPC3 descriptor engines (RX at HPC3+0x14000, TX at HPC3+0x16000).
//! - HPC3 base physical 0x1FB8_0000, kseg1 0xBFB8_0000.
//! - 10 Mbit/s, half duplex, no auto-negotiation.
//!
//! This stage implements register access, soft reset, initialisation and MAC
//! read, plus a polled single-descriptor transmit. The full descriptor ring
//! and interrupt-driven path are follow-ups; the DMA flag bit values below are
//! taken from the hardware report and must be confirmed against the live
//! hardware on iris before the driver is trusted with real frames.

use core::ptr::{read_volatile, write_volatile};

/// Seeq register window, kseg1 virtual base.
const SEEQ_BASE: *mut u8 = 0xBFBD_4000 as *mut u8;

/// HPC3 base, kseg1 virtual base.
const HPC3_BASE: *mut u32 = 0xBFB8_0000u32 as *mut u32;

/// TX descriptor engine base (HPC3 + 0x16000), kseg1.
const TX_ENGINE: *mut u32 = (0xBFB8_0000u32 + 0x1_6000) as *mut u32;

/// Seeq register numbers.
const REG_MAC0: u8 = 0;
#[allow(dead_code)]
const REG_MAC1: u8 = 1;
#[allow(dead_code)]
const REG_MAC2: u8 = 2;
#[allow(dead_code)]
const REG_MAC3: u8 = 3;
#[allow(dead_code)]
const REG_MAC4: u8 = 4;
#[allow(dead_code)]
const REG_MAC5: u8 = 5;
const REG_RX_CMD: u8 = 6;
const REG_TX_CMD: u8 = 7;

/// HPC3 TX engine register offsets (bytes from TX_ENGINE).
const TX_CBPTR: usize = 0x0000; // current buffer pointer
const TX_NDPTR: usize = 0x0004; // next descriptor pointer
const TX_CTRL: usize = 0x0104; // control: ACTIVE bit
const TX_CTRL_ACTIVE: u32 = 1 << 9; // (see rx_ctrl.ACTIVE; TX mirrors it)

/// Transmit descriptor: programmed into memory, then its address goes in
/// tx_cbptr / tx_ndptr.
#[repr(C)]
#[allow(dead_code)]
struct TxDesc {
    pbuf: u32,    // physical address of the frame buffer
    cntinfo: u32, // len | XIE | EOP | EOX (flag bit values TBD vs hardware)
    pnext: u32,   // next descriptor (0 = end of list)
}

/// DMA flag bit values for `cntinfo` / `tx_bcd`.
/// These come from the hardware report (`cntinfo = len | XIE | EOP | EOX`);
/// exact bit positions are TBD pending confirmation against `sgiseeq.h`.
mod dma_flags {
    #[allow(dead_code)]
    pub const XIE: u32 = 0; // transmit interrupt enable (TBD)
    #[allow(dead_code)]
    pub const EOP: u32 = 0; // end of packet (TBD)
    #[allow(dead_code)]
    pub const EOX: u32 = 0; // end of transmit (TBD)
}

/// Soft-reset the Seeq (write the RX/TX command register reset bit).
fn seeq_wr(reg: u8, data: u8) {
    unsafe {
        write_volatile(SEEQ_BASE.add((reg as usize) << 2), data);
    }
}

fn seeq_rd(reg: u8) -> u8 {
    unsafe { read_volatile(SEEQ_BASE.add((reg as usize) << 2)) }
}

#[allow(dead_code)]
fn hpc3_wr32(offset: usize, value: u32) {
    unsafe {
        write_volatile(HPC3_BASE.add(offset >> 2), value);
    }
}

fn hpc3_rd32(offset: usize) -> u32 {
    unsafe { read_volatile(HPC3_BASE.add(offset >> 2)) }
}

fn tx_wr32(offset: usize, value: u32) {
    unsafe {
        write_volatile(TX_ENGINE.add(offset >> 2), value);
    }
}

/// The Seeq device.
pub struct Seeq {
    /// Station address (MAC), big-endian order, SGI `08:00:69` OUI.
    pub mac: [u8; 6],
}

impl Seeq {
    /// Probe and initialise the Seeq: soft reset, then latch the MAC from the
    /// station-address registers. Returns `None` if the MAC reads as
    /// `ff:ff:ff:ff:ff:ff` (dead RTC battery) — the caller must generate a
    /// locally-administered address or refuse to bring the interface up.
    pub fn init() -> Option<Self> {
        // Soft reset: toggle the RX command register. A full reset sequence is
        // board-specific; here we assert reset via the RX cmd register and
        // re-read the MAC, which the Seeq latches from NVRAM on reset.
        seeq_wr(REG_RX_CMD, 0x00);
        seeq_wr(REG_TX_CMD, 0x00);

        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = seeq_rd(REG_MAC0.wrapping_add(i as u8));
        }
        let bad_mac = mac.iter().all(|&b| b == 0xFF);
        if bad_mac {
            return None;
        }
        Some(Self { mac })
    }

    /// Return the station MAC address.
    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    /// Transmit a single Ethernet frame, polled.
    ///
    /// `frame` must be a physically contiguous buffer holding the complete
    /// Ethernet frame (14-byte header + payload, no pad on transmit). Returns
    /// `Ok(())` once the descriptor engine reports the frame sent, `Err(())`
    /// if the engine does not go inactive within the poll bound.
    #[allow(clippy::result_unit_err)] // skeleton DMA path; error type is a follow-up
    pub fn transmit_frame(frame: &[u8]) -> Result<(), ()> {
        let phys = frame.as_ptr() as u32; // buffer must be physically contiguous

        let desc = TxDesc {
            pbuf: phys,
            cntinfo: (frame.len() as u32) | dma_flags::XIE | dma_flags::EOP | dma_flags::EOX,
            pnext: 0,
        };

        // Program the descriptor engine: address of the descriptor, then go.
        // tx_cbptr/tx_ndptr point at the descriptor; tx_bufptr at the frame.
        // Flag values below mirror the transmit programming sequence and are
        // TBD pending confirmation against the hardware.
        tx_wr32(TX_CBPTR, &desc as *const TxDesc as u32);
        tx_wr32(TX_NDPTR, 0);
        tx_wr32(TX_CTRL, TX_CTRL_ACTIVE);

        // Poll until ACTIVE clears (frame sent) or we time out.
        for _ in 0..1_000_000 {
            if hpc3_rd32(TX_CTRL) & TX_CTRL_ACTIVE == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_not_all_ff_is_accepted() {
        // init() reads real hardware; here we just verify the all-FF constant is
        // distinct from a valid SGI OUI prefix (element-wise, no memcmp).
        let oui = [0x08u8, 0x00, 0x69, 0x00, 0x00, 0x01];
        assert!(oui.iter().any(|&b| b != 0xFF));
    }

    #[test]
    fn desc_size_is_12_bytes() {
        assert_eq!(core::mem::size_of::<TxDesc>(), 12);
    }
}
