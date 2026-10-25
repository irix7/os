// SPDX-License-Identifier: GPL-3.0-only
//! Telnet console server for the kernel.
//!
//! One listening TCP socket on guest port 23.  The console mirrors the
//! serial log (through [`write_output`]) and accepts a small command set so
//! the machine can be inspected over the network after the ROM handoff.
//!
//! The socket lives in the shared smoltcp `SocketSet` owned by [`crate::net`];
//! this module only services it.  It must therefore be initialised after
//! `net::init()`.

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use smoltcp::iface::SocketSet;
use smoltcp::socket::tcp::{Socket as TcpSocket, SocketBuffer, State};
use smoltcp::wire::Ipv4Address;

use crate::net;
use crate::range::ByteRange;

const TELNET_PORT: u16 = 23;
const LINE_LEN: usize = 256;

/// Receive and transmit buffers for the console socket.
static mut TELNET_RX_BUF: [u8; 2048] = [0; 2048];
static mut TELNET_TX_BUF: [u8; 16384] = [0; 16384];

/// Set when the client asks the kernel to continue booting.
static BOOT_REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set when the client asks the kernel to restart from the PROM.
static REBOOT_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Console session state.
struct Console {
    handle: smoltcp::iface::SocketHandle,
    line: [u8; LINE_LEN],
    line_len: usize,
    connected: bool,
    /// Telnet IAC parser state: 0 = data, 1 = after IAC, 2 = swallowing option.
    iac: u8,
    /// The previous byte was CR, so a following LF must not run the line twice.
    last_cr: bool,
    /// A deferred HTTP fetch, run outside the socket-set borrow.
    pending_http: Option<HttpRequest>,
    /// A deferred NTP query, run outside the socket-set borrow.
    pending_ntp: Option<NtpRequest>,
    /// A deferred ICMP echo run, outside the socket-set borrow.
    pending_ping: Option<PingRequest>,
    /// A deferred module fetch and run.
    pending_module: Option<HttpRequest>,
    /// Slot for the pending module request.
    pending_module_slot: crate::module::Slot,
    /// A deferred sash package/mount request.
    pending_sash: Option<crate::sash::Deferred>,
    #[cfg(feature = "erofs-debug")]
    /// A deferred EROFS image read over HTTP.
    pending_erofs: Option<crate::erofs_cmd::Request>,
}

/// Parsed `ntp <host>[:port]` request.
struct NtpRequest {
    host: [u8; 64],
    host_len: usize,
    port: u16,
}

/// Parsed `ping <host>` request.
struct PingRequest {
    host: [u8; 64],
    host_len: usize,
}

/// Parsed `http`/`https <ip>:<port><path>` request.
struct HttpRequest {
    host: [u8; 64],
    host_len: usize,
    port: u16,
    path: [u8; 96],
    path_len: usize,
    tls: bool,
    expected: Option<[u8; 32]>,
    range: Option<ByteRange>,
}

static mut CONSOLE: Option<Console> = None;

/// Whether a console client is connected, safe to read while `CONSOLE` is
/// mutably borrowed by `service`.
static CONSOLE_CONNECTED: AtomicBool = AtomicBool::new(false);

/// Telnet output is queued rather than written from inside `service`: a user
/// syscall or diagnostic can print while the socket set is already borrowed.
const OUTPUT_QUEUE_SIZE: usize = 8192;
static mut OUTPUT_QUEUE: [u8; OUTPUT_QUEUE_SIZE] = [0; OUTPUT_QUEUE_SIZE];
static mut OUTPUT_HEAD: usize = 0;
static mut OUTPUT_LEN: usize = 0;
static mut OUTPUT_LAST_CR: bool = false;

/// Register the listening socket.  Call after `net::init()`.
pub fn init() {
    let handle = net::sockets_with(|sockets| {
        // SAFETY: the buffers are static and used by this one socket only.
        let rx = unsafe { &mut TELNET_RX_BUF[..] };
        let tx = unsafe { &mut TELNET_TX_BUF[..] };
        let socket = TcpSocket::new(SocketBuffer::new(rx), SocketBuffer::new(tx));
        let handle = sockets.add(socket);
        let _ = sockets.get_mut::<TcpSocket>(handle).listen(TELNET_PORT);
        handle
    });

    if let Some(handle) = handle {
        // SAFETY: single-threaded initialisation.
        unsafe {
            CONSOLE = Some(Console {
                handle,
                line: [0; LINE_LEN],
                line_len: 0,
                connected: false,
                iac: 0,
                last_cr: false,
                pending_http: None,
                pending_ntp: None,
                pending_ping: None,
                pending_module: None,
                pending_module_slot: crate::module::Slot::A,
                pending_sash: None,
                #[cfg(feature = "erofs-debug")]
                pending_erofs: None,
            });
        }
    }
}

