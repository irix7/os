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

use module_abi::{ModuleVtable, Services};

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

/// Deferred reset/poweroff: 0 none, 1 reset, 2 poweroff.  The response is
/// queued first; the action fires after a few idle ticks.
static mut ACTION: u32 = 0;
static mut COUNTDOWN: u32 = 0;

/// Bearer token from `mcp.token`; an empty token leaves the endpoint open.
static mut TOKEN: [u8; 64] = [0; 64];
static mut TOKEN_LEN: usize = 0;

/// Module entry.
///
/// # Safety
/// `services` must point to a valid service table supplied by the kernel.
#[no_mangle]
pub unsafe extern "C" fn _start(services: *const Services) -> *const ModuleVtable {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return &ERROR_VTABLE;
    };
    if services.version < 8 {
        return &ERROR_VTABLE;
    }
    let mut token = [0u8; 64];
    let n = unsafe {
        (services.config_get)(
            b"mcp.token".as_ptr(),
            b"mcp.token".len(),
            token.as_mut_ptr(),
            token.len(),
        )
    };
    if n > 0 {
        let n = (n as usize).min(token.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                token.as_ptr(),
                core::ptr::addr_of_mut!(TOKEN) as *mut u8,
                n,
            );
            TOKEN_LEN = n;
        }
        let message = b"mcp: bearer token required\n";
        (services.write)(message.as_ptr(), message.len());
    } else {
        let message = b"mcp: no bearer token configured\n";
        (services.write)(message.as_ptr(), message.len());
    }
    let message = b"mcp: listening on port 80\n";
    (services.write)(message.as_ptr(), message.len());
    &VTABLE
}

