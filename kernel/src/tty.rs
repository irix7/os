// SPDX-License-Identifier: GPL-3.0-only
//! Console terminal: termios state and the input line discipline.
//!
//! The console descriptor is one terminal.  Input bytes from the PS/2 keyboard
//! and the serial receiver pass through [`Tty::input`], which applies the
//! `c_iflag` translations, generates signals for the `ISIG` characters, edits
//! and echoes the line in canonical mode, and queues what `read` may return.
//!
//! Canonical mode returns one line per `read`, including its newline; `VEOF`
//! at the start of a line is a zero-length read.  Non-canonical mode returns
//! the bytes that are queued; `VMIN` and `VTIME` are not interpreted.
//!
//! Output processing (`OPOST`/`ONLCR`) is carried in the termios state but the
//! console writer is not switched by it: the serial and graphics consoles
//! receive the bytes a program writes.

use native_abi::{
    termios, B9600, CREAD, CS8, ECHO, ECHOE, ECHOK, ECHONL, ICANON, ICRNL, IEXTEN, IGNCR, INLCR,
    ISIG, ISTRIP, IXON, NOFLSH, ONLCR, OPOST, SIGINT, SIGQUIT, VEOF, VEOL, VERASE, VINTR, VKILL,
    VMIN, VQUIT, VSTART, VSTOP, VSUSP, VWERASE,
};

/// Bytes queued for `read`.
const QUEUE_CAPACITY: usize = 1024;
/// The longest line canonical mode edits, newline included.
const LINE_CAPACITY: usize = 256;
/// Completed canonical records waiting for `read`.
const MAX_RECORDS: usize = 32;

/// The power-on terminal state: canonical input with echo and signals.
pub const fn default_termios() -> termios {
    let mut c_cc = [0u8; 32];
    c_cc[VINTR] = 0x03;
    c_cc[VQUIT] = 0x1C;
    c_cc[VERASE] = 0x7F;
    c_cc[VKILL] = 0x15;
    c_cc[VEOF] = 0x04;
    c_cc[VMIN] = 1;
    c_cc[VSTART] = 0x11;
    c_cc[VSTOP] = 0x13;
    c_cc[VSUSP] = 0x1A;
    c_cc[VWERASE] = 0x17;
    termios {
        c_iflag: ICRNL | IXON,
        c_oflag: OPOST | ONLCR,
        c_cflag: B9600 | CS8 | CREAD,
        c_lflag: ISIG | ICANON | ECHO | ECHOE | ECHOK | IEXTEN,
        c_line: 0,
        c_cc,
        _pad: [0; 3],
        c_ispeed: B9600,
        c_ospeed: B9600,
    }
}

/// One terminal's line discipline.
pub struct Tty {
    termios: termios,
    /// Ring of bytes `read` may return.
    queue: [u8; QUEUE_CAPACITY],
    queue_head: usize,
    queue_len: usize,
    /// Lengths of the completed canonical records in `queue`, oldest first.
    /// A zero-length record is an end-of-file mark.
    records: [u16; MAX_RECORDS],
    record_head: usize,
    record_count: usize,
    /// The line being edited in canonical mode.
    line: [u8; LINE_CAPACITY],
    line_len: usize,
    /// Foreground process group; terminal signals go to it.
    foreground: u32,
}

impl Tty {
    pub const fn new() -> Self {
        Self {
            termios: default_termios(),
            queue: [0; QUEUE_CAPACITY],
            queue_head: 0,
            queue_len: 0,
            records: [0; MAX_RECORDS],
            record_head: 0,
            record_count: 0,
            line: [0; LINE_CAPACITY],
            line_len: 0,
            foreground: 0,
        }
    }

    pub fn termios(&self) -> termios {
        self.termios
    }

    /// Install new attributes.  `flush` discards unread input first.
    pub fn set_termios(&mut self, value: termios, flush: bool) {
        let was_canonical = self.canonical();
        self.termios = value;
        if flush {
            self.flush_input();
            return;
        }
        if was_canonical && !self.canonical() {
            // The edited line and the record boundaries have no meaning in
            // raw mode; every byte typed so far becomes readable.
            for index in 0..self.line_len {
                let byte = self.line[index];
                self.push(byte);
            }
            self.line_len = 0;
            self.record_count = 0;
        } else if !was_canonical && self.canonical() && self.queue_len != 0 {
            self.record_count = 0;
            self.push_record(self.queue_len);
        }
    }

    pub fn foreground(&self) -> u32 {
        self.foreground
    }