/// Service the listener and session.  Call after `net::poll()`.
pub fn poll() {
    // Run any deferred HTTP fetch first: it needs the interface and the
    // socket set to itself, so it must not run inside `service`'s borrow.
    let request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_http.take());
    if let Some(req) = request {
        let path = core::str::from_utf8(&req.path[..req.path_len]).unwrap_or("/");
        let host = core::str::from_utf8(&req.host[..req.host_len]).unwrap_or("");
        let scheme = if req.tls { "https" } else { "http" };
        let expected = req.expected;
        let range = req.range;
        let result = match resolve_host(&req.host[..req.host_len]) {
            Some(ip) => match (req.tls, range) {
                (true, Some(range)) => crate::tls::get_range(ip, req.port, host, path, range),
                (true, None) => crate::tls::get(ip, req.port, host, path),
                (false, Some(range)) => crate::http::get_range(ip, req.port, host, path, range),
                (false, None) => crate::http::get(ip, req.port, host, path),
            },
            None => Err("unknown host"),
        };
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match result {
                    Ok(summary) => {
                        let _ = write!(w, "{}: {}", scheme, summary.status);
                        if let Some(range) = range {
                            let _ = write!(w, " range={}-{}", range.start(), range.end());
                        }
                        match summary.content_length {
                            Some(expected) if expected != summary.body_len => {
                                let _ = write!(w, " {}/{} bytes\r\n", summary.body_len, expected);
                            }
                            _ => {
                                let _ = write!(w, " {} bytes\r\n", summary.body_len);
                            }
                        }
                        let hex = summary.sha256.to_hex();
                        if let Ok(text) = core::str::from_utf8(&hex) {
                            let _ = write!(w, "sha256: {}\r\n", text);
                        }
                        if let Some(want) = expected {
                            if summary.sha256.eq_digest(&crate::sha256::Sha256(want)) {
                                let _ = w.write_str("verify: ok\r\n");
                            } else {
                                let _ = w.write_str("verify: MISMATCH\r\n");
                            }
                        }
                    }
                    Err(err) => {
                        let _ = write!(w, "{}: {}\r\n", scheme, err);
                    }
                }
                let _ = w.sock.send_slice(b"sash> ");
            });
        }
    }

    #[cfg(feature = "erofs-debug")]
    {
        // Run a deferred EROFS image read.  It performs several HTTP range
        // fetches, so it must own the interface outside the console borrow.
        let erofs_request =
            unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_erofs.take());
        if let Some(request) = erofs_request {
            let output = crate::erofs_cmd::run(&request);
            let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
            if let Some(handle) = handle {
                net::sockets_with(|sockets| {
                    let sock = sockets.get_mut::<TcpSocket>(handle);
                    let _ = sock.send_slice(output.as_bytes());
                    let _ = sock.send_slice(b"sash> ");
                });
            }
        }
    }

    // Run any deferred NTP query, for the same reason.
    let ntp_request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_ntp.take());
    if let Some(req) = ntp_request {
        let resolved = resolve_host(&req.host[..req.host_len]);
        let result = resolved.and_then(|ip| crate::ntp::query(ip, req.port));
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match (resolved, result) {
                    (Some(_), Some(unix)) => {
                        let mut buf = [0u8; 20];
                        let n = crate::ntp::format_utc(unix, &mut buf);
                        let text = core::str::from_utf8(&buf[..n]).unwrap_or("?");
                        let _ = write!(w, "ntp: {} UTC ({})\r\n", text, unix);
                    }
                    (Some(_), None) => {
                        let _ = w.write_str("ntp: no reply\r\n");
                    }
                    (None, _) => {
                        let _ = w.write_str("ntp: unknown host\r\n");
                    }
                }
                let _ = w.sock.send_slice(b"sash> ");
            });
        }
    }

    // Run any deferred ICMP echo run, outside the socket-set borrow.
    let ping_request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_ping.take());
    if let Some(req) = ping_request {
        let resolved = resolve_host(&req.host[..req.host_len]);
        let result = resolved.and_then(crate::ping::run);
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match (resolved, result) {
                    (Some(_), Some(summary)) => {
                        let _ = write!(
                            w,
                            "ping: {}/{} replies, avg {} ms\r\n",
                            summary.received, summary.sent, summary.avg_ms
                        );
                    }
                    (Some(_), None) => {
                        let _ = w.write_str("ping: no reply\r\n");
                    }
                    (None, _) => {
                        let _ = w.write_str("ping: unknown host\r\n");
                    }
                }
                let _ = w.sock.send_slice(b"sash> ");
            });
        }
    }

    // Run a deferred module fetch.  The image is downloaded straight into the
    // reserved module area and executed only after its hash checks out.
    let module_request = unsafe { CONSOLE.as_mut() }.and_then(|console| {
        let request = console.pending_module.take()?;
        Some((request, console.pending_module_slot))
    });
    if let Some((req, slot)) = module_request {
        let host = core::str::from_utf8(&req.host[..req.host_len]).unwrap_or("");
        let path = core::str::from_utf8(&req.path[..req.path_len]).unwrap_or("/");
        let outcome: Result<i32, &'static str> = match resolve_host(&req.host[..req.host_len]) {
            Some(ip) => {
                // Zero the area first: the flat image does not include .bss.
                crate::module::clear(slot);
                let area = crate::module::area(slot);
                let fetched = if req.tls {
                    crate::tls::get_to(ip, req.port, host, path, area)
                } else {
                    crate::http::get_to(ip, req.port, host, path, area)
                };
                match fetched {
                    Ok((summary, captured)) => {
                        if captured != summary.body_len {
                            Err("module did not fit")
                        } else {
                            match req.expected {
                                Some(want)
                                    if summary.sha256.eq_digest(&crate::sha256::Sha256(want)) =>
                                {
                                    // SAFETY: the image fits and its hash matched.
                                    unsafe { Ok(crate::module::call_entry(slot, captured)) }
                                }
                                Some(_) => Err("module hash mismatch"),
                                None => Err("module needs a sha256"),
                            }
                        }
                    }
                    Err(err) => Err(err),
                }
            }
            None => Err("unknown host"),
        };

        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let mut w = SocketWriter { sock };
                match outcome {
                    Ok(code) => {
                        let _ = write!(w, "module: returned {}\r\n", code);
                    }
                    Err(err) => {
                        let _ = write!(w, "module: {}\r\n", err);
                    }
                }
                let _ = w.sock.send_slice(b"sash> ");
            });
        }
    }

    // A sash package/index or remote-mount request runs outside the socket
    // borrow, like the module and EROFS commands.
    let sash_request = unsafe { CONSOLE.as_mut() }.and_then(|console| console.pending_sash.take());
    if let Some(request) = sash_request {
        let output = crate::sash::run_deferred(request);
        let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
        if let Some(handle) = handle {
            net::sockets_with(|sockets| {
                // A userland run may have queued console output while the
                // socket was not borrowed.  Deliver it before the command's
                // result and the next prompt.
                drain_output(sockets, handle);
                let sock = sockets.get_mut::<TcpSocket>(handle);
                let _ = sock.send_slice(output);
                let _ = sock.send_slice(b"sash> ");
            });
        }
    }

    // SAFETY: single-threaded kernel; the console is set once by init().
    let console = unsafe { CONSOLE.as_mut() };
    let Some(console) = console else { return };
    net::sockets_with(|sockets| service(sockets, console));
    let handle = unsafe { CONSOLE.as_ref() }.map(|console| console.handle);
    if let Some(handle) = handle {
        net::sockets_with(|sockets| drain_output(sockets, handle));
    }
}