unsafe extern "C" fn tick(services: *const Services) -> i32 {
    let Some(services) = (unsafe { services.as_ref() }) else {
        return 1;
    };

    // A deferred system action runs after the tool response has had a few
    // poll cycles to reach the client.
    if unsafe { ACTION } != 0 {
        let left = unsafe { COUNTDOWN };
        if left > 0 {
            unsafe {
                COUNTDOWN = left - 1;
            }
        } else {
            if unsafe { ACTION } == 1 {
                (services.reset)();
            } else {
                (services.poweroff)();
            }
        }
    }

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
    if is_post && path.starts_with(b"/mcp") && !authorized(request) {
        send_unauthorized(services, slot);
        return;
    }

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

fn authorized(request: &[u8]) -> bool {
    let token = unsafe { &*core::ptr::addr_of!(TOKEN) };
    let token = &token[..unsafe { TOKEN_LEN }];
    if token.is_empty() {
        return true;
    }
    let mut p = 0;
    while p < request.len() {
        let end = find(&request[p..], b"\r\n")
            .map(|i| p + i)
            .unwrap_or(request.len());
        let line = &request[p..end];
        if starts_with_ci(line, b"authorization:") {
            let mut q = b"authorization:".len();
            while q < line.len() && (line[q] == b' ' || line[q] == b'\t') {
                q += 1;
            }
            if !starts_with_ci(&line[q..], b"bearer ") {
                return false;
            }
            return &line[q + 7..] == token;
        }
        p = end + 2;
    }
    false
}

fn send_unauthorized(services: &Services, slot: u32) {
    send_all(
        services,
        slot,
        b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Bearer\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
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
            let version = json_string(body, b"\"protocolVersion\"").unwrap_or(b"2026-07-28");
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"protocolVersion\":");
            w.json_string(version);
            w.push(b",\"capabilities\":{\"tools\":{},\"resources\":{\"subscribe\":false,\"listChanged\":false}},\"serverInfo\":{\"name\":\"irix7-mcp\",\"version\":\"0.1.0\"}}}");
        }
        Some(b"ping") | Some(b"notifications/initialized") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{}}");
        }
        Some(b"tools/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"tools\":[{\"name\":\"status\",\"description\":\"Guest status\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"echo\",\"description\":\"Echo text\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"console\",\"description\":\"Write a line to the guest console\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"serial.read\",\"description\":\"Read the serial console ring\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"offset\":{\"type\":\"integer\"},\"length\":{\"type\":\"integer\"}}}},{\"name\":\"serial.write\",\"description\":\"Write a line to the guest console\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"text\":{\"type\":\"string\"}}}},{\"name\":\"system.reset\",\"description\":\"Restart the guest from the PROM\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"system.poweroff\",\"description\":\"Halt the guest\",\"inputSchema\":{\"type\":\"object\"}},{\"name\":\"shell.exec\",\"description\":\"Run a small guest shell command\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"command\":{\"type\":\"string\"}}}}]}}");
        }
        Some(b"resources/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"resources\":[{\"uri\":\"irix7://serial\",\"name\":\"serial\",\"mimeType\":\"text/plain\",\"description\":\"Kernel serial ring\"},{\"uri\":\"irix7://status\",\"name\":\"status\",\"mimeType\":\"text/plain\",\"description\":\"Machine status\"},{\"uri\":\"irix7://net\",\"name\":\"net\",\"mimeType\":\"text/plain\",\"description\":\"Network summary\"},{\"uri\":\"irix7://mem\",\"name\":\"mem\",\"mimeType\":\"text/plain\",\"description\":\"Frame allocator summary\"}]}}");
        }
        Some(b"resources/read") => {
            let uri = json_string(body, b"\"uri\"").unwrap_or(b"");
            let mut data = [0u8; 1024];
            let n: isize = if uri == b"irix7://serial" {
                let total = (services.console_total)();
                let offset = total.saturating_sub(1024);
                unsafe { (services.console_read)(offset, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://status" {
                unsafe { (services.info)(0, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://net" {
                unsafe { (services.info)(1, data.as_mut_ptr(), data.len()) }
            } else if uri == b"irix7://mem" {
                unsafe { (services.info)(2, data.as_mut_ptr(), data.len()) }
            } else {
                -1
            };
            if n < 0 {
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"error\":{\"code\":-32602,\"message\":\"unknown resource\"}}");
            } else {
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"result\":{\"contents\":[{\"uri\":");
                w.json_string(uri);
                w.push(b",\"mimeType\":\"text/plain\",\"text\":");
                w.json_string(&data[..n as usize]);
                w.push(b"}]}}");
            }
        }
        Some(b"resources/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"resources\":[{\"uri\":\"irix7://serial\",\"name\":\"serial\",\"description\":\"Guest serial ring\",\"mimeType\":\"text/plain\"}]}}");
        }
        Some(b"resources/templates/list") => {
            w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
            w.push(id);
            w.push(b",\"result\":{\"resourceTemplates\":[]}}");
        }
        Some(b"resources/read") => {
            let uri = json_string(body, b"\"uri\"").unwrap_or(b"");
            if uri != b"irix7://serial" {
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"error\":{\"code\":-32602,\"message\":\"unknown resource\"}}");
            } else {
                let total = (services.console_total)();
                let offset = total.saturating_sub(1024);
                let mut data = [0u8; 1024];
                let n = unsafe { (services.console_read)(offset, data.as_mut_ptr(), data.len()) };
                let n = if n > 0 { n as usize } else { 0 };
                w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                w.push(id);
                w.push(b",\"result\":{\"contents\":[{\"uri\":\"irix7://serial\",\"mimeType\":\"text/plain\",\"text\":");
                w.json_string(&data[..n]);
                w.push(b"}]}}");
            }
        }
        Some(b"tools/call") => {
            let name = json_string(body, b"\"name\"");
            let text = json_string(body, b"\"text\"").unwrap_or(b"");
            match name {
                Some(b"status") => {
                    let mut data = [0u8; 512];
                    let n = unsafe { (services.status)(data.as_mut_ptr(), data.len()) };
                    if n < 0 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"error\":{\"code\":-32603,\"message\":\"status failed\"}}");
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                        w.json_string(&data[..n as usize]);
                        w.push(b"}]}}");
                    }
                }
                Some(b"echo") => {
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                    w.json_string(text);
                    w.push(b"}]}}");
                }
                Some(b"console") | Some(b"serial.write") => {
                    (services.write)(text.as_ptr(), text.len());
                    (services.write)(b"\r\n".as_ptr(), 2);
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"ok\"}]}}");
                }
                Some(b"system.reset") => {
                    unsafe {
                        ACTION = 1;
                        COUNTDOWN = 5;
                    }
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(
                        b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"resetting\"}]}}",
                    );
                }
                Some(b"system.poweroff") => {
                    unsafe {
                        ACTION = 2;
                        COUNTDOWN = 5;
                    }
                    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                    w.push(id);
                    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"powering off\"}]}}");
                }
                Some(b"shell.exec") => {
                    let command = json_string(body, b"\"command\"").unwrap_or(b"");
                    shell_exec(services, command, &mut w, id);
                }
                Some(b"serial.read") => {
                    let total = (services.console_total)();
                    let offset =
                        json_u64(body, b"\"offset\"").unwrap_or(total.saturating_sub(2048));
                    let length = json_u64(body, b"\"length\"").unwrap_or(1024).min(1024) as usize;
                    let mut data = [0u8; 1024];
                    let n = unsafe { (services.console_read)(offset, data.as_mut_ptr(), length) };
                    if n < 0 {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(
                            b",\"error\":{\"code\":-32603,\"message\":\"console read failed\"}}",
                        );
                    } else {
                        w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
                        w.push(id);
                        w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
                        w.json_string(&data[..n as usize]);
                        w.push(b"}]}}");
                    }
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

fn info_block<'a>(services: &Services, kind: u32, buf: &'a mut [u8; 512]) -> &'a [u8] {
    let n = unsafe { (services.info)(kind, buf.as_mut_ptr(), buf.len()) };
    if n > 0 {
        &buf[..n as usize]
    } else {
        b"info failed"
    }
}

