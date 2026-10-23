// SPDX-License-Identifier: GPL-3.0-only
//! Dallas DS1386 real-time clock.
//!
//! Register *n* is the low byte of the 32-bit word at `0xBFBE_0000 + n*4`
//! (byte lane 3).  The time registers are BCD.  Stop the transfer with the TE
//! bit, read the calendar, then restore TE so the clock keeps counting.

use core::ptr::{read_volatile, write_volatile};

const RTC_BASE: usize = 0xBFBE_0000;

const REG_SECONDS: usize = 0x01;
const REG_MINUTES: usize = 0x02;
const REG_HOURS: usize = 0x04;
const REG_DATE: usize = 0x08;
const REG_MONTH: usize = 0x09;
const REG_YEAR: usize = 0x0A;
const REG_COMMAND: usize = 0x0B;

const TE: u8 = 0x80;

fn reg(index: usize) -> u8 {
    // SAFETY: the DS1386 window is mapped; the low byte is the register.
    unsafe { (read_volatile((RTC_BASE + index * 4) as *const u32) & 0xff) as u8 }
}

fn write_reg(index: usize, value: u8) {
    // SAFETY: the DS1386 window is mapped; the low byte is the register.
    unsafe {
        write_volatile((RTC_BASE + index * 4) as *mut u32, u32::from(value));
    }
}

fn bcd(value: u8) -> u32 {
    u32::from(value >> 4) * 10 + u32::from(value & 0x0f)
}

/// Read the calendar as Unix seconds, or `None` if it is not valid.
pub fn read_unix() -> Option<u64> {
    let command = reg(REG_COMMAND);
    write_reg(REG_COMMAND, command & !TE);

    let seconds = bcd(reg(REG_SECONDS));
    let minutes = bcd(reg(REG_MINUTES));
    let hours_raw = reg(REG_HOURS);
    let hours = if hours_raw & 0x40 != 0 {
        let pm = hours_raw & 0x20 != 0;
        let mut hour = bcd(hours_raw & 0x1f);
        if pm && hour != 12 {
            hour += 12;
        }
        if !pm && hour == 12 {
            hour = 0;
        }
        hour
    } else {
        bcd(hours_raw)
    };
    let date = bcd(reg(REG_DATE));
    let month = bcd(reg(REG_MONTH) & 0x1f);
    let year_short = bcd(reg(REG_YEAR));
    // The DS1386 stores two BCD digits counting from 1940 (range 1940–2039).
    let year = i64::from(year_short) + 1940;

    write_reg(REG_COMMAND, command | TE);

    if month == 0
        || month > 12
        || date == 0
        || date > 31
        || hours > 23
        || minutes > 59
        || seconds > 59
    {
        return None;
    }

    let days = days_from_civil(year, month, date);
    Some(
        (days * 86_400 + i64::from(hours) * 3600 + i64::from(minutes) * 60 + i64::from(seconds))
            as u64,
    )
}

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * i64::from(month_index) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