/// Take and clear a telnet client's request to retry the userland image.
pub fn boot_requested() -> bool {
    BOOT_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Whether a telnet client asked the kernel to reboot.
pub fn reboot_requested() -> bool {
    REBOOT_REQUESTED.load(Ordering::SeqCst)
}

/// Queue console output for the connected telnet client.
///
/// This is safe to call from inside `service`: it never touches the socket or
/// the `CONSOLE` object.  The queue is drained by [`poll`] after the socket
/// borrow ends.  Output generated without a client is dropped.
pub fn write_output(data: &[u8]) {
    if !CONSOLE_CONNECTED.load(Ordering::SeqCst) || data.is_empty() {
        return;
    }
    // SAFETY: single-threaded kernel; the queue is only touched here and in
    // drain_output, which never runs concurrently.
    unsafe {
        for &byte in data {
            if byte == b'\n' && !OUTPUT_LAST_CR {
                push_output(b'\r');
            }
            push_output(byte);
            OUTPUT_LAST_CR = byte == b'\r';
        }
    }
}

unsafe fn push_output(byte: u8) {
    if OUTPUT_LEN == OUTPUT_QUEUE_SIZE {
        // Keep the newest bytes; losing a diagnostic is worse than losing an
        // old line when a client is not draining.
        OUTPUT_HEAD = (OUTPUT_HEAD + 1) % OUTPUT_QUEUE_SIZE;
        OUTPUT_LEN -= 1;
    }
    let tail = (OUTPUT_HEAD + OUTPUT_LEN) % OUTPUT_QUEUE_SIZE;
    OUTPUT_QUEUE[tail] = byte;
    OUTPUT_LEN += 1;
}

fn clear_output() {
    unsafe {
        OUTPUT_HEAD = 0;
        OUTPUT_LEN = 0;
        OUTPUT_LAST_CR = false;
    }
}

fn drain_output(sockets: &mut SocketSet<'static>, handle: smoltcp::iface::SocketHandle) {
    drain_to_socket(sockets.get_mut::<TcpSocket>(handle));
}

/// Send queued console bytes to one socket, preserving the queue if the
/// transmit window is currently full.
fn drain_to_socket(sock: &mut TcpSocket) {
    // SAFETY: single-threaded kernel; only this function and write_output
    // touch the queue, and they never run concurrently.
    unsafe {
        while OUTPUT_LEN > 0 && sock.can_send() {
            let first = OUTPUT_HEAD;
            let contiguous = OUTPUT_LEN.min(OUTPUT_QUEUE_SIZE - first);
            match sock.send_slice(&OUTPUT_QUEUE[first..first + contiguous]) {
                Ok(0) => break,
                Ok(sent) => {
                    OUTPUT_HEAD = (OUTPUT_HEAD + sent) % OUTPUT_QUEUE_SIZE;
                    OUTPUT_LEN -= sent;
                }
                Err(_) => break,
            }
        }
    }
}

fn service(sockets: &mut SocketSet<'static>, console: &mut Console) {
    let sock = sockets.get_mut::<TcpSocket>(console.handle);

    // Once the peer has gone, re-arm the listener for the next client.
    if matches!(
        sock.state(),
        State::Closed | State::TimeWait | State::LastAck
    ) {
        sock.abort();
        let _ = sock.listen(TELNET_PORT);
        console.connected = false;
        CONSOLE_CONNECTED.store(false, Ordering::SeqCst);
        clear_output();
        return;
    }

    let now_connected = sock.remote_endpoint().is_some()
        && matches!(
            sock.state(),
            State::Established | State::FinWait1 | State::FinWait2 | State::CloseWait
        );

    if now_connected && !console.connected {
        console.connected = true;
        CONSOLE_CONNECTED.store(true, Ordering::SeqCst);
        clear_output();
        console.line_len = 0;
        console.iac = 0;
        console.last_cr = false;
        // IAC WILL ECHO, IAC WILL SUPPRESS-GO-AHEAD.
        let _ = sock.send_slice(&[0xFF, 0xFB, 0x01, 0xFF, 0xFB, 0x03]);
        let _ =
            sock.send_slice(b"\r\nIRIX 7 sash (kernel console)\r\nType `help` for commands.\r\n");
        prompt(sock);
    } else if !now_connected {
        console.connected = false;
        CONSOLE_CONNECTED.store(false, Ordering::SeqCst);
        clear_output();
    }

    let mut buf = [0u8; 64];
    while sock.can_recv() {
        let n = match sock.recv_slice(&mut buf) {
            Ok(n) => n,
            Err(_) => break,
        };
        for &b in &buf[..n] {
            input(sock, console, b);
        }
    }

    // The peer sent FIN.  Close our side so the next poll reaches Closed and
    // the socket can listen again.
    if sock.state() == State::CloseWait {
        sock.close();
        console.connected = false;
        CONSOLE_CONNECTED.store(false, Ordering::SeqCst);
        clear_output();
    }
}

fn input(sock: &mut TcpSocket, console: &mut Console, b: u8) {
    // Telnet command stream: IAC (0xFF) followed by a command byte, then an
    // option byte for WILL/WONT/DO/DONT.  Ignore the whole negotiation.
    if console.iac == 1 {
        console.iac = if b == 0xFF { 0 } else { 2 };
        return;
    }
    if console.iac == 2 {
        console.iac = 0;
        return;
    }
    if b == 0xFF {
        console.iac = 1;
        return;
    }

    match b {
        b'\r' => {
            console.last_cr = true;
            execute(sock, console);
        }
        b'\n' => {
            if console.last_cr {
                console.last_cr = false;
            } else {
                execute(sock, console);
            }
        }
        0x08 | 0x7F => {
            if console.line_len > 0 {
                console.line_len -= 1;
                let _ = sock.send_slice(b"\x08 \x08");
            }
        }
        0x03 => {
            console.line_len = 0;
            let _ = sock.send_slice(b"^C\r\n");
            prompt(sock);
        }
        0x15 => {
            console.line_len = 0;
            let _ = sock.send_slice(b"\r\n");
            prompt(sock);
        }
        b if (0x20..0x7F).contains(&b) => {
            if console.line_len < LINE_LEN {
                console.line[console.line_len] = b;
                console.line_len += 1;
                let _ = sock.send_slice(&[b]);
            }
        }
        _ => {}
    }
}

struct SocketSink<'a, 'b> {
    sock: &'a mut TcpSocket<'b>,
}

impl crate::sash::Sink for SocketSink<'_, '_> {
    fn write(&mut self, bytes: &[u8]) {
        let _ = self.sock.send_slice(bytes);
    }
}

fn execute(sock: &mut TcpSocket, console: &mut Console) {
    let _ = sock.send_slice(b"\r\n");
    let line = &console.line[..console.line_len];
    let line = trim(line);

    {
        let mut sink = SocketSink { sock: &mut *sock };
        match crate::sash::execute(line, &mut sink) {
            crate::sash::Outcome::Handled => {
                console.line_len = 0;
                drain_to_socket(sock);
                prompt(sock);
                return;
            }
            crate::sash::Outcome::Deferred(request) => {
                console.line_len = 0;
                console.pending_sash = Some(request);
                let _ = sock.send_slice(b"sash: fetching...\r\n");
                drain_to_socket(sock);
                return;
            }
            crate::sash::Outcome::Unknown => {}
        }
    }

    match line {
        b"" => {}
        b"help" => {
            let _ = sock.send_slice(
                b"commands:\r\n\
                  \x20 help          this text\r\n\
                  \x20 status        kernel and network state\r\n\
                  \x20 diagnostics   CPU, memory and network inventory\r\n\
                  \x20 net           interface addresses\r\n\
                  \x20 mem           RAM accounting\r\n\
                  \x20 scsi          reset/probe the WD33C93\r\n\
                  \x20 time          time since boot\r\n\
                  \x20 echo <text>   echo the argument\r\n\
                  \x20 http <host>:<port><path> [sha256] [range=start-end]  fetch over HTTP\r\n\
                  \x20 https <host>:<port><path> [sha256] [range=start-end]  fetch over HTTPS (TLS 1.3)\r\n\
                  \x20 ntp <host>[:port]  set the clock from an NTP server\r\n\
                  \x20 ping <host>     ICMP echo\r\n\
                  \x20 module [https://]host:port/path <sha256>  fetch and run a RAM module\r\n\
                  \x20 boot          retry the target-2 userland image\r\n\
                  \x20 panic         dump the panic screen and halt (test hook)\r\n\
                  \x20 trap          exercise the exception vector (test hook)\r\n\
                  \x20 tlb           exercise the TLB-refill vector (test hook)\r\n\
                  \x20 gfx           verify REX3 rgbmode and FIFO drain (test hook)\r\n\
                  \x20 user          enter the fixed user-mode smoke test\r\n\
                  \x20 userelf       load and run an n32 ELF from SCSI target 2\r\n\
                  \x20 reboot        restart from the PROM\r\n",
            );
        }
        b"status" => {
            let mut buf = [0u8; 512];
            let n = crate::status::text(&mut buf);
            let _ = sock.send_slice(&buf[..n]);
        }
        b"diagnostics" => {
            crate::diagnostics::run();
        }
        b"net" => {
            let mut buf = [0u8; 256];
            let n = crate::status::net(&mut buf);
            let _ = sock.send_slice(&buf[..n]);
        }
        b"mem" => {
            let mut buf = [0u8; 256];
            let n = crate::status::mem(&mut buf);
            let _ = sock.send_slice(&buf[..n]);
        }
        b"scsi" => {
            let mut writer = SocketWriter { sock };
            match crate::scsi::probe() {
                Ok(probe) => {
                    let _ = write!(
                        writer,
                        "scsi: WD33C93 rev 0x{:02x} csr 0x{:02x} asr 0x{:02x}->0x{:02x} \
                         id 0x{:02x}\r\n",
                        probe.revision, probe.csr, probe.asr_before, probe.asr_after, probe.own_id
                    );
                    match crate::scsi::inquiry(1) {
                        Ok(data) => {
                            let vendor = core::str::from_utf8(&data[8..16]).unwrap_or("?");
                            let product = core::str::from_utf8(&data[16..32]).unwrap_or("?");
                            let revision = core::str::from_utf8(&data[32..36]).unwrap_or("?");
                            let _ = write!(
                                writer,
                                "scsi: target 1 INQUIRY vendor `{}` product `{}` rev `{}`\r\n",
                                vendor.trim(),
                                product.trim(),
                                revision.trim()
                            );
                            let mut sector = [0u8; 512];
                            match crate::scsi::read_sector(1, 0, &mut sector) {
                                Ok(()) => {
                                    let magic = u32::from_be_bytes([
                                        sector[0], sector[1], sector[2], sector[3],
                                    ]);
                                    let _ = write!(
                                        writer,
                                        "scsi: read block 0 magic 0x{magic:08x}\r\n"
                                    );
                                    let _ = writer.write_str("scsi: sector:");
                                    for byte in &sector[..16] {
                                        let _ = write!(writer, " {byte:02x}");
                                    }
                                    let _ = writer.write_str("\r\n");
                                }
                                Err(error) => {
                                    let _ = write!(writer, "scsi: read block 0: {error}\r\n");
                                }
                            }
                        }
                        Err(error) => {
                            let _ = write!(writer, "scsi: target 1: {error}\r\n");
                        }
                    }
                }
                Err(error) => {
                    let _ = write!(writer, "scsi: {error}\r\n");
                }
            }
        }
        b"time" => {
            let mut buf = [0u8; 128];
            let n = crate::status::time(&mut buf);
            let _ = sock.send_slice(&buf[..n]);
        }
        b"boot" => {
            let _ = sock.send_slice(b"booting\r\n");
            BOOT_REQUESTED.store(true, Ordering::SeqCst);
        }
        b"panic" => {
            let _ = sock.send_slice(b"deliberate panic from the console\r\n");
            panic!("deliberate panic from the console");
        }
        b"trap" => {
            crate::traps::self_test();
        }
        b"tlb" => {
            crate::traps::tlb_self_test();
        }
        b"gfx" => {
            crate::gfx::self_test();
        }
        b"user" => {
            let _ = sock.send_slice(b"user: entering (write returns, exit returns here)\r\n");
            unsafe {
                crate::traps::user_from_console();
            }
            // Drop the smoke-test mappings before returning to the command loop.
            crate::tlb::invalidate_index(40);
            crate::tlb::invalidate_index(41);
            let _ = sock.send_slice(b"user: returned to console\r\n");
        }
        b"userelf" => {
            let _ = sock.send_slice(b"userelf: loading target 2\r\n");
            unsafe {
                crate::traps::user_from_console_elf();
            }
            crate::user::cleanup();
            let _ = sock.send_slice(b"userelf: returned to console\r\n");
        }
        b"reboot" => {
            let _ = sock.send_slice(b"rebooting\r\n");
            REBOOT_REQUESTED.store(true, Ordering::SeqCst);
        }
        _ if line.starts_with(b"http ") => match parse_http(trim(&line[5..]), false) {
            Some(request) => {
                let _ = sock.send_slice(b"http: fetching\r\n");
                console.pending_http = Some(request);
            }
            None => {
                let _ = sock
                    .send_slice(b"usage: http <host>:<port><path> [sha256] [range=start-end]\r\n");
            }
        },
        _ if line.starts_with(b"https ") => match parse_http(trim(&line[6..]), true) {
            Some(request) => {
                let _ = sock.send_slice(b"https: fetching\r\n");
                console.pending_http = Some(request);
            }
            None => {
                let _ = sock
                    .send_slice(b"usage: https <host>:<port><path> [sha256] [range=start-end]\r\n");
            }
        },
        _ if line.starts_with(b"module ") => {
            let mut arg = trim(&line[7..]);
            let slot = if let Some(rest) = arg.strip_prefix(b"b ") {
                arg = trim(rest);
                crate::module::Slot::B
            } else {
                crate::module::Slot::A
            };
            let (arg, tls) = if let Some(rest) = arg.strip_prefix(b"https://") {
                (rest, true)
            } else if let Some(rest) = arg.strip_prefix(b"http://") {
                (rest, false)
            } else {
                // Secure by default: the TLS client is the module transport.
                (arg, true)
            };
            match parse_http(arg, tls) {
                Some(request) => {
                    let _ = sock.send_slice(b"module: fetching\r\n");
                    console.pending_module = Some(request);
                    console.pending_module_slot = slot;
                }
                None => {
                    let _ =
                        sock.send_slice(b"usage: module [b] [https://]host:port/path <sha256>\r\n");
                }
            }
        }
        #[cfg(feature = "erofs-debug")]
        _ if line.starts_with(b"erofs ") => {
            match crate::erofs_cmd::Request::parse(trim(&line[6..])) {
                Some(request) => {
                    let _ = sock.send_slice(b"erofs: mounting\r\n");
                    console.pending_erofs = Some(request);
                }
                None => {
                    let _ = sock.send_slice(
                        b"usage: erofs [http|https]://<host>:<port><path> [sha256=hex] \
                          [info|ls|cat] [path]\r\n",
                    );
                }
            }
        }
        _ if line.starts_with(b"ping ") => match parse_ping(trim(&line[5..])) {
            Some(request) => {
                let _ = sock.send_slice(b"ping: probing\r\n");
                console.pending_ping = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: ping <host>\r\n");
            }
        },
        _ if line.starts_with(b"ntp ") => match parse_ntp(trim(&line[4..])) {
            Some(request) => {
                let _ = sock.send_slice(b"ntp: querying\r\n");
                console.pending_ntp = Some(request);
            }
            None => {
                let _ = sock.send_slice(b"usage: ntp <host>[:port]\r\n");
            }
        },
        _ if line.starts_with(b"echo ") => {
            let _ = sock.send_slice(&line[5..]);
            let _ = sock.send_slice(b"\r\n");
        }
        _ => {
            let _ = sock.send_slice(b"unknown command; type `help`\r\n");
        }
    }

    console.line_len = 0;
    drain_to_socket(sock);
    prompt(sock);
}

/// The local keyboard/display sash line buffer.  It is independent of the
/// telnet socket so sash still works with `net=off`.
static mut LOCAL_LINE: [u8; LINE_LEN] = [0; LINE_LEN];
static mut LOCAL_LEN: usize = 0;

struct LocalSink;

impl crate::sash::Sink for LocalSink {
    fn write(&mut self, bytes: &[u8]) {
        crate::write_bytes(bytes);
    }
}

/// Print a local sash prompt on the graphics and serial consoles.
pub fn local_prompt() {
    crate::write_bytes(b"sash> ");
}

/// Poll the local keyboard and feed it to the local sash session.
pub fn poll_local() {
    loop {
        let Some(key) = crate::kbd::poll_key() else {
            break;
        };
        let byte = match key {
            crate::kbd::Key::Char(byte) => byte,
            crate::kbd::Key::Enter => b'\r',
            crate::kbd::Key::Backspace => 0x7F,
            crate::kbd::Key::Escape => 0x1B,
            _ => continue,
        };
        local_input(byte);
    }
}

fn local_input(byte: u8) {
    match byte {
        b'\r' | b'\n' => {
            crate::write_bytes(b"\r\n");
            let line = unsafe { &LOCAL_LINE[..LOCAL_LEN] };
            let mut sink = LocalSink;
            match crate::sash::execute(line, &mut sink) {
                crate::sash::Outcome::Handled => {}
                crate::sash::Outcome::Deferred(request) => {
                    let output = crate::sash::run_deferred(request);
                    crate::sash::Sink::write(&mut sink, output);
                }
                crate::sash::Outcome::Unknown => {
                    crate::sash::Sink::write(&mut sink, b"unknown command; type `help`\r\n");
                }
            }
            unsafe {
                LOCAL_LEN = 0;
            }
            local_prompt();
        }
        0x08 | 0x7F => unsafe {
            if LOCAL_LEN > 0 {
                LOCAL_LEN -= 1;
                crate::write_bytes(b"\x08 \x08");
            }
        },
        0x03 => unsafe {
            LOCAL_LEN = 0;
            crate::write_bytes(b"^C\r\n");
            local_prompt();
        },
        byte if (0x20..0x7F).contains(&byte) => unsafe {
            if LOCAL_LEN < LINE_LEN {
                LOCAL_LINE[LOCAL_LEN] = byte;
                LOCAL_LEN += 1;
                crate::write_bytes(&[byte]);
            }
        },
        _ => {}
    }
}

/// Ask the idle loop to retry the target-2 user image.
pub fn request_boot() {
    BOOT_REQUESTED.store(true, Ordering::SeqCst);
}

/// Ask the idle loop to restart from the PROM.
pub fn request_reboot() {
    REBOOT_REQUESTED.store(true, Ordering::SeqCst);
}

fn prompt(sock: &mut TcpSocket) {
    let _ = sock.send_slice(b"sash> ");
}

fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = line.split_first() {
        if b == b' ' || b == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((&b, rest)) = line.split_last() {
        if b == b' ' || b == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    line
}

/// `core::fmt::Write` adapter that pushes bytes straight at the socket.
struct SocketWriter<'a, 'b> {
    sock: &'a mut TcpSocket<'b>,
}

impl Write for SocketWriter<'_, '_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        if self.sock.can_send() {
            let _ = self.sock.send_slice(s.as_bytes());
        }
        Ok(())
    }
}

