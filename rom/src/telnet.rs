//! Polled ROM console over telnet, styled with a small no_std Lip Gloss subset.
//!
//! iris forwards host `127.0.0.1:2323` to guest port 23 by default.  The
//! console is a single listening smoltcp TCP socket: when a client connects
//! it becomes the session, and when it closes the socket is re-armed for the
//! next client.  There are no threads; `service()` is called from the ROM's
//! network wait loops and, after the hand-off, from the kernel idle path.

use crate::gloss::{self, Color, ColorMode, Style};
use crate::net::{clock_now, clock_tick, SeeqDevice};
use alloc::boxed::Box;
use alloc::vec;
use core::fmt::{self, Write};
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::{Ipv4Address, Ipv4Cidr};

const TELNET_PORT: u16 = 23;
const HISTORY_LEN: usize = 8;
const LINE_LEN: usize = 128;

const BORDER: Style = Style::new().fg(Color::Rgb(125, 86, 244));
const TITLE: Style = Style::new().bold().fg(Color::Rgb(242, 93, 148));
const LABEL: Style = Style::new().fg(Color::Rgb(148, 163, 184));
const VALUE: Style = Style::new().fg(Color::Rgb(248, 250, 252));
const ACCENT: Style = Style::new().bold().fg(Color::Rgb(94, 234, 212));
const WARN: Style = Style::new().fg(Color::Rgb(251, 191, 36));
const MUTED: Style = Style::new().fg(Color::Rgb(100, 116, 139));
const GOOD: Style = Style::new().fg(Color::Rgb(74, 222, 128));

const COMMANDS: &[&str] = &[
    "help", "status", "net", "mem", "time", "date", "uptime", "sysinfo", "log", "history",
    "banner", "clear", "color", "echo", "boot", "exit",
];

/// Live information shown by the console's `status`/`net` commands.
pub struct ConsoleInfo {
    pub stage: &'static str,
    pub ip: Option<Ipv4Cidr>,
    pub gateway: Option<Ipv4Address>,
    pub mac: [u8; 6],
    pub ram_size: usize,
    pub reserved_bytes: usize,
    pub ntp_time: Option<u64>,
    pub board: &'static str,
    pub cpu: &'static str,
    pub firmware: &'static str,
    pub dhcp: bool,
}

pub struct TelnetConsole {
    handle: smoltcp::iface::SocketHandle,
    line: [u8; LINE_LEN],
    line_len: usize,
    cursor: usize,
    iac: u8,
    sb: [u8; 5],
    sb_len: usize,
    sb_naws: bool,
    esc: u8,
    last_cr: bool,
    connected: bool,
    boot_requested: bool,
    width: u16,
    height: u16,
    color: ColorMode,
    history: [[u8; LINE_LEN]; HISTORY_LEN],
    history_lens: [u8; HISTORY_LEN],
    history_len: usize,
    history_pos: usize,
    draft: [u8; LINE_LEN],
    draft_len: usize,
}

