// SPDX-License-Identifier: GPL-3.0-only
//! WD33C93 SCSI controller bring-up.
//!
//! The controller is in the HPC3 external window at kseg1 `0xBFBC_4000`; the
//! indirect address register is byte lane 3 (`+3`) and the data register is
//! byte lane 3 of the next word (`+7`), matching Linux `sgiwd93.c`
//! (`regs.SASR = wdregs + 3`, `regs.SCMD = wdregs + 7`).
//!
//! This module only resets the channel and the chip, then reads the reset
//! status and microcode revision.  Selection and block I/O come next.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering};

/// WD33C93 external register window (kseg1).
const BASE: usize = 0xBFBC_4000;
/// HPC3 SCSI channel 0 DMA control (kseg1, HPC3 + 0x1_1004).
const HPC3_SCSI_CTRL: *mut u32 = 0xBFB9_1004 as *mut u32;
/// `HPC3_SCTRL_CRESET`: reset the DMA channel and the external controller.
const HPC3_SCTRL_CRESET: u32 = 0x40;

const ASR_INT: u8 = 0x80;
const ASR_CIP: u8 = 0x10;

const REG_OWN_ID: u8 = 0x00;
const REG_CONTROL: u8 = 0x01;
const REG_CDB1: u8 = 0x03;
const REG_RSELID: u8 = 0x16;
const REG_CSR: u8 = 0x17;
const REG_COMMAND: u8 = 0x18;
const REG_SYN: u8 = 0x11;

const CMD_ABORT: u8 = 0x01;
const CMD_RESET: u8 = 0x00;

const ID_EAF: u8 = 0x08;
const ID_RAF: u8 = 0x20;
const OWN_ID: u8 = 7;
const CONTROL_EDI_IDI: u8 = 0x0c;
const RSELID_ER: u8 = 0x80;

/// Result of the controller probe.
#[derive(Clone, Copy, Debug)]
pub struct Probe {
    /// Auxiliary status before reset.
    pub asr_before: u8,
    /// Status after the chip reset.
    pub csr: u8,
    /// Microcode revision (CDB1 after reset).
    pub revision: u8,
    /// Own SCSI ID read back.
    pub own_id: u8,
    /// Auxiliary status after initialisation.
    pub asr_after: u8,
}

fn delay() {
    for _ in 0..10_000 {
        core::hint::spin_loop();
    }
}

fn read_asr() -> u8 {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 3) as *const u8) }
}

fn write_reg(register: u8, value: u8) {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 3) as *mut u8, register);
        compiler_fence(Ordering::SeqCst);
        for _ in 0..8 {
            core::hint::spin_loop();
        }
        write_volatile((BASE + 7) as *mut u8, value);
    }
}

fn read_reg(register: u8) -> u8 {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 3) as *mut u8, register);
        compiler_fence(Ordering::SeqCst);
        for _ in 0..8 {
            core::hint::spin_loop();
        }
        read_volatile((BASE + 7) as *const u8)
    }
}

fn wait_cip_clear() -> Result<(), &'static str> {
    for _ in 0..200_000 {
        if read_asr() & ASR_CIP == 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("CIP timeout")
}

fn wait_interrupt() -> Result<(), &'static str> {
    for _ in 0..200_000 {
        if read_asr() & ASR_INT != 0 {
            return Ok(());
        }
        core::hint::spin_loop();
    }
    Err("interrupt timeout")
}

/// Reset the HPC3 SCSI channel and the WD33C93, then read back its identity.
pub fn probe() -> Result<Probe, &'static str> {
    let asr_before = read_asr();

    // Reset the DMA channel and the external controller together.
    // SAFETY: fixed HPC3 MMIO register.
    unsafe {
        write_volatile(HPC3_SCSI_CTRL, HPC3_SCTRL_CRESET);
    }
    delay();
    // SAFETY: fixed HPC3 MMIO register.
    unsafe {
        write_volatile(HPC3_SCSI_CTRL, 0);
    }
    delay();

    write_reg(REG_COMMAND, CMD_ABORT);
    wait_cip_clear()?;

    // Own ID 7; advanced and really-advanced features (as NetBSD does).
    write_reg(REG_OWN_ID, OWN_ID | ID_EAF | ID_RAF);
    write_reg(REG_COMMAND, CMD_RESET);
    delay();
    wait_interrupt()?;
    let csr = read_reg(REG_CSR);
    let revision = read_reg(REG_CDB1);

    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    write_reg(REG_RSELID, RSELID_ER);
    write_reg(REG_SYN, 0);

    let own_id = read_reg(REG_OWN_ID);
    let asr_after = read_asr();
    Ok(Probe {
        asr_before,
        csr,
        revision,
        own_id,
        asr_after,
    })
}
