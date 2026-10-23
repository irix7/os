// SPDX-License-Identifier: GPL-3.0-only
//! WD33C93 SCSI controller bring-up.
//!
//! The controller is in the HPC3 external window at kseg1 `0xBFBC_4000`; the
//! indirect address register is byte lane 3 (`+3`) and the data register is
//! byte lane 3 of the next word (`+7`), matching Linux `sgiwd93.c`
//! (`regs.SASR = wdregs + 3`, `regs.SCMD = wdregs + 7`).
//!
//! The address register auto-increments after most data-port accesses, so
//! every data byte must reselect the DATA register (0x19).  Writing a byte
//! with the address register left at COMMAND (0x18) is interpreted as a
//! command; the identify byte 0x80 would reset the chip.  The console command
//! resets the chip, selects target 1, issues a polled INQUIRY and reads block 0
//! with READ(6); descriptor DMA and the block-device cell come next.

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
const ASR_DBR: u8 = 0x01;

const REG_OWN_ID: u8 = 0x00;
const REG_CONTROL: u8 = 0x01;
const REG_TIMEO: u8 = 0x02;
const REG_CDB1: u8 = 0x03;
const REG_CMD_PHASE: u8 = 0x10;
const REG_SYN: u8 = 0x11;
const REG_COUNT_HI: u8 = 0x12;
const REG_COUNT_MED: u8 = 0x13;
const REG_COUNT_LO: u8 = 0x14;
const REG_SELID: u8 = 0x15;
const REG_RSELID: u8 = 0x16;
const REG_CSR: u8 = 0x17;
const REG_COMMAND: u8 = 0x18;
const REG_DATA: u8 = 0x19;

const CMD_ABORT: u8 = 0x01;
const CMD_RESET: u8 = 0x00;
const CMD_SEL_ATN: u8 = 0x06;
const CMD_SEL_ATN_XFER: u8 = 0x08;
const CMD_XFER_INFO: u8 = 0x20;

const ID_EAF: u8 = 0x08;
const ID_RAF: u8 = 0x20;
const OWN_ID: u8 = 7;
const CONTROL_EDI_IDI: u8 = 0x0c;
const RSELID_ER: u8 = 0x80;
/// `SBIC_TIMEOUT(250, 20)`: 250 ms at a 20 MHz input clock.
const TIMEOUT: u8 = 63;

const CSR_PHASE_MASK: u8 = 0x07;
const PHASE_DATA_IN: u8 = 0x01;
const PHASE_CMD: u8 = 0x02;
const PHASE_MESG_OUT: u8 = 0x06;
const CSR_PHASE_MISMATCH: u8 = 0x88;
const CSR_SEL_TIMEO: u8 = 0x42;
const CSR_S_XFERRED: u8 = 0x16;
const CSR_DISC: u8 = 0x41;
const CSR_DISC_1: u8 = 0x85;
const CMD_PHASE_COMPLETE: u8 = 0x46;

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

fn set_ar(register: u8) {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 3) as *mut u8, register);
    }
    crate::cache::sync();
    for _ in 0..8 {
        core::hint::spin_loop();
    }
}

fn read_asr() -> u8 {
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 3) as *const u8) }
}

fn write_reg(register: u8, value: u8) {
    set_ar(register);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 7) as *mut u8, value);
    }
}

fn read_reg(register: u8) -> u8 {
    set_ar(register);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 7) as *const u8) }
}

fn write_data(value: u8) {
    // The data register is 0x19, not necessarily the current AR after a
    // status read or a command write.
    set_ar(REG_DATA);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe {
        write_volatile((BASE + 7) as *mut u8, value);
    }
}

fn read_data() -> u8 {
    set_ar(REG_DATA);
    // SAFETY: the WD33C93 external window is a fixed MMIO range in kseg1.
    unsafe { read_volatile((BASE + 7) as *const u8) }
}