impl TelnetConsole {
    /// Add the listening socket to `socket_set` and bind guest port 23.
    pub fn new(socket_set: &mut SocketSet) -> Option<Self> {
        let rx = Box::leak(Box::new(vec![0u8; 2048]));
        let tx = Box::leak(Box::new(vec![0u8; 16384]));
        let socket = TcpSocket::new(
            SocketBuffer::new(&mut rx[..]),
            SocketBuffer::new(&mut tx[..]),
        );
        let handle = socket_set.add(socket);
        socket_set
            .get_mut::<TcpSocket>(handle)
            .listen(TELNET_PORT)
            .ok()?;
        Some(Self {
            handle,
            line: [0; LINE_LEN],
            line_len: 0,
            cursor: 0,
            iac: 0,
            sb: [0; 5],
            sb_len: 0,
            sb_naws: false,
            esc: 0,
            last_cr: false,
            connected: false,
            boot_requested: false,
            width: 80,
            height: 24,
            color: ColorMode::TrueColor,
            history: [[0; LINE_LEN]; HISTORY_LEN],
            history_lens: [0; HISTORY_LEN],
            history_len: 0,
            history_pos: 0,
            draft: [0; LINE_LEN],
            draft_len: 0,
        })
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn boot_requested(&self) -> bool {
        self.boot_requested
    }

    /// Service the listener/session.  Call after `iface.poll`.
    pub fn service(&mut self, socket_set: &mut SocketSet, info: &ConsoleInfo) {
        let sock = socket_set.get_mut::<TcpSocket>(self.handle);

        // Once the peer has gone, re-arm the listener for the next client.
        if matches!(
            sock.state(),
            State::Closed | State::TimeWait | State::LastAck
        ) {
            sock.abort();
            let _ = sock.listen(TELNET_PORT);
            self.connected = false;
        }

        let now_connected = sock.remote_endpoint().is_some()
            && matches!(
                sock.state(),
                State::Established | State::FinWait1 | State::FinWait2 | State::CloseWait
            );
        if now_connected && !self.connected {
            self.connected = true;
            self.line_len = 0;
            self.cursor = 0;
            self.iac = 0;
            self.sb_len = 0;
            self.esc = 0;
            self.last_cr = false;
            self.history_pos = self.history_len;
            // IAC WILL ECHO, IAC WILL SUPPRESS-GO-AHEAD, IAC DO NAWS.
            let _ = sock.send_slice(&[0xFF, 0xFB, 0x01, 0xFF, 0xFB, 0x03, 0xFF, 0xFD, 0x1F]);
            self.welcome(sock, info);
            self.prompt(sock);
        } else if !now_connected {
            self.connected = false;
        }

        let mut buf = [0u8; 64];
        while sock.can_recv() {
            let n = match sock.recv_slice(&mut buf) {
                Ok(n) => n,
                Err(_) => break,
            };
            for &b in &buf[..n] {
                self.input(sock, b, info);
            }
        }

        // The peer sent FIN.  Close our side so the next poll reaches Closed
        // and the socket can listen again; otherwise the console would only
        // ever serve its first client.
        if sock.state() == State::CloseWait {
            sock.close();
            self.connected = false;
        }
    }

    /// Wait after DHCP for a client to attach.  Returns true if the client
    /// asked to continue booting.  If nobody connects, the boot continues
    /// after roughly `window_counts` CP0 Count ticks.
    pub fn wait_for_console(
        &mut self,
        iface: &mut Interface,
        socket_set: &mut SocketSet,
        device: &mut SeeqDevice,
        info: &ConsoleInfo,
        window_counts: u32,
    ) -> bool {
        let start = read_count();
        let mut iterations = 0u32;
        loop {
            let timestamp = clock_now();
            let _ = iface.poll(timestamp, device, socket_set);
            self.service(socket_set, info);
            clock_tick();

            if self.boot_requested {
                return true;
            }
            if self.connected {
                // Keep the session alive until the client says `boot` or
                // disconnects; do not auto-continue under their cursor.
                iterations = 0;
                core::hint::spin_loop();
                continue;
            }

            iterations = iterations.saturating_add(1);
            if read_count().wrapping_sub(start) >= window_counts || iterations > 5_000_000 {
                return false;
            }
            core::hint::spin_loop();
        }
    }

    fn input(&mut self, sock: &mut TcpSocket, b: u8, info: &ConsoleInfo) {
        if self.iac != 0 {
            self.iac_byte(b);
            return;
        }
        if b == 0xFF {
            self.iac = 1;
            return;
        }
        if self.esc != 0 {
            self.escape_byte(sock, b);
            return;
        }
        if b == 0x1B {
            self.esc = 1;
            return;
        }

        match b {
            b'\r' => {
                self.last_cr = true;
                self.send(sock, "\r\n");
                self.execute(sock, info);
            }
            b'\n' => {
                if self.last_cr {
                    self.last_cr = false;
                    return;
                }
                self.send(sock, "\r\n");
                self.execute(sock, info);
            }
            0x03 => {
                // Ctrl-C: discard the line.
                self.line_len = 0;
                self.cursor = 0;
                self.history_pos = self.history_len;
                self.send(sock, "^C\r\n");
                self.prompt(sock);
            }
            0x0C => {
                // Ctrl-L: clear the screen and redraw the current line.
                self.send(sock, "\x1b[2J\x1b[H");
                self.redraw(sock);
            }
            0x15 => {
                // Ctrl-U: kill the line.
                self.line_len = 0;
                self.cursor = 0;
                self.redraw(sock);
            }
            0x08 | 0x7F => {
                self.last_cr = false;
                if self.cursor > 0 {
                    if self.cursor < self.line_len {
                        self.line
                            .copy_within(self.cursor..self.line_len, self.cursor - 1);
                    }
                    self.line_len -= 1;
                    self.cursor -= 1;
                    if self.cursor == self.line_len {
                        self.send(sock, "\x08 \x08");
                    } else {
                        self.redraw_at_cursor(sock);
                    }
                }
            }
            b'\t' => self.complete(sock),
            0x01 => {
                // Ctrl-A: start of line.
                self.cursor = 0;
                self.redraw_at_cursor(sock);
            }
            0x05 => {
                // Ctrl-E: end of line.
                self.cursor = self.line_len;
                self.redraw_at_cursor(sock);
            }
            0x20..=0x7E => {
                self.last_cr = false;
                if self.line_len < self.line.len() {
                    if self.cursor < self.line_len {
                        self.line
                            .copy_within(self.cursor..self.line_len, self.cursor + 1);
                    }
                    self.line[self.cursor] = b;
                    self.line_len += 1;
                    self.cursor += 1;
                    if self.cursor == self.line_len {
                        self.send_bytes(sock, &[b]);
                    } else {
                        self.redraw_at_cursor(sock);
                    }
                }
            }
            _ => {}
        }
    }

    fn iac_byte(&mut self, b: u8) {
        match self.iac {
            1 => match b {
                0xFA => {
                    self.iac = 3;
                    self.sb_len = 0;
                    self.sb_naws = false;
                }
                0xFB..=0xFE => self.iac = 2,
                _ => self.iac = 0,
            },
            2 => self.iac = 0,
            3 => {
                if b == 0xFF {
                    self.iac = 4;
                    return;
                }
                if self.sb_len == 0 {
                    self.sb_naws = b == 31;
                    self.sb[0] = b;
                    self.sb_len = 1;
                } else if self.sb_naws && self.sb_len < self.sb.len() {
                    self.sb[self.sb_len] = b;
                    self.sb_len += 1;
                }
            }
            4 => {
                if b == 0xF0 {
                    self.apply_naws();
                    self.iac = 0;
                } else {
                    self.iac = 3;
                }
            }
            _ => self.iac = 0,
        }
    }

    fn apply_naws(&mut self) {
        if self.sb_naws && self.sb_len >= 5 {
            let width = u16::from_be_bytes([self.sb[1], self.sb[2]]);
            let height = u16::from_be_bytes([self.sb[3], self.sb[4]]);
            if width >= 40 {
                self.width = width.min(180);
            }
            if height >= 10 {
                self.height = height.min(80);
            }
        }
    }

    fn escape_byte(&mut self, sock: &mut TcpSocket, b: u8) {
        if self.esc == 1 {
            self.esc = if b == b'[' { 2 } else { 0 };
            return;
        }
        if self.esc == 3 {
            if b == b'~' && self.cursor < self.line_len {
                self.line
                    .copy_within(self.cursor + 1..self.line_len, self.cursor);
                self.line_len -= 1;
                self.redraw_at_cursor(sock);
            }
            self.esc = 0;
            return;
        }
        match b {
            b'A' => self.history_up(sock),
            b'B' => self.history_down(sock),
            b'C' => {
                if self.cursor < self.line_len {
                    self.cursor += 1;
                    self.send(sock, "\x1b[C");
                }
            }
            b'D' => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                    self.send(sock, "\x1b[D");
                }
            }
            b'H' => {
                self.cursor = 0;
                self.redraw_at_cursor(sock);
            }
            b'F' => {
                self.cursor = self.line_len;
                self.redraw_at_cursor(sock);
            }
            b'3' => {
                self.esc = 3;
                return;
            }
            _ => {}
        }
        self.esc = 0;
    }

    fn execute(&mut self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        let mut line = [0u8; LINE_LEN];
        let n = self.line_len;
        line[..n].copy_from_slice(&self.line[..n]);
        let line = core::str::from_utf8(&line[..n]).unwrap_or("");
        if !line.is_empty() {
            self.remember(line);
        }
        self.line_len = 0;
        self.cursor = 0;
        self.history_pos = self.history_len;

        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("");
        let mut show_prompt = true;

        match cmd {
            "" => {}
            "help" => self.help(sock),
            "status" => self.status(sock, info),
            "net" => self.net(sock, info),
            "mem" => self.mem(sock, info),
            "time" | "date" => self.time(sock, info, cmd == "date"),
            "uptime" => self.uptime(sock),
            "sysinfo" => self.sysinfo(sock, info),
            "log" => self.log(sock),
            "history" => self.show_history(sock),
            "banner" => {
                self.send(sock, "\x1b[2J\x1b[H");
                self.welcome(sock, info);
            }
            "clear" => self.send(sock, "\x1b[2J\x1b[H"),
            "color" => self.color(sock, arg),
            "echo" => {
                let rest = line.strip_prefix("echo").unwrap_or("").trim_start();
                self.send(sock, rest);
                self.send(sock, "\r\n");
            }
            "boot" => {
                self.boot_requested = true;
                self.send(sock, "continuing boot\r\n");
            }
            "exit" => {
                sock.close();
                self.connected = false;
                show_prompt = false;
            }
            _ => {
                self.styled(sock, WARN, "unknown command: ", self.color);
                self.send(sock, cmd);
                self.send(sock, "\r\n");
                self.send(sock, "try 'help'\r\n");
            }
        }

        if show_prompt {
            self.prompt(sock);
        }
    }

    fn welcome(&self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        let logo = [
            " ██╗██████╗ ██╗██╗  ██╗     ███████╗",
            " ██║██╔══██╗██║╚██╗██╔╝     ╚══███╔╝",
            " ██║██████╔╝██║ ╚███╔╝        ███╔╝ ",
            " ██║██╔══██╗██║ ██╔██╗       ███╔╝  ",
            " ██║██║  ██║██║██╔╝ ██╗     ███████╗",
            " ╚═╝╚═╝  ╚═╝╚═╝╚═╝  ╚═╝     ╚══════╝",
        ];
        for (i, line) in logo.iter().enumerate() {
            let start = if i < 3 {
                (125, 86, 244)
            } else {
                (242, 93, 148)
            };
            let end = if i < 3 {
                (242, 93, 148)
            } else {
                (251, 191, 36)
            };
            self.gradient(sock, line, start, end);
            self.send(sock, "\r\n");
        }
        self.styled(
            sock,
            MUTED,
            " Indy IP22  ·  n32/MIPS III  ·  ROM console",
            self.color,
        );
        self.send(sock, "\r\n\r\n");
        self.status(sock, info);
        self.send(sock, "\r\n");
        self.styled(
            sock,
            MUTED,
            " help for commands  ·  ↑/↓ history  ·  Tab complete  ·  Ctrl-L clear",
            self.color,
        );
        self.send(sock, "\r\n");
    }

    fn status(&self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        self.box_top(sock, "SYSTEM");
        self.field(sock, "stage", info.stage);
        self.field(sock, "board", info.board);
        self.field(sock, "cpu", info.cpu);
        self.field(sock, "firmware", info.firmware);
        let ip = ip_string(info.ip);
        self.field(sock, "ip", ip.as_str());
        let mac = mac_string(&info.mac);
        self.field(sock, "mac", mac.as_str());
        let mut v = FmtBuf::new();
        let _ = write!(
            v,
            "{} MiB total / {} MiB usable",
            info.ram_size / (1024 * 1024),
            info.ram_size.saturating_sub(info.reserved_bytes) / (1024 * 1024)
        );
        self.field(sock, "ram", v.as_str());
        match info.ntp_time {
            Some(t) => {
                let d = utc_string(t);
                self.field(sock, "ntp", d.as_str());
            }
            None => self.field(sock, "ntp", "not set"),
        }
        let up = uptime_string();
        self.field(sock, "uptime", up.as_str());
        let mut term = FmtBuf::new();
        let _ = write!(term, "{}x{}", self.width, self.height);
        self.field(sock, "term", term.as_str());
        self.box_bottom(sock);
    }

    fn net(&self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        self.box_top(sock, "NETWORK");
        let ip = ip_string(info.ip);
        self.field(sock, "ip", ip.as_str());
        let gw = match info.gateway {
            Some(ip) => {
                let o = ip.octets();
                let mut b = FmtBuf::new();
                let _ = write!(b, "{}.{}.{}.{}", o[0], o[1], o[2], o[3]);
                b
            }
            None => {
                let mut b = FmtBuf::new();
                let _ = b.write_str("none");
                b
            }
        };
        self.field(sock, "gateway", gw.as_str());
        let mac = mac_string(&info.mac);
        self.field(sock, "mac", mac.as_str());
        self.field(sock, "dhcp", if info.dhcp { "yes" } else { "no" });
        self.field(sock, "driver", "Seeq 8003 · HPC3 PDMA");
        self.field(sock, "console", "telnet :23");
        self.box_bottom(sock);
    }

    fn mem(&self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        self.box_top(sock, "MEMORY");
        let mut total = FmtBuf::new();
        let _ = write!(total, "{} MiB", info.ram_size / (1024 * 1024));
        self.field(sock, "tested", total.as_str());
        let mut reserved = FmtBuf::new();
        let _ = write!(reserved, "{} MiB", info.reserved_bytes / (1024 * 1024));
        self.field(sock, "reserved", reserved.as_str());
        let mut usable = FmtBuf::new();
        let _ = write!(
            usable,
            "{} MiB",
            info.ram_size.saturating_sub(info.reserved_bytes) / (1024 * 1024)
        );
        self.field(sock, "usable", usable.as_str());
        self.field(sock, "test", "full pass, no errors");
        self.field(sock, "heap", "2 MiB bump allocator");
        self.field(sock, "layout", "kernel 4 MiB · console 7–11 MiB");
        self.box_bottom(sock);
    }

    fn time(&self, sock: &mut TcpSocket, info: &ConsoleInfo, date_only: bool) {
        match info.ntp_time {
            Some(t) => {
                let d = utc_string(t);
                self.box_top(sock, "TIME");
                self.field(sock, "utc", d.as_str());
                let mut u = FmtBuf::new();
                let _ = write!(u, "{}", t);
                self.field(sock, "unix", u.as_str());
                self.field(sock, "source", "NTP");
                if !date_only {
                    let up = uptime_string();
                    self.field(sock, "rom clock", up.as_str());
                }
                self.box_bottom(sock);
            }
            None => {
                self.styled(sock, WARN, "NTP time is not set", self.color);
                self.send(sock, "\r\n");
            }
        }
    }

    fn uptime(&self, sock: &mut TcpSocket) {
        let up = uptime_string();
        self.styled(sock, LABEL, "rom clock  ", self.color);
        self.styled(sock, ACCENT, up.as_str(), self.color);
        self.send(sock, "\r\n");
    }

    fn sysinfo(&self, sock: &mut TcpSocket, info: &ConsoleInfo) {
        self.box_top(sock, "SYSINFO");
        self.field(sock, "board", info.board);
        self.field(sock, "firmware", info.firmware);
        self.field(sock, "cpu", info.cpu);
        let prid = cp0_prid();
        let mut b = FmtBuf::new();
        let _ = write!(b, "0x{:08X}", prid);
        self.field(sock, "PRId", b.as_str());
        let mut b = FmtBuf::new();
        let _ = write!(b, "{}", (prid >> 8) & 0xFF);
        self.field(sock, "implementation", b.as_str());
        let mut b = FmtBuf::new();
        let _ = write!(b, "{}", prid & 0xFF);
        self.field(sock, "revision", b.as_str());
        let mut b = FmtBuf::new();
        let _ = write!(b, "0x{:08X}", cp0_status());
        self.field(sock, "status", b.as_str());
        let mut b = FmtBuf::new();
        let _ = write!(b, "0x{:08X}", cp0_config());
        self.field(sock, "config", b.as_str());
        let mut b = FmtBuf::new();
        let _ = write!(b, "0x{:08X}", cp0_count());
        self.field(sock, "count", b.as_str());
        self.box_bottom(sock);
    }

    fn help(&self, sock: &mut TcpSocket) {
        self.box_top(sock, "COMMANDS");
        self.field(sock, "help", "this page");
        self.field(sock, "status", "system dashboard");
        self.field(sock, "net", "network settings");
        self.field(sock, "mem", "RAM map and test");
        self.field(sock, "time", "NTP time (UTC + unix)");
        self.field(sock, "date", "UTC date and time");
        self.field(sock, "uptime", "ROM clock");
        self.field(sock, "sysinfo", "CPU and CP0 registers");
        self.field(sock, "log", "replay the boot log");
        self.field(sock, "history", "recent commands");
        self.field(sock, "banner", "redraw the dashboard");
        self.field(sock, "clear", "clear the screen");
        self.field(sock, "color", "true | 256 | 16 | off");
        self.field(sock, "echo", "print text");
        self.field(sock, "boot", "continue the boot");
        self.field(sock, "exit", "close this session");
        self.box_rule(sock);
        self.field(sock, "↑ / ↓", "command history");
        self.field(sock, "Tab", "complete a command");
        self.field(sock, "Ctrl-L", "clear the screen");
        self.field(sock, "Ctrl-C", "cancel the current line");
        self.box_bottom(sock);
    }

    fn log(&self, sock: &mut TcpSocket) {
        self.styled(sock, TITLE, "boot log", self.color);
        self.send(sock, "\r\n");
        self.styled(
            sock,
            MUTED,
            "────────────────────────────────────────",
            self.color,
        );
        self.send(sock, "\r\n");
        let mut out = FmtBuf::new();
        let _ = MUTED.open(self.color, &mut out);
        self.send_bytes(sock, out.as_bytes());
        self.send_bytes(sock, crate::boot_log());
        if self.color != ColorMode::None {
            self.send(sock, "\x1b[0m");
        }
        self.send(sock, "\r\n");
    }

    fn show_history(&self, sock: &mut TcpSocket) {
        self.box_top(sock, "HISTORY");
        if self.history_len == 0 {
            self.field(sock, "-", "empty");
        } else {
            for i in 0..self.history_len {
                let mut label = FmtBuf::new();
                let _ = write!(label, "{}", i + 1);
                let text = core::str::from_utf8(&self.history[i][..self.history_lens[i] as usize])
                    .unwrap_or("");
                self.field(sock, label.as_str(), text);
            }
        }
        self.box_bottom(sock);
    }

    fn color(&mut self, sock: &mut TcpSocket, arg: &str) {
        if arg.is_empty() {
            self.styled(sock, LABEL, "colour mode: ", self.color);
            self.styled(sock, ACCENT, self.color.label(), self.color);
            self.send(sock, "\r\n");
            return;
        }
        match ColorMode::parse(arg) {
            Some(mode) => {
                self.color = mode;
                self.styled(sock, GOOD, "colour mode set to ", mode);
                self.send(sock, mode.label());
                self.send(sock, "\r\n");
            }
            None => {
                self.styled(sock, WARN, "use true, 256, 16 or off", self.color);
                self.send(sock, "\r\n");
            }
        }
    }

    fn box_top(&self, sock: &mut TcpSocket, title: &str) {
        let mut out = FmtBuf::new();
        let _ = gloss::box_top(
            &mut out,
            title,
            self.card_width(),
            BORDER,
            TITLE,
            self.color,
        );
        self.send_bytes(sock, out.as_bytes());
    }

    fn box_bottom(&self, sock: &mut TcpSocket) {
        let mut out = FmtBuf::new();
        let _ = gloss::box_bottom(&mut out, self.card_width(), BORDER, self.color);
        self.send_bytes(sock, out.as_bytes());
    }

    fn box_rule(&self, sock: &mut TcpSocket) {
        let mut out = FmtBuf::new();
        let _ = gloss::box_rule(&mut out, self.card_width(), BORDER, self.color);
        self.send_bytes(sock, out.as_bytes());
    }

    fn field(&self, sock: &mut TcpSocket, label: &str, value: &str) {
        let width = self.card_width();
        let inner = width.saturating_sub(4);
        let label_width = 14;
        let value_width = inner.saturating_sub(label_width + 2);
        self.styled(sock, BORDER, "│ ", self.color);
        self.styled_padded(sock, LABEL, label, label_width);
        self.send(sock, "  ");
        self.styled_padded(sock, VALUE, value, value_width);
        self.styled(sock, BORDER, " │", self.color);
        self.send(sock, "\r\n");
    }

    fn prompt(&self, sock: &mut TcpSocket) {
        self.styled(sock, ACCENT, "rom", self.color);
        self.send(sock, " ");
        self.styled(sock, TITLE, "❯", self.color);
        self.send(sock, " ");
    }

    fn redraw(&self, sock: &mut TcpSocket) {
        self.send(sock, "\r\x1b[K");
        self.prompt(sock);
        self.send_bytes(sock, &self.line[..self.line_len]);
    }

    fn redraw_at_cursor(&self, sock: &mut TcpSocket) {
        self.redraw(sock);
        let back = self.line_len.saturating_sub(self.cursor);
        if back > 0 {
            let mut out = FmtBuf::new();
            let _ = write!(out, "\x1b[{}D", back);
            self.send_bytes(sock, out.as_bytes());
        }
    }

    fn remember(&mut self, line: &str) {
        let bytes = line.as_bytes();
        if bytes.is_empty() {
            return;
        }
        if self.history_len > 0 {
            let last = self.history_len - 1;
            let len = self.history_lens[last] as usize;
            if &self.history[last][..len] == bytes {
                return;
            }
        }
        if self.history_len == HISTORY_LEN {
            for i in 1..HISTORY_LEN {
                self.history[i - 1] = self.history[i];
                self.history_lens[i - 1] = self.history_lens[i];
            }
            self.history_len -= 1;
        }
        let idx = self.history_len;
        let len = bytes.len().min(LINE_LEN);
        self.history[idx][..len].copy_from_slice(&bytes[..len]);
        self.history_lens[idx] = len as u8;
        self.history_len += 1;
        self.history_pos = self.history_len;
    }

    fn history_up(&mut self, sock: &mut TcpSocket) {
        if self.history_len == 0 {
            return;
        }
        if self.history_pos == self.history_len {
            self.draft_len = self.line_len;
            self.draft[..self.line_len].copy_from_slice(&self.line[..self.line_len]);
        }
        if self.history_pos > 0 {
            self.history_pos -= 1;
            let len = self.history_lens[self.history_pos] as usize;
            self.line[..len].copy_from_slice(&self.history[self.history_pos][..len]);
            self.line_len = len;
            self.cursor = len;
            self.redraw(sock);
        }
    }

    fn history_down(&mut self, sock: &mut TcpSocket) {
        if self.history_pos < self.history_len {
            self.history_pos += 1;
            if self.history_pos == self.history_len {
                let len = self.draft_len;
                self.line[..len].copy_from_slice(&self.draft[..len]);
                self.line_len = len;
                self.cursor = len;
            } else {
                let len = self.history_lens[self.history_pos] as usize;
                self.line[..len].copy_from_slice(&self.history[self.history_pos][..len]);
                self.line_len = len;
                self.cursor = len;
            }
            self.redraw(sock);
        }
    }

    fn complete(&mut self, sock: &mut TcpSocket) {
        let line = core::str::from_utf8(&self.line[..self.line_len]).unwrap_or("");
        if line.contains(' ') {
            return;
        }
        let mut one: Option<&str> = None;
        let mut matches = 0usize;
        for &cmd in COMMANDS {
            if cmd.starts_with(line) {
                one = Some(cmd);
                matches += 1;
            }
        }
        if matches == 1 {
            if let Some(cmd) = one {
                self.line_len = cmd.len().min(LINE_LEN);
                self.line[..self.line_len].copy_from_slice(&cmd.as_bytes()[..self.line_len]);
                self.cursor = self.line_len;
                self.redraw(sock);
            }
        } else if matches > 1 {
            self.send(sock, "\r\n");
            for &cmd in COMMANDS {
                if cmd.starts_with(line) {
                    self.styled(sock, ACCENT, cmd, self.color);
                    self.send(sock, "  ");
                }
            }
            self.send(sock, "\r\n");
            self.redraw(sock);
        }
    }

    fn card_width(&self) -> usize {
        (self.width as usize).saturating_sub(2).clamp(40, 76)
    }

    fn styled(&self, sock: &mut TcpSocket, style: Style, text: &str, mode: ColorMode) {
        let mut out = FmtBuf::new();
        let _ = style.render(text, mode, &mut out);
        self.send_bytes(sock, out.as_bytes());
    }

    fn styled_padded(&self, sock: &mut TcpSocket, style: Style, text: &str, width: usize) {
        let mut out = FmtBuf::new();
        let _ = style.open(self.color, &mut out);
        let _ = gloss::pad(&mut out, text, width);
        let _ = style.close(self.color, &mut out);
        self.send_bytes(sock, out.as_bytes());
    }

    fn gradient(&self, sock: &mut TcpSocket, text: &str, start: (u8, u8, u8), end: (u8, u8, u8)) {
        let mut out = FmtBuf::new();
        let _ = gloss::gradient(text, start, end, self.color, &mut out);
        self.send_bytes(sock, out.as_bytes());
    }

    fn send(&self, sock: &mut TcpSocket, text: &str) {
        self.send_bytes(sock, text.as_bytes());
    }

    fn send_bytes(&self, sock: &mut TcpSocket, bytes: &[u8]) {
        for chunk in bytes.chunks(512) {
            let _ = sock.send_slice(chunk);
        }
    }
}