/// Dotted-decimal wrapper for `Ipv4Address` (`Display` is not implemented).
struct DisplayIpv4(Ipv4Address);

impl core::fmt::Display for DisplayIpv4 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let o = self.0.octets();
        write!(f, "{}.{}.{}.{}", o[0], o[1], o[2], o[3])
    }
}

/// Parse `http|https <host>:<port><path> [sha256-hex] [range=start-end]`.
fn parse_http(arg: &[u8], tls: bool) -> Option<HttpRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    let mut parts = s.split_whitespace();
    let url = parts.next()?;
    let mut expected = None;
    let mut range = None;
    for part in parts {
        if let Some(rest) = part.strip_prefix("range=") {
            if range.is_some() {
                return None;
            }
            range = Some(ByteRange::parse(rest)?);
        } else if expected.is_none() {
            expected = Some(parse_sha256_hex(part)?);
        } else {
            return None;
        }
    }
    let (host, rest) = url.split_once(':')?;
    let slash = rest.find('/')?;
    let (port_str, path) = rest.split_at(slash);
    let port: u16 = port_str.parse().ok()?;
    if host.is_empty() || host.len() > 64 || path.is_empty() || path.len() > 96 {
        return None;
    }
    let mut host_buf = [0u8; 64];
    host_buf[..host.len()].copy_from_slice(host.as_bytes());
    let mut buf = [0u8; 96];
    buf[..path.len()].copy_from_slice(path.as_bytes());
    Some(HttpRequest {
        host: host_buf,
        host_len: host.len(),
        port,
        path: buf,
        path_len: path.len(),
        tls,
        expected,
        range,
    })
}

