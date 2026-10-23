// SPDX-License-Identifier: GPL-3.0-only
//! HTTP and MCP server module.
//!
//! Listens on guest port 80 through the version 3 TCP services.  `GET /`
//! returns a small status page; `POST /mcp` speaks JSON-RPC 2.0 with the MCP
//! methods `initialize`, `tools/list` and `tools/call` (`status`, `echo`,
//! `console`).  This is the first long-running module and the prototype for
//! the control plane: the compressed PROM image does not grow.

#![no_std]
#![no_main]

/// Must match `kernel::module::Services` (version 3).
#[repr(C)]
pub struct Services {
    pub version: u32,
    pub write: extern "C" fn(*const u8, usize),
    pub clock_ms: extern "C" fn() -> u64,
    pub alloc: unsafe extern "C" fn(usize, usize) -> *mut u8,
    pub poll: extern "C" fn(),
    pub tcp_listen: unsafe extern "C" fn(u32, u16) -> i32,
    pub tcp_state: extern "C" fn(u32) -> u32,
    pub tcp_recv: unsafe extern "C" fn(u32, *mut u8, usize) -> isize,
    pub tcp_send: unsafe extern "C" fn(u32, *const u8, usize) -> isize,
    pub tcp_close: extern "C" fn(u32),
    pub tcp_abort: extern "C" fn(u32),
}

#[repr(C)]
pub struct ModuleVtable {
    pub status: i32,
    pub tick: Option<unsafe extern "C" fn(*const Services) -> i32>,
}

/// Listening phase.
const ARM: u32 = 0;
const WAIT_REQUEST: u32 = 1;
const WAIT_CLOSE: u32 = 2;
const CLOSING: u32 = 3;

const PORT: u16 = 80;
const REQUEST_CAP: usize = 2048;

static VTABLE: ModuleVtable = ModuleVtable {
    status: 0,
    tick: Some(tick),
};
static ERROR_VTABLE: ModuleVtable = ModuleVtable {
    status: 1,
    tick: None,
};

static mut PHASE: u32 = ARM;
static mut REQUEST: [u8; REQUEST_CAP] = [0; REQUEST_CAP];
static mut REQ_LEN: usize = 0;
static mut IN_CONN: bool = false;
static mut RESPONSE: [u8; 4096] = [0; 4096];

/// The slot serving the current connection, and the slot armed for the next
/// one.  Alternating the two hides the close handshake from a back-to-back
/// client.
static mut CURRENT: u32 = 0;
static mut NEXT: u32 = 1;
static mut NEXT_ARMED: bool = false;

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &ERROR_VTABLE;
    };
    if services.version < 3 {
        return &ERROR_VTABLE;
    }
    let message = b"mcp: listening on port 80\n";
    (services.write)(message.as_ptr(), message.len());
    &VTABLE
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };

    match unsafe { PHASE } {
        ARM => {
            unsafe {
                REQ_LEN = 0;
                IN_CONN = false;
            }
            let slot = unsafe { CURRENT };
            if (services.tcp_listen)(slot, PORT) == 0 {
                unsafe {
                    PHASE = WAIT_REQUEST;
                }
            }
        }
        WAIT_REQUEST => {
            let slot = unsafe { CURRENT };
            let state = (services.tcp_state)(slot);
            if state == 2 {
                if !unsafe { IN_CONN } {
                    unsafe {
                        REQ_LEN = 0;
                        IN_CONN = true;
                    }
                }
                let request = unsafe { &mut *core::ptr::addr_of_mut!(REQUEST) };
                let used = unsafe { REQ_LEN };
                if used < request.len() {
                    let n = (services.tcp_recv)(
                        slot,
                        request.as_mut_ptr().add(used),
                        request.len() - used,
                    );
                    if n > 0 {
                        unsafe {
                            REQ_LEN = used + n as usize;
                        }
                    } else if n < 0 {
                        (services.tcp_abort)(slot);
                        unsafe {
                            PHASE = ARM;
                        }
                    }
                }
                let used = unsafe { REQ_LEN };
                if used > 0 && request_complete(&request[..used]) {
                    handle_request(services, slot);
                    // Arm the other slot before the client closes, so the
                    // next connection lands on a listening socket.
                    let next = unsafe { NEXT };
                    if (services.tcp_listen)(next, PORT) == 0 {
                        unsafe {
                            NEXT_ARMED = true;
                        }
                    }
                    unsafe {
                        IN_CONN = false;
                        PHASE = WAIT_CLOSE;
                    }
                }
            } else if state == 3 {
                (services.tcp_close)(slot);
                unsafe {
                    PHASE = CLOSING;
                }
            } else if state == 0 {
                unsafe {
                    PHASE = ARM;
                }
            }
        }
        WAIT_CLOSE => {
            let slot = unsafe { CURRENT };
            match (services.tcp_state)(slot) {
                3 => {
                    (services.tcp_close)(slot);
                    unsafe {
                        PHASE = CLOSING;
                    }
                }
                0 => unsafe {
                    finish_close();
                },
                _ => {}
            }
        }
        CLOSING => {
            let slot = unsafe { CURRENT };
            if (services.tcp_state)(slot) == 0 {
                (services.tcp_abort)(slot);
                unsafe {
                    finish_close();
                }
            }
        }
        _ => unsafe {
            PHASE = ARM;
        },
    }
    0
}