fn shell_exec(services: &Services, command: &[u8], w: &mut Out, id: &[u8]) {
    let mut parts = command.splitn(2, |&b| b == b' ');
    let cmd = parts.next().unwrap_or(b"");
    let mut arg = parts.next().unwrap_or(b"");
    while let Some((&b' ', rest)) = arg.split_first() {
        arg = rest;
    }

    let mut buf = [0u8; 512];
    let text: &[u8] = match cmd {
        b"uname" => b"IRIX 7 indy mips n32",
        b"echo" => arg,
        b"uptime" => {
            let mut n = copy(&mut buf, b"uptime: ");
            n += write_u64(&mut buf[n..], (services.clock_ms)() / 1000);
            n += copy(&mut buf[n..], b" s");
            &buf[..n]
        }
        b"status" => {
            let n = unsafe { (services.status)(buf.as_mut_ptr(), buf.len()) };
            if n > 0 {
                &buf[..n as usize]
            } else {
                b"status failed"
            }
        }
        b"net" => info_block(services, 1, &mut buf),
        b"mem" => info_block(services, 2, &mut buf),
        b"time" => info_block(services, 3, &mut buf),
        b"help" => b"commands: echo, uname, uptime, status, net, mem, help, reboot, poweroff",
        b"reboot" => {
            unsafe {
                ACTION = 1;
                COUNTDOWN = 5;
            }
            b"resetting"
        }
        b"poweroff" => {
            unsafe {
                ACTION = 2;
                COUNTDOWN = 5;
            }
            b"powering off"
        }
        _ => b"unknown command",
    };

    w.push(b"{\"jsonrpc\":\"2.0\",\"id\":");
    w.push(id);
    w.push(b",\"result\":{\"content\":[{\"type\":\"text\",\"text\":");
    w.json_string(text);
    w.push(b"}]}}");
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

fn json_u64(body: &[u8], key: &[u8]) -> Option<u64> {
    let mut p = find(body, key)? + key.len();
    while p < body.len() && (body[p] == b' ' || body[p] == b':') {
        p += 1;
    }
    let mut value = 0u64;
    let mut any = false;
    while p < body.len() && body[p].is_ascii_digit() {
        value = value
            .saturating_mul(10)
            .saturating_add((body[p] - b'0') as u64);
        p += 1;
        any = true;
    }
    if any {
        Some(value)
    } else {
        None
    }
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

fn copy(dst: &mut [u8], src: &[u8]) -> usize {
    let n = dst.len().min(src.len());
    dst[..n].copy_from_slice(&src[..n]);
    n
}

fn write_u64(dst: &mut [u8], mut value: u64) -> usize {
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
    let count = n.min(dst.len());
    for i in 0..count {
        dst[i] = digits[n - 1 - i];
    }
    count
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
