// SPDX-License-Identifier: GPL-3.0-only
//! The IRIX 7 standalone shell (sash).
//!
//! Sash runs in the kernel over the local keyboard/display and the telnet
//! console.  It deliberately has no userland or process dependency: the
//! recovery commands use the bring-up fd table, NVRAM, status block and the
//! boot request flags.  The command vocabulary follows SGI's standalone shell
//! (`help`, `ls`, `cat`, `pwd`, `cd`, `printenv`, `version`, `hinv`, `boot` and
//! `reboot`), but the implementation is original Rust.

use core::fmt::{self, Write};

use crate::fd::{self, Kind};

const INDEX_HOST: &str = "raw.githubusercontent.com";
const INDEX_PATH: &str = "/irix7/os/main/packages/index.conf";

/// Where sash writes its output.
pub trait Sink {
    /// Write bytes to the console.
    fn write(&mut self, bytes: &[u8]);
}

struct Writer<'a> {
    out: &'a mut dyn Sink,
}

impl Write for Writer<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.out.write(text.as_bytes());
        Ok(())
    }
}

/// A network command that must run outside the telnet socket borrow.
#[derive(Clone, Copy, Debug)]
pub enum Deferred {
    /// Fetch and list the package index.
    Packages,
    /// Install a package by name.
    Install {
        /// Name copied from the command line.
        name: [u8; 32],
        /// Valid bytes in `name`.
        len: u8,
    },
    /// Mount or inspect an EROFS image over HTTPS.
    Mount {
        /// Command argument copied from the line.
        argument: [u8; 112],
        /// Valid bytes in `argument`.
        len: u8,
    },
}

/// Result of parsing one sash line.
pub enum Outcome {
    /// Sash handled it and wrote any output.
    Handled,
    /// Not a sash command; let the telnet console try kernel-only commands.
    Unknown,
    /// A network operation that must be deferred.
    Deferred(Deferred),
}

static mut PACKAGE_INDEX: [u8; 4096] = [0; 4096];
static mut RESULT: [u8; 4096] = [0; 4096];
static mut RESULT_LEN: usize = 0;

struct ResultSink;

impl Sink for ResultSink {
    fn write(&mut self, bytes: &[u8]) {
        // SAFETY: sash is single-threaded and a deferred command completes
        // before the next one starts.
        unsafe {
            let space = RESULT.len().saturating_sub(RESULT_LEN);
            let count = bytes.len().min(space);
            RESULT[RESULT_LEN..RESULT_LEN + count].copy_from_slice(&bytes[..count]);
            RESULT_LEN += count;
        }
    }
}