/// Move to the pre-armed listener, or re-arm the current slot if the second
/// listener could not be created.
unsafe fn finish_close() {
    unsafe {
        IN_CONN = false;
    }
    if unsafe { NEXT_ARMED } {
        let old = unsafe { CURRENT };
        unsafe {
            CURRENT = NEXT;
            NEXT = old;
            NEXT_ARMED = false;
            PHASE = WAIT_REQUEST;
        }
    } else {
        unsafe {
            PHASE = ARM;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP request handling
// ---------------------------------------------------------------------------

fn request_complete(req: &[u8]) -> bool {
    let Some(header_end) = find(req, b"\r\n\r\n").map(|i| i + 4) else {
        // An oversized request without a header terminator is dispatched as-is.
        return req.len() == REQUEST_CAP;
    };
    let content = content_length(&req[..header_end]);
    req.len() >= header_end + content
}

fn content_length(headers: &[u8]) -> usize {
    let mut p = 0;
    while p < headers.len() {
        let end = find(&headers[p..], b"\r\n")
            .map(|i| p + i)
            .unwrap_or(headers.len());
        let line = &headers[p..end];
        if starts_with_ci(line, b"content-length:") {
            let mut q = b"content-length:".len();
            while q < line.len() && (line[q] == b' ' || line[q] == b'\t') {
                q += 1;
            }
            let mut value = 0usize;
            while q < line.len() && line[q].is_ascii_digit() {
                value = value * 10 + (line[q] - b'0') as usize;
                q += 1;
            }
            return value;
        }
        p = end + 2;
    }
    0
}

fn handle_request(services: &Services, slot: u32) {
    let request = unsafe { &*core::ptr::addr_of!(REQUEST) };
    let used = unsafe { REQ_LEN };
    let request = &request[..used.min(REQUEST_CAP)];
    let header_end = find(request, b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(request.len());
    let body = &request[header_end..];
    let (path, is_post) = request_line(request);

    let response = unsafe { &mut *core::ptr::addr_of_mut!(RESPONSE) };
    let (content_type, n) = if is_post && path.starts_with(b"/mcp") {
        (
            &b"application/json"[..],
            mcp_response(services, body, response),
        )
    } else {
        (&b"text/plain"[..], status_page(services, response))
    };
    send_http(services, slot, content_type, &response[..n]);
}

fn request_line(request: &[u8]) -> (&[u8], bool) {
    let end = find(request, b"\r\n").unwrap_or(request.len());
    let line = &request[..end];
    let is_post = starts_with_ci(line, b"POST ");
    let path = match find(line, b" ") {
        Some(i) => {
            let rest = &line[i + 1..];
            match find(rest, b" ") {
                Some(j) => &rest[..j],
                None => rest,
            }
        }
        None => b"/",
    };
    (path, is_post)
}

fn status_page(services: &Services, out: &mut [u8]) -> usize {
    let mut w = Out::new(out);
    w.push(b"IRIX 7 module HTTP server\nuptime: ");
    w.u64((services.clock_ms)() / 1000);
    w.push(b" s\n");
    w.len
}

fn send_http(services: &Services, slot: u32, content_type: &[u8], body: &[u8]) {
    let mut header = [0u8; 160];
    let mut w = Out::new(&mut header);
    w.push(b"HTTP/1.0 200 OK\r\nContent-Type: ");
    w.push(content_type);
    w.push(b"\r\nConnection: close\r\nContent-Length: ");
    w.u64(body.len() as u64);
    w.push(b"\r\n\r\n");
    let n = w.len;
    send_all(services, slot, &header[..n]);
    send_all(services, slot, body);
}

// ---------------------------------------------------------------------------
// JSON-RPC / MCP
// ---------------------------------------------------------------------------

fn mcp_response(services: &Services, body: &[u8], out: &mut [u8]) -> usize {
    let id = json_id(body);
    let mut w = Out::new(out);
    match json_string(body, b"\"method\"") {
        Some(b"initialize") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"protocolVersion\":\"2024-11-05\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"irix7-mcp\",\"version\":\"0.1.0\"}}}");
        }
        Some(b"tools/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"tools\":[{\"name\":\"status\",\"description\":\"Guest uptime\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"echo\",\"description\":\"Echo text\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"console\",\"description\":\"Write a line to the guest console\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}}]}}");
        }
        Some(b"tools/call") => {
            let name = json_string(body, b"\"name\"");
            let text = json_string(body, b"\"text\"").unwrap_or(b"");
            match name {
                Some(b"status") => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"uptime: ");
                    w.u64((services.clock_ms)() / 1000);
                    w.push(b" s\"}]}}");
                }
                Some(b"echo") => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                    w.json_string(text);
                    w.push(b"}]}}");
                }
                Some(b"console") => {
                    (services.write)(text.as_ptr(), text.len());
                    (services.write)(b"\r\n".as_ptr(), 2);
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}");
                }
                _ => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"error\":{\"code\":-32602,\"message\":\"unknown tool\"}}");
                }
            }
        }
        _ => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"error\":{\"code\":-32601,\"message\":\"method not found\"}}");
        }
    }
    w.len
}