    pub fn set_foreground(&mut self, group: u32) {
        self.foreground = group;
    }

    fn canonical(&self) -> bool {
        self.termios.c_lflag & ICANON != 0
    }

    fn flush_input(&mut self) {
        self.queue_head = 0;
        self.queue_len = 0;
        self.record_head = 0;
        self.record_count = 0;
        self.line_len = 0;
    }

    fn push(&mut self, byte: u8) -> bool {
        if self.queue_len == QUEUE_CAPACITY {
            return false;
        }
        let tail = (self.queue_head + self.queue_len) % QUEUE_CAPACITY;
        self.queue[tail] = byte;
        self.queue_len += 1;
        true
    }

    fn push_record(&mut self, length: usize) -> bool {
        if self.record_count == MAX_RECORDS {
            return false;
        }
        let tail = (self.record_head + self.record_count) % MAX_RECORDS;
        self.records[tail] = length as u16;
        self.record_count += 1;
        true
    }

    /// Whether a completed line can be queued without losing bytes.
    fn room_for_line(&self) -> bool {
        self.record_count < MAX_RECORDS && QUEUE_CAPACITY - self.queue_len >= self.line_len + 1
    }

    /// Whether `read` would return without waiting.
    pub fn readable(&self) -> bool {
        if self.canonical() {
            self.record_count != 0
        } else {
            self.queue_len != 0
        }
    }

    /// Copy queued input into `out`.
    ///
    /// `None` means nothing is ready.  `Some(0)` is end-of-file from `VEOF`.
    pub fn read(&mut self, out: &mut [u8]) -> Option<usize> {
        if !self.readable() {
            return None;
        }
        let limit = if self.canonical() {
            let record = self.records[self.record_head] as usize;
            let count = record.min(out.len());
            if count == record {
                self.record_head = (self.record_head + 1) % MAX_RECORDS;
                self.record_count -= 1;
            } else {
                self.records[self.record_head] = (record - count) as u16;
            }
            count
        } else {
            self.queue_len.min(out.len())
        };
        for slot in out[..limit].iter_mut() {
            *slot = self.queue[self.queue_head];
            self.queue_head = (self.queue_head + 1) % QUEUE_CAPACITY;
        }
        self.queue_len -= limit;
        Some(limit)
    }

    fn echo_byte(&self, byte: u8, echo: &mut dyn FnMut(&[u8])) {
        if self.termios.c_lflag & ECHO == 0 {
            return;
        }
        match byte {
            b'\n' => echo(b"\r\n"),
            b'\t' => echo(b"\t"),
            0..=0x1F => echo(&[b'^', byte + 0x40]),
            0x7F => echo(b"^?"),
            _ => echo(&[byte]),
        }
    }

    /// Remove the last byte of the edited line and rub it out on the screen.
    fn erase_one(&mut self, echo: &mut dyn FnMut(&[u8])) {
        if self.line_len == 0 {
            return;
        }
        self.line_len -= 1;
        let byte = self.line[self.line_len];
        if self.termios.c_lflag & ECHO != 0 && self.termios.c_lflag & ECHOE != 0 {
            // A control character was echoed as two cells.
            if byte < 0x20 && byte != b'\t' || byte == 0x7F {
                echo(b"\x08 \x08");
            }
            echo(b"\x08 \x08");
        }
    }