/// Write the 24-bit transfer count.  Each byte reselects its register instead
/// of relying on the chip's auto-increment.
fn put_count(count: u32) {
    write_reg(REG_COUNT_HI, (count >> 16) as u8);
    write_reg(REG_COUNT_MED, (count >> 8) as u8);
    write_reg(REG_COUNT_LO, count as u8);
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

fn read_csr_ack() -> u8 {
    let mut csr = read_reg(REG_CSR);
    // A status read acknowledges the current interrupt.  If another status is
    // queued the chip re-asserts INT immediately; drain it before returning.
    for _ in 0..8 {
        delay();
        if read_asr() & ASR_INT == 0 {
            break;
        }
        csr = read_reg(REG_CSR);
    }
    csr
}

fn xfer(len: usize, buf: &mut [u8], outbound: bool) -> Result<u8, &'static str> {
    write_reg(REG_CONTROL, CONTROL_EDI_IDI);
    put_count(len as u32);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_XFER_INFO);
    wait_cip_clear()?;

    let mut done = 0usize;
    let mut spins = 0u32;
    loop {
        let asr = read_asr();
        if asr & ASR_DBR != 0 {
            if done < len {
                if outbound {
                    write_data(buf[done]);
                } else {
                    buf[done] = read_data();
                }
                done += 1;
            } else if !outbound {
                let _ = read_data();
            }
            spins = 0;
            continue;
        }
        if asr & ASR_INT != 0 {
            break;
        }
        spins += 1;
        if spins > 1_000_000 {
            put_count(0);
            return Err("data timeout");
        }
        core::hint::spin_loop();
    }

    let csr = read_csr_ack();
    delay();
    put_count(0);
    if done < len {
        return Err("short data transfer");
    }
    Ok(csr)
}

fn select(target: u8) -> Result<u8, &'static str> {
    put_count(0);
    write_reg(REG_SELID, target);
    write_reg(REG_TIMEO, TIMEOUT);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_SEL_ATN);
    wait_cip_clear()?;

    let mut interrupts = 0u32;
    let mut spins = 0u32;
    loop {
        let asr = read_asr();
        if asr & ASR_INT != 0 {
            let csr = read_csr_ack();
            if csr == CSR_SEL_TIMEO {
                return Err("selection timeout");
            }
            if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT)
                || csr == (CSR_PHASE_MISMATCH | PHASE_CMD)
            {
                return Ok(csr);
            }
            interrupts += 1;
            if interrupts > 8 {
                return Err("unexpected selection status");
            }
            delay();
            continue;
        }
        spins += 1;
        if spins > 2_000_000 {
            return Err("selection interrupt timeout");
        }
        core::hint::spin_loop();
    }
}

fn send_identify() -> Result<(), &'static str> {
    let mut identify = [0x80u8]; // identify, LUN 0, no disconnect
    let csr = xfer(1, &mut identify, true)?;
    if csr & CSR_PHASE_MASK != PHASE_CMD {
        return Err("no command phase after identify");
    }
    Ok(())
}

fn send_cdb() -> Result<(), &'static str> {
    // INQUIRY, allocation length 36.
    let mut cdb = [0x12u8, 0, 0, 0, 36, 0];
    let csr = xfer(cdb.len(), &mut cdb, true)?;
    if csr & CSR_PHASE_MASK != PHASE_DATA_IN {
        return Err("no data-in phase after INQUIRY");
    }
    Ok(())
}

fn finish() -> Result<(), &'static str> {
    put_count(0);
    write_reg(REG_CMD_PHASE, CMD_PHASE_COMPLETE);
    wait_cip_clear()?;
    write_reg(REG_COMMAND, CMD_SEL_ATN_XFER);
    wait_cip_clear()?;

    for _ in 0..8 {
        wait_interrupt()?;
        let csr = read_csr_ack();
        delay();
        if csr == CSR_DISC || csr == CSR_DISC_1 || csr == CSR_S_XFERRED {
            return Ok(());
        }
    }
    Err("no disconnect after status")
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
    write_reg(REG_TIMEO, TIMEOUT);

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

/// Issue a polled INQUIRY to `target` and return the 36-byte response.
pub fn inquiry(target: u8) -> Result<[u8; 36], &'static str> {
    let csr = select(target)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        send_identify()?;
    }
    send_cdb()?;

    let mut data = [0u8; 36];
    let csr = xfer(data.len(), &mut data, false)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        return Ok(data);
    }
    let _ = finish();
    Ok(data)
}

/// Read one 512-byte sector from `target` with a polled READ(6).
pub fn read_sector(target: u8, lba: u32, buf: &mut [u8; 512]) -> Result<(), &'static str> {
    let csr = select(target)?;
    if csr == (CSR_PHASE_MISMATCH | PHASE_MESG_OUT) {
        send_identify()?;
    }

    let mut cdb = [
        0x08, // READ(6)
        ((lba >> 16) & 0x1f) as u8,
        (lba >> 8) as u8,
        lba as u8,
        1, // one block
        0,
    ];
    let csr = xfer(cdb.len(), &mut cdb, true)?;
    if csr & CSR_PHASE_MASK != PHASE_DATA_IN {
        return Err("no data-in phase after READ(6)");
    }

    let csr = xfer(buf.len(), buf, false)?;
    if csr == CSR_S_XFERRED || csr == CSR_DISC || csr == CSR_DISC_1 {
        return Ok(());
    }
    let _ = finish();
    Ok(())
}