fn json_id(body: &[u8]) -> &[u8] {
    let Some(mut p) = find(body, b"\"id\"") else {
        return b"null";
    };
    p += 4;
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    let start = p;
    if p < body.len() && body[p] == b'"' {
        p += 1;
        while p < body.len() && body[p] != b'"' {
            p += 1;
        }
        if p < body.len() {
            p += 1;
        }
    } else if body[p..].starts_with(b"null") {
        p += 4;
    } else if body[p..].starts_with(b"false") {
        p += 5;
    } else if body[p..].starts_with(b"true") {
        p += 4;
    } else {
        if p < body.len() && body[p] == b'-' {
            p += 1;
        }
        while p < body.len() && body[p].is_ascii_digit() {
            p += 1;
        }
    }
    &body[start..p]
}

fn json_string<'a>(body: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut p = find(body, key)? + key.len();
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    if p >= body.len() || body[p] != b'"' {
        return None;
    }
    p += 1;
    let start = p;
    while p < body.len() && body[p] != b'"' {
        p += 1;
    }
    if p >= body.len() {
        return None;
    }
    Some(&body[start..p])
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|&i| &haystack[i..i + needle.len()] == needle)
}

fn starts_with_ci(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

struct Out<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, len: 0 }
    }

    fn push(&mut self, data: &[u8]) {
        let n = (self.buf.len() - self.len).min(data.len());
        self.buf[self.len..self.len + n].copy_from_slice(&data[..n]);
        self.len += n;
    }

    fn byte(&mut self, b: u8) {
        if self.len < self.buf.len() {
            self.buf[self.len] = b;
            self.len += 1;
        }
    }

    fn u64(&mut self, mut value: u64) {
        let mut digits = [0u8; 20];
        let mut n = 0;
        loop {
            digits[n] = b'0' + (value % 10) as u8;
            value /= 10;
            n += 1;
            if value == 0 {
                break;
            }
        }
        while n > 0 {
            n -= 1;
            self.byte(digits[n]);
        }
    }

    fn json_string(&mut self, s: &[u8]) {
        self.byte(b'"');
        for &c in s {
            match c {
                b'"' => self.push(b"\\\""),
                b'\\' => self.push(b"\\\\"),
                b'\n' => self.push(b"\\n"),
                b'\r' => self.push(b"\\r"),
                b'\t' => self.push(b"\\t"),
                _ => self.byte(c),
            }
        }
        self.byte(b'"');
    }
}

fn send_all(services: &Services, slot: u32, data: &[u8]) {
    let mut offset = 0;
    while offset < data.len() {
        let n = unsafe { (services.tcp_send)(slot, data[offset..].as_ptr(), data.len() - offset) };
        if n <= 0 {
            return;
        }
        offset += n as usize;
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

// The module links only core and compiler_builtins; provide the byte
// primitives the compiler may emit for array initialisation and copies.
//
/// # Safety
/// `dst` must be valid for writes of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memset(
    dst: *mut core::ffi::c_void,
    c: i32,
    n: usize,
) -> *mut core::ffi::c_void {
    let dst = dst as *mut u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *dst.add(i) = c as u8;
        }
        i += 1;
    }
    dst as *mut core::ffi::c_void
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n`
/// bytes; the regions must not overlap.
#[no_mangle]
pub unsafe extern "C" fn memcpy(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    let mut i = 0;
    while i < n {
        unsafe {
            *d.add(i) = *s.add(i);
        }
        i += 1;
    }
    dst
}

/// # Safety
/// `dst` must be valid for writes of `n` bytes and `src` for reads of `n`
/// bytes; the regions may overlap.
#[no_mangle]
pub unsafe extern "C" fn memmove(
    dst: *mut core::ffi::c_void,
    src: *const core::ffi::c_void,
    n: usize,
) -> *mut core::ffi::c_void {
    let d = dst as *mut u8;
    let s = src as *const u8;
    if (d as usize) < (s as usize) {
        return unsafe { memcpy(dst, src, n) };
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        unsafe {
            *d.add(i) = *s.add(i);
        }
    }
    dst
}

/// # Safety
/// `a` and `b` must be valid for reads of `n` bytes.
#[no_mangle]
pub unsafe extern "C" fn memcmp(
    a: *const core::ffi::c_void,
    b: *const core::ffi::c_void,
    n: usize,
) -> i32 {
    let a = a as *const u8;
    let b = b as *const u8;
    let mut i = 0;
    while i < n {
        let x = unsafe { *a.add(i) };
        let y = unsafe { *b.add(i) };
        if x != y {
            return x as i32 - y as i32;
        }
        i += 1;
    }
    0
}