fn ip_string(ip: Option<Ipv4Cidr>) -> FmtBuf {
    let mut b = FmtBuf::new();
    match ip {
        Some(ip) => {
            let o = ip.address().octets();
            let _ = write!(b, "{}.{}.{}.{}/{}", o[0], o[1], o[2], o[3], ip.prefix_len());
        }
        None => {
            let _ = b.write_str("none");
        }
    }
    b
}

fn mac_string(mac: &[u8; 6]) -> FmtBuf {
    let mut b = FmtBuf::new();
    let _ = write!(
        b,
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );
    b
}

fn utc_string(secs: u64) -> FmtBuf {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let mut b = FmtBuf::new();
    let _ = write!(
        b,
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        year,
        month,
        day,
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    );
    b
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    (y, m as u32, d as u32)
}

fn uptime_string() -> FmtBuf {
    let secs = (clock_now().total_millis().max(0) as u64) / 1000;
    let mut b = FmtBuf::new();
    let _ = write!(
        b,
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    );
    b
}

/// Fixed-size formatter so console commands never allocate from the ROM heap.
struct FmtBuf {
    data: [u8; 2048],
    len: usize,
}

impl FmtBuf {
    fn new() -> Self {
        Self {
            data: [0; 2048],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len]
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

impl Write for FmtBuf {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        if self.len + bytes.len() > self.data.len() {
            return Err(fmt::Error);
        }
        self.data[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        Ok(())
    }
}

#[cfg(target_arch = "mips64")]
fn cp0_read(reg: u32) -> u32 {
    let v: u32;
    unsafe {
        match reg {
            9 => core::arch::asm!(".set noat", "mfc0 {0}, $9", out(reg) v),
            12 => core::arch::asm!(".set noat", "mfc0 {0}, $12", out(reg) v),
            15 => core::arch::asm!(".set noat", "mfc0 {0}, $15", out(reg) v),
            16 => core::arch::asm!(".set noat", "mfc0 {0}, $16", out(reg) v),
            _ => v = 0,
        }
    }
    v
}

#[cfg(not(target_arch = "mips64"))]
fn cp0_read(_reg: u32) -> u32 {
    0
}

fn cp0_prid() -> u32 {
    cp0_read(15)
}

fn cp0_status() -> u32 {
    cp0_read(12)
}

fn cp0_config() -> u32 {
    cp0_read(16)
}

fn cp0_count() -> u32 {
    cp0_read(9)
}

#[cfg(target_arch = "mips64")]
fn read_count() -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!(".set noat", "mfc0 {0}, $9", out(reg) v);
    }
    v
}

#[cfg(not(target_arch = "mips64"))]
fn read_count() -> u32 {
    0
}