/// Run one command.  Returns `Unknown` when the command is not a sash command,
/// so the telnet console can fall through to its kernel-only commands.
pub fn execute(line: &[u8], out: &mut dyn Sink) -> Outcome {
    let line = trim(line);
    let (command, argument) = split(line);
    let mut lower = [0u8; 16];
    let command = if command.len() <= lower.len() {
        for (index, byte) in command.iter().enumerate() {
            lower[index] = byte.to_ascii_lowercase();
        }
        &lower[..command.len()]
    } else {
        command
    };
    match command {
        b"" => Outcome::Handled,
        b"help" | b"?" => {
            help(out);
            Outcome::Handled
        }
        b"version" => {
            out.write(b"IRIX 7 sash 0.1 (Rust, no userland)\r\n");
            Outcome::Handled
        }
        b"pwd" => {
            let mut path = [0u8; 64];
            let len = fd::copy_cwd(&mut path);
            out.write(&path[..len]);
            out.write(b"\r\n");
            Outcome::Handled
        }
        b"cd" => {
            if argument.is_empty() {
                out.write(b"cd: missing directory\r\n");
            } else if fd::set_cwd(argument) {
                // cd is silent on success, as in sash.
            } else {
                out.write(b"cd: no such directory\r\n");
            }
            Outcome::Handled
        }
        b"ls" => {
            list(out, argument);
            Outcome::Handled
        }
        b"cat" => {
            cat(out, argument);
            Outcome::Handled
        }
        b"printenv" => {
            print_env(out, argument);
            Outcome::Handled
        }
        b"echo" => {
            out.write(argument);
            out.write(b"\r\n");
            Outcome::Handled
        }
        b"status" => {
            let mut buffer = [0u8; 512];
            let len = crate::status::text(&mut buffer);
            out.write(&buffer[..len]);
            Outcome::Handled
        }
        b"net" => {
            let mut buffer = [0u8; 256];
            let len = crate::status::net(&mut buffer);
            out.write(&buffer[..len]);
            Outcome::Handled
        }
        b"mem" => {
            let mut buffer = [0u8; 256];
            let len = crate::status::mem(&mut buffer);
            out.write(&buffer[..len]);
            Outcome::Handled
        }
        b"time" => {
            let mut buffer = [0u8; 128];
            let len = crate::status::time(&mut buffer);
            out.write(&buffer[..len]);
            Outcome::Handled
        }
        b"hinv" | b"inventory" => {
            // diagnostics prints through the global console, which mirrors
            // both the local screen and a connected telnet client.
            crate::diagnostics::run();
            Outcome::Handled
        }
        b"boot" => {
            out.write(b"boot: retrying the target-2 user image\r\n");
            crate::telnet::request_boot();
            Outcome::Handled
        }
        b"reboot" | b"reset" => {
            out.write(b"reboot: restarting from the PROM\r\n");
            crate::telnet::request_reboot();
            Outcome::Handled
        }
        b"packages" | b"modules" => {
            let argument = trim(argument);
            if argument.is_empty() {
                Outcome::Deferred(Deferred::Packages)
            } else if let Some(name) = argument
                .strip_prefix(b"install ")
                .or_else(|| argument.strip_prefix(b"load "))
            {
                let name = trim(name);
                if name.is_empty() || name.len() > 32 || !name.is_ascii() {
                    out.write(b"packages: invalid name\r\n");
                    return Outcome::Handled;
                }
                let mut buffer = [0u8; 32];
                buffer[..name.len()].copy_from_slice(name);
                Outcome::Deferred(Deferred::Install {
                    name: buffer,
                    len: name.len() as u8,
                })
            } else {
                out.write(b"usage: packages [install NAME]\r\n");
                Outcome::Handled
            }
        }
        b"mount" => {
            #[cfg(feature = "erofs-debug")]
            {
                if argument.is_empty() || argument.len() > 112 {
                    out.write(b"usage: mount https://host:port/image [sha256=..] [info|ls|cat] [path]\r\n");
                    return Outcome::Handled;
                }
                let mut buffer = [0u8; 112];
                buffer[..argument.len()].copy_from_slice(argument);
                Outcome::Deferred(Deferred::Mount {
                    argument: buffer,
                    len: argument.len() as u8,
                })
            }
            #[cfg(not(feature = "erofs-debug"))]
            {
                out.write(b"mount: kernel built without remote EROFS support\r\n");
                Outcome::Handled
            }
        }
        _ => Outcome::Unknown,
    }
}

/// Execute a deferred network command.  The caller must not hold a socket-set
/// borrow; telnet runs it from `poll`, local sash runs it from the idle loop.
pub fn run_deferred(request: Deferred) -> &'static [u8] {
    unsafe {
        RESULT_LEN = 0;
    }
    let mut sink = ResultSink;
    match request {
        Deferred::Packages => packages(&mut sink),
        Deferred::Install { name, len } => install(&mut sink, &name[..len as usize]),
        Deferred::Mount { argument, len } => mount(&mut sink, &argument[..len as usize]),
    }
    unsafe { &RESULT[..RESULT_LEN] }
}