/// Parse a 64-digit hex SHA-256 digest.
pub(crate) fn parse_sha256_hex(s: &str) -> Option<[u8; 32]> {
    let bytes = s.as_bytes();
    if bytes.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (hex_digit(bytes[i * 2])? << 4) | hex_digit(bytes[i * 2 + 1])?;
    }
    Some(out)
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Parse a dotted-decimal IPv4 address.
fn parse_ipv4(s: &str) -> Option<Ipv4Address> {
    let mut octets = [0u8; 4];
    let mut n = 0;
    for part in s.split('.') {
        if n == 4 {
            return None;
        }
        octets[n] = part.parse().ok()?;
        n += 1;
    }
    if n != 4 {
        return None;
    }
    Some(Ipv4Address::new(octets[0], octets[1], octets[2], octets[3]))
}

/// Parse `ntp <host>[:port]`.
fn parse_ntp(arg: &[u8]) -> Option<NtpRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    let (host, port) = match s.split_once(':') {
        Some((host, port)) => (host, port.parse().ok()?),
        None => (s, 123),
    };
    if host.is_empty() || host.len() > 64 {
        return None;
    }
    let mut host_buf = [0u8; 64];
    host_buf[..host.len()].copy_from_slice(host.as_bytes());
    Some(NtpRequest {
        host: host_buf,
        host_len: host.len(),
        port,
    })
}

/// Parse `ping <host>`.
fn parse_ping(arg: &[u8]) -> Option<PingRequest> {
    let s = core::str::from_utf8(arg).ok()?;
    if s.is_empty() || s.len() > 64 {
        return None;
    }
    let mut host = [0u8; 64];
    host[..s.len()].copy_from_slice(s.as_bytes());
    Some(PingRequest {
        host,
        host_len: s.len(),
    })
}

/// Resolve a host name or dotted-decimal address.
pub(crate) fn resolve_host(host: &[u8]) -> Option<Ipv4Address> {
    let name = core::str::from_utf8(host).ok()?;
    if let Some(ip) = parse_ipv4(name) {
        return Some(ip);
    }
    let server = net::dns_server()?;
    crate::dns::resolve(name, server)
}