    /// Process one received byte.
    ///
    /// `echo` receives the bytes to display.  The return value is a signal to
    /// send to the foreground process group, if the byte was an `ISIG`
    /// character.
    pub fn input(&mut self, byte: u8, echo: &mut dyn FnMut(&[u8])) -> Option<i32> {
        let iflag = self.termios.c_iflag;
        let lflag = self.termios.c_lflag;
        let cc = self.termios.c_cc;
        let mut byte = byte;
        if iflag & ISTRIP != 0 {
            byte &= 0x7F;
        }
        if byte == b'\r' {
            if iflag & IGNCR != 0 {
                return None;
            }
            if iflag & ICRNL != 0 {
                byte = b'\n';
            }
        } else if byte == b'\n' && iflag & INLCR != 0 {
            byte = b'\r';
        }

        if lflag & ISIG != 0 {
            let signal = if byte == cc[VINTR] {
                Some(SIGINT)
            } else if byte == cc[VQUIT] {
                Some(SIGQUIT)
            } else if byte == cc[VSUSP] {
                // No job control stop in v0: the character is consumed.
                return None;
            } else {
                None
            };
            if let Some(signal) = signal {
                if lflag & NOFLSH == 0 {
                    self.flush_input();
                }
                if lflag & ECHO != 0 {
                    echo(&[b'^', byte + 0x40]);
                    echo(b"\r\n");
                }
                return Some(signal);
            }
        }

        if lflag & ICANON == 0 {
            if self.push(byte) {
                self.echo_byte(byte, echo);
            }
            return None;
        }

        // A backspace key that sends BS is accepted beside the VERASE byte.
        if byte == cc[VERASE] || byte == 0x08 {
            self.erase_one(echo);
            return None;
        }
        if byte == cc[VKILL] {
            if lflag & ECHOK != 0 {
                while self.line_len != 0 {
                    self.erase_one(echo);
                }
            }
            self.line_len = 0;
            return None;
        }
        if byte == cc[VWERASE] && lflag & IEXTEN != 0 {
            while self.line_len != 0 && self.line[self.line_len - 1] == b' ' {
                self.erase_one(echo);
            }
            while self.line_len != 0 && self.line[self.line_len - 1] != b' ' {
                self.erase_one(echo);
            }
            return None;
        }
        if byte == cc[VEOF] {
            if !self.room_for_line() {
                return None;
            }
            // Mid-line, VEOF completes the line without a newline; at the
            // start of a line it queues the end-of-file mark.
            let length = self.line_len;
            for index in 0..length {
                let queued = self.line[index];
                self.push(queued);
            }
            self.push_record(length);
            self.line_len = 0;
            return None;
        }
        if byte == b'\n' || (cc[VEOL] != 0 && byte == cc[VEOL]) {
            if !self.room_for_line() {
                return None;
            }
            if lflag & ECHO != 0 || (byte == b'\n' && lflag & ECHONL != 0) {
                echo(b"\r\n");
            }
            let length = self.line_len;
            for index in 0..length {
                let queued = self.line[index];
                self.push(queued);
            }
            self.push(byte);
            self.push_record(length + 1);
            self.line_len = 0;
            return None;
        }
        // Leave room for the terminating newline.
        if self.line_len + 1 >= LINE_CAPACITY {
            if lflag & ECHO != 0 {
                echo(b"\x07");
            }
            return None;
        }
        self.line[self.line_len] = byte;
        self.line_len += 1;
        self.echo_byte(byte, echo);
        None
    }
}

static mut CONSOLE: Tty = Tty::new();

fn console() -> &'static mut Tty {
    // SAFETY: single-threaded kernel; the terminal is only touched from the
    // syscall path.
    unsafe { &mut *core::ptr::addr_of_mut!(CONSOLE) }
}

/// Return the console to its power-on state.
pub fn reset() {
    *console() = Tty::new();
}

/// Drain the keyboard and the serial receiver through the line discipline.
///
/// A signal character is delivered to the foreground process group here, so a
/// program that never reads the console still receives `SIGINT`.
pub fn pump() {
    let mut echoed = false;
    while let Some(byte) = crate::kbd::read_input_byte() {
        let signal = console().input(byte, &mut |bytes| {
            crate::write_bytes(bytes);
            echoed = true;
        });
        if let Some(signal) = signal {
            let group = console().foreground();
            if group != 0 {
                crate::process::signal_group(group, signal);
            }
        }
    }
    if echoed {
        crate::graphics_console::flush();
    }
}

pub fn readable() -> bool {
    pump();
    console().readable()
}

pub fn read(out: &mut [u8]) -> Option<usize> {
    pump();
    console().read(out)
}

pub fn termios() -> termios {
    console().termios()
}

pub fn set_termios(value: termios, flush: bool) {
    console().set_termios(value, flush);
}

pub fn foreground() -> u32 {
    console().foreground()
}

