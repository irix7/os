// SPDX-License-Identifier: GPL-3.0-only
//! The minimal serial boot monitor.
//!
//! The boot polls the selected ESCC channel for one second after the ATA and
//! module checks.  A byte enters the monitor; silence prints
//! `ppc: monitor timeout` and halts exactly as the non-interactive boot did.
//!
//! A command is one line of printable ASCII, terminated by CR or LF.  The
//! monitor echoes it, with backspace erasing the previous character, and
//! accepts three command names.  There is no history, cursor movement or
//! argument parsing: the command set is fixed and every line is matched as a
//! whole.

use crate::platform::{Channel, Path};
use crate::{halt, print_dec, print_dec64, print_hex, print_line, print_model, serial, timer};

/// How long the boot waits for the first key.  One second of timebase is long
/// enough for a terminal or the gate to answer and short enough that a
/// headless boot still reaches `ppc: halted` promptly.
const WAIT_MS: u32 = 1000;

/// Longest accepted command line, excluding the terminator.  Further
/// printable characters are read and discarded without an echo; the line is
/// then refused as too long instead of being matched in truncated form.
const LINE_CAPACITY: usize = 32;

/// What the module check did, for the `info` command.
pub enum ModuleState {
    /// The disk was not the gate disk, so no module was loaded.
    Skipped,
    /// The image was verified, entered and its vtable accepted.
    Loaded { length: u32, ticked: bool },
}

/// The machine facts the monitor reports.  The boot values are collected
/// before the module is entered; the module status is the loader's report.
/// Every field is a plain copy, so the monitor owns no borrowed device-tree
/// state.
pub struct Info {
    pub macio_base: u32,
    pub macio_size: u32,
    pub macio_path: Path,
    pub console: Option<Channel>,
    pub input: Option<Channel>,
    pub memory_base: u32,
    pub memory_size: u32,
    pub timebase: u32,
    pub ata_model: [u8; 40],
    pub ata_sectors: u64,
    pub module: ModuleState,
}

/// Poll the input channel for a first key, bounded by one second.
pub fn first_key() -> Option<u8> {
    let start = timer::timebase();
    let limit = u64::from(timer::frequency()) * u64::from(WAIT_MS) / 1000;
    let limit = limit.min(u64::from(u32::MAX)) as u32;
    loop {
        if let Some(byte) = serial::read_byte() {
            return Some(byte);
        }
        if timer::timebase().wrapping_sub(start) > limit {
            return None;
        }
        core::hint::spin_loop();
    }
}

/// Run the monitor.  `first` is the key that entered it; it is echoed and
/// treated like any other byte, so a command can begin in the same write that
/// wakes the machine.
pub fn run(info: &Info, first: u8) -> ! {
    print_line(b"ppc: monitor ready");
    if serial::take_overflow() {
        print_line(b"monitor: input overrun");
    }
    serial::write(b"ppc> ");

    let mut line = [0u8; LINE_CAPACITY];
    let mut length = 0;
    let mut overflow = false;
    let mut pending = Some(first);
    loop {
        let byte = match pending.take() {
            Some(byte) => byte,
            None => read_blocking(),
        };
        match byte {
            b'\r' | b'\n' => {
                serial::write_byte(b'\n');
                if overflow {
                    print_line(b"monitor: command too long");
                } else {
                    execute(info, &line[..length]);
                }
                length = 0;
                overflow = false;
                serial::write(b"ppc> ");
            }
            0x08 | 0x7f => {
                if !overflow && length > 0 {
                    length -= 1;
                    serial::write(b"\x08 \x08");
                }
            }
            byte if (0x20..0x7f).contains(&byte) => {
                if length < line.len() {
                    line[length] = byte;
                    length += 1;
                    serial::write_byte(byte);
                } else {
                    // The rest of the line cannot be kept.  Consume it
                    // without an echo and refuse the whole line at CR, so a
                    // paste cannot turn into a valid command by truncation.
                    overflow = true;
                }
            }
            _ => {}
        }
    }
}

fn read_blocking() -> u8 {
    loop {
        if let Some(byte) = serial::read_byte() {
            if serial::take_overflow() {
                print_line(b"monitor: input overrun");
            }
            return byte;
        }
        core::hint::spin_loop();
    }
}

fn execute(info: &Info, command: &[u8]) {
    match command {
        b"" => {}
        b"help" => print_help(),
        b"info" => print_info(info),
        b"halt" => {
            print_line(b"monitor: halt");
            print_line(b"ppc: halted");
            halt();
        }
        _ => {
            serial::write(b"monitor: unknown command: ");
            serial::write(command);
            serial::write_byte(b'\n');
        }
    }
}

fn print_help() {
    print_line(b"monitor: help    list commands");
    print_line(b"monitor: info    machine selection and status");
    print_line(b"monitor: halt    stop the machine");
}

fn print_info(info: &Info) {
    serial::write(b"monitor: macio base=");
    print_hex(info.macio_base);
    serial::write(b" size=");
    print_dec(info.macio_size);
    serial::write(b" path=");
    serial::write(info.macio_path.bytes());
    serial::write(b" console=");
    print_channel(info.console);
    serial::write(b" input=");
    print_channel(info.input);
    serial::write_byte(b'\n');

    serial::write(b"monitor: memory base=");
    print_hex(info.memory_base);
    serial::write(b" size=");
    print_dec(info.memory_size / (1024 * 1024));
    serial::write(b" MiB\n");

    serial::write(b"monitor: timebase=");
    print_dec(info.timebase);
    serial::write(b" Hz\n");

    serial::write(b"monitor: ata model=");
    print_model(&info.ata_model);
    serial::write(b" sectors=");
    print_dec64(info.ata_sectors);
    serial::write_byte(b'\n');

    match info.module {
        ModuleState::Skipped => print_line(b"monitor: module skipped=not-gate-disk"),
        ModuleState::Loaded { length, ticked } => {
            serial::write(b"monitor: module status=0 tick=");
            print_dec(u32::from(ticked));
            serial::write(b" length=");
            print_dec(length);
            serial::write_byte(b'\n');
        }
    }
}

fn print_channel(channel: Option<Channel>) {
    match channel {
        Some(channel) => serial::write_byte(channel.letter()),
        None => serial::write(b"none"),
    }
}