fn packages(out: &mut dyn Sink) {
    let Some(index) = fetch_index() else {
        out.write(b"packages: index fetch failed\r\n");
        return;
    };
    let mut writer = Writer { out };
    for line in index.split(|byte| *byte == b'\n') {
        let line = trim(line);
        if line.is_empty() || line[0] == b'#' || line.starts_with(b"version=") {
            continue;
        }
        if line.starts_with(b"module ") || line.starts_with(b"userland ") {
            let _ = writer.write_str(core::str::from_utf8(line).unwrap_or("?"));
            let _ = writer.write_str("\r\n");
        }
    }
}

fn install(out: &mut dyn Sink, name: &[u8]) {
    let Some(index) = fetch_index() else {
        out.write(b"packages: index fetch failed\r\n");
        return;
    };
    for line in index.split(|byte| *byte == b'\n') {
        let line = trim(line);
        let mut fields = line.split(|byte| *byte == b' ');
        let Some(kind) = fields.next() else { continue };
        let Some(record_name) = fields.next() else {
            continue;
        };
        if kind != b"module" && kind != b"userland" || record_name != name {
            continue;
        }
        let Some(_version) = fields.next() else {
            continue;
        };
        let Some(url) = fields.next() else { continue };
        let Some(digest_text) = fields.next() else {
            continue;
        };
        let Ok(url) = core::str::from_utf8(url) else {
            out.write(b"packages: URL is not UTF-8\r\n");
            return;
        };
        let Some(digest) =
            crate::telnet::parse_sha256_hex(core::str::from_utf8(digest_text).unwrap_or(""))
        else {
            out.write(b"packages: bad sha256\r\n");
            return;
        };
        if kind == b"userland" {
            out.write(b"packages: userland install is not implemented\r\n");
            return;
        }
        match crate::autoload::load_url(url, &crate::sha256::Sha256(digest)) {
            Ok(0) => out.write(b"packages: module loaded\r\n"),
            Ok(code) => {
                let _ = write!(
                    ResultSinkWriter { out },
                    "packages: module status {code}\r\n"
                );
            }
            Err(error) => {
                let _ = write!(ResultSinkWriter { out }, "packages: {error}\r\n");
            }
        }
        return;
    }
    out.write(b"packages: not found\r\n");
}

struct ResultSinkWriter<'a> {
    out: &'a mut dyn Sink,
}

impl Write for ResultSinkWriter<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.out.write(text.as_bytes());
        Ok(())
    }
}

fn fetch_index() -> Option<&'static [u8]> {
    let ip = crate::telnet::resolve_host(INDEX_HOST.as_bytes())?;
    let buffer = unsafe { &mut *core::ptr::addr_of_mut!(PACKAGE_INDEX) };
    let (summary, captured) = crate::tls::get_to(ip, 443, INDEX_HOST, INDEX_PATH, buffer).ok()?;
    if summary.status != 200 || captured == 0 {
        return None;
    }
    Some(unsafe { &PACKAGE_INDEX[..captured] })
}

fn mount(out: &mut dyn Sink, argument: &[u8]) {
    #[cfg(feature = "erofs-debug")]
    {
        let Ok(text) = core::str::from_utf8(argument) else {
            out.write(b"mount: argument is not UTF-8\r\n");
            return;
        };
        let Some(request) = crate::erofs_cmd::Request::parse(text.as_bytes()) else {
            out.write(
                b"mount: usage: mount https://host:port/image [sha256=..] [info|ls|cat] [path]\r\n",
            );
            return;
        };
        let output = crate::erofs_cmd::run(&request);
        out.write(output.as_bytes());
    }
    #[cfg(not(feature = "erofs-debug"))]
    {
        let _ = argument;
        out.write(b"mount: kernel built without remote EROFS support\r\n");
    }
}