pub fn set_foreground(group: u32) {
    console().set_foreground(group);
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;
    use std::vec::Vec;

    fn feed(tty: &mut Tty, bytes: &[u8]) -> (Vec<u8>, Vec<i32>) {
        let mut echoed = Vec::new();
        let mut signals = Vec::new();
        for &byte in bytes {
            if let Some(signal) = tty.input(byte, &mut |out| echoed.extend_from_slice(out)) {
                signals.push(signal);
            }
        }
        (echoed, signals)
    }

    #[test]
    fn a_line_is_readable_only_when_complete() {
        let mut tty = Tty::new();
        let (echoed, _) = feed(&mut tty, b"ls");
        assert_eq!(echoed, b"ls");
        assert!(!tty.readable());
        let (echoed, _) = feed(&mut tty, b"\r");
        assert_eq!(echoed, b"\r\n");
        let mut out = [0u8; 16];
        assert_eq!(tty.read(&mut out), Some(3));
        assert_eq!(&out[..3], b"ls\n");
        assert!(!tty.readable());
    }

    #[test]
    fn erase_and_kill_edit_the_line() {
        let mut tty = Tty::new();
        let (echoed, _) = feed(&mut tty, b"lx\x7fs\n");
        assert_eq!(echoed, b"lx\x08 \x08s\r\n");
        let mut out = [0u8; 16];
        assert_eq!(tty.read(&mut out), Some(3));
        assert_eq!(&out[..3], b"ls\n");
        feed(&mut tty, b"junk\x15pwd\n");
        assert_eq!(tty.read(&mut out), Some(4));
        assert_eq!(&out[..4], b"pwd\n");
    }

    #[test]
    fn one_read_returns_one_line_and_a_short_buffer_splits_it() {
        let mut tty = Tty::new();
        feed(&mut tty, b"ab\ncd\n");
        let mut out = [0u8; 2];
        assert_eq!(tty.read(&mut out), Some(2));
        assert_eq!(&out, b"ab");
        assert_eq!(tty.read(&mut out), Some(1));
        assert_eq!(out[0], b'\n');
        let mut out = [0u8; 16];
        assert_eq!(tty.read(&mut out), Some(3));
        assert_eq!(&out[..3], b"cd\n");
    }

    #[test]
    fn eof_at_line_start_is_a_zero_length_read() {
        let mut tty = Tty::new();
        feed(&mut tty, b"\x04");
        let mut out = [0u8; 4];
        assert_eq!(tty.read(&mut out), Some(0));
        assert_eq!(tty.read(&mut out), None);
        feed(&mut tty, b"ab\x04");
        assert_eq!(tty.read(&mut out), Some(2));
    }

    #[test]
    fn interrupt_flushes_the_line_and_reports_sigint() {
        let mut tty = Tty::new();
        let (echoed, signals) = feed(&mut tty, b"sleep\x03");
        assert_eq!(signals, [SIGINT]);
        assert!(echoed.ends_with(b"^C\r\n"));
        feed(&mut tty, b"\n");
        let mut out = [0u8; 8];
        assert_eq!(tty.read(&mut out), Some(1));
        let (_, signals) = feed(&mut tty, b"\x1c");
        assert_eq!(signals, [SIGQUIT]);
    }

    #[test]
    fn raw_mode_returns_bytes_without_echo_or_signals() {
        let mut tty = Tty::new();
        let mut raw = tty.termios();
        raw.c_lflag &= !(ICANON | ECHO | ISIG);
        raw.c_iflag &= !ICRNL;
        tty.set_termios(raw, false);
        let (echoed, signals) = feed(&mut tty, b"a\x03\r");
        assert!(echoed.is_empty());
        assert!(signals.is_empty());
        let mut out = [0u8; 8];
        assert_eq!(tty.read(&mut out), Some(3));
        assert_eq!(&out[..3], b"a\x03\r");
    }

    #[test]
    fn a_partial_line_survives_the_switch_to_raw_mode() {
        let mut tty = Tty::new();
        feed(&mut tty, b"ab");
        let mut raw = tty.termios();
        raw.c_lflag &= !ICANON;
        tty.set_termios(raw, false);
        let mut out = [0u8; 8];
        assert_eq!(tty.read(&mut out), Some(2));
        let cooked = default_termios();
        feed(&mut tty, b"cd");
        tty.set_termios(cooked, false);
        assert_eq!(tty.read(&mut out), Some(2));
        assert_eq!(&out[..2], b"cd");
    }

    #[test]
    fn a_full_line_rings_the_bell_and_still_accepts_newline() {
        let mut tty = Tty::new();
        let filler = [b'x'; LINE_CAPACITY + 8];
        let (echoed, _) = feed(&mut tty, &filler);
        assert!(echoed.ends_with(b"\x07"));
        feed(&mut tty, b"\n");
        let mut out = [0u8; LINE_CAPACITY + 8];
        assert_eq!(tty.read(&mut out), Some(LINE_CAPACITY));
        assert_eq!(out[LINE_CAPACITY - 1], b'\n');
    }

    #[test]
    fn termios_is_sixty_bytes() {
        assert_eq!(core::mem::size_of::<termios>(), 60);
    }
}