fn list(out: &mut dyn Sink, path: &[u8]) {
    let path = if path.is_empty() {
        b".".as_slice()
    } else {
        path
    };
    let fd = fd::open_path(path, 0);
    if fd < 0 {
        out.write(b"ls: no such file or directory\r\n");
        return;
    }
    let mut writer = Writer { out };
    if matches!(fd::kind(fd), Kind::Dir(_)) {
        let mut name = [0u8; 256];
        let mut index = 0usize;
        while let Some((kind, _)) = fd::readdir(fd, index, &mut name) {
            let len = name.iter().position(|byte| *byte == 0).unwrap_or(0);
            if &name[..len] != b"." && &name[..len] != b".." {
                let _ = writer.write_str(core::str::from_utf8(&name[..len]).unwrap_or("?"));
                if kind == 4 {
                    let _ = writer.write_str("/");
                }
                let _ = writer.write_str("\r\n");
            }
            index += 1;
        }
    } else if let Some((mode, size)) = fd::metadata(fd) {
        let _ = writer.write_str("file mode=0o");
        let _ = write!(writer, "{:o} size={}\r\n", mode, size);
    }
    let _ = fd::close(fd);
}

fn cat(out: &mut dyn Sink, path: &[u8]) {
    if path.is_empty() {
        out.write(b"cat: missing file\r\n");
        return;
    }
    let fd = fd::open_path(path, 0);
    if fd < 0 {
        out.write(b"cat: no such file\r\n");
        return;
    }
    let mut buffer = [0u8; 512];
    loop {
        let count = fd::read(fd, &mut buffer);
        if count < 0 {
            out.write(b"cat: read error\r\n");
            break;
        }
        if count == 0 {
            break;
        }
        out.write(&buffer[..count as usize]);
    }
    let _ = fd::close(fd);
}

fn print_env(out: &mut dyn Sink, name: &[u8]) {
    if name.is_empty() {
        out.write(b"printenv: name required\r\n");
        return;
    }
    let Ok(name) = core::str::from_utf8(name) else {
        out.write(b"printenv: name is not UTF-8\r\n");
        return;
    };
    let mut value = [0u8; 256];
    match crate::nvram::get(name, &mut value) {
        Some(len) => {
            out.write(&value[..len]);
            out.write(b"\r\n");
        }
        None => out.write(b"printenv: variable not set\r\n"),
    }
}

fn help(out: &mut dyn Sink) {
    out.write(
        b"sash commands:\r\n\
          \x20 help                 this text\r\n\
          \x20 version              sash and kernel identity\r\n\
          \x20 pwd                  print the current directory\r\n\
          \x20 cd DIRECTORY         change directory\r\n\
          \x20 ls [DIRECTORY]       list the bring-up namespace\r\n\
          \x20 cat FILE             write a file to the console\r\n\
          \x20 printenv NAME        print a PROM environment value\r\n\
          \x20 echo TEXT            echo the argument\r\n\
          \x20 status               kernel and network state\r\n\
          \x20 net                  interface addresses\r\n\
          \x20 mem                  RAM accounting\r\n\
          \x20 time                 uptime and RTC time\r\n\
          \x20 hinv                 CPU, memory and network inventory\r\n\
          \x20 packages             list the GitHub package index\r\n\
          \x20 packages install N   verify and load a module\r\n\
          \x20 mount URL [...]      inspect EROFS over HTTPS Range\r\n\
          \x20 boot                 retry the target-2 user image\r\n\
          \x20 reboot               restart from the PROM\r\n\
          \r\n\
          kernel-only (telnet): diagnostics, scsi, http, https, ntp, ping,\r\n\
          module, user, userelf, panic, trap, tlb and gfx.\r\n",
    );
}

/// Remove leading and trailing spaces and tabs.
pub fn trim(mut line: &[u8]) -> &[u8] {
    while let Some((&byte, rest)) = line.split_first() {
        if byte == b' ' || byte == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    while let Some((&byte, rest)) = line.split_last() {
        if byte == b' ' || byte == b'\t' {
            line = rest;
        } else {
            break;
        }
    }
    line
}

fn split(line: &[u8]) -> (&[u8], &[u8]) {
    let Some(index) = line.iter().position(|byte| *byte == b' ' || *byte == b'\t') else {
        return (line, b"");
    };
    (trim(&line[..index]), trim(&line[index..]))
}
