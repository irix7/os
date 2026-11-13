// SPDX-License-Identifier: GPL-3.0-only
//! Small utilities built into the shell image.
//!
//! The kernel can stage one extra executable, so the everyday commands live
//! here.  The shell runs an applet in a forked child, which makes pipes and
//! redirection work exactly as they do for an external program.

use core::fmt::Write;

use crate::io::{parse_u32, perror, write_all, Out};
use crate::libc;

/// Applet arguments: NUL-terminated strings, `argv[0]` first.
pub type Args<'a> = &'a [*const u8];

type Applet = fn(Args) -> i32;

const TABLE: &[(&str, Applet, &str)] = &[
    ("cat", cat, "cat [FILE]...        copy files or standard input to standard output"),
    ("echo", echo, "echo [-n] [WORD]...  print the words"),
    ("env", env, "env                  print the environment"),
    ("false", fail, "false                exit 1"),
    ("kill", kill, "kill [-SIGNAL] PID   send a signal (default TERM)"),
    ("ls", ls, "ls [-l] [PATH]...    list directories"),
    ("mkdir", mkdir, "mkdir DIR...         create directories"),
    ("mv", mv, "mv OLD NEW           rename a file or directory"),
    ("pwd", pwd, "pwd                  print the working directory"),
    ("rm", rm, "rm FILE...           remove files"),
    ("rmdir", rmdir, "rmdir DIR...         remove empty directories"),
    ("sleep", sleep, "sleep SECONDS        wait"),
    ("true", succeed, "true                 exit 0"),
    ("uname", uname, "uname [-a]           print the system name"),
    ("wc", wc, "wc [FILE]...         count lines, words and bytes"),
];

/// The applet called `name`, if there is one.
pub fn find(name: &[u8]) -> Option<Applet> {
    TABLE
        .iter()
        .find(|(applet, _, _)| applet.as_bytes() == name)
        .map(|&(_, run, _)| run)
}

/// Print one usage line per applet.
pub fn help(out: &mut Out) {
    for (_, _, usage) in TABLE {
        let _ = writeln!(out, "  {usage}");
    }
}

fn arg<'a>(pointer: *const u8) -> &'a [u8] {
    // SAFETY: every argument is a NUL-terminated string owned by the shell.
    unsafe { libc::cstr(pointer) }
}

fn succeed(_args: Args) -> i32 {
    0
}

fn fail(_args: Args) -> i32 {
    1
}

fn echo(args: Args) -> i32 {
    let mut out = Out::new(1);
    let mut words = &args[1..];
    let newline = !matches!(words.first(), Some(&first) if arg(first) == b"-n");
    if !newline {
        words = &words[1..];
    }
    for (index, &word) in words.iter().enumerate() {
        if index != 0 {
            out.bytes(b" ");
        }
        out.bytes(arg(word));
    }
    if newline {
        out.bytes(b"\n");
    }
    out.flush();
    out.failed as i32
}

/// Copy `fd` to standard output until end-of-file.
fn copy(fd: i32) -> bool {
    let mut buffer = [0u8; 512];
    loop {
        // SAFETY: the buffer is valid for its length.
        let count = unsafe { libc::read(fd, buffer.as_mut_ptr(), buffer.len()) };
        if count < 0 {
            return false;
        }
        if count == 0 {
            return true;
        }
        if !write_all(1, &buffer[..count as usize]) {
            return false;
        }
    }
}

fn cat(args: Args) -> i32 {
    if args.len() == 1 {
        return !copy(0) as i32;
    }
    let mut status = 0;
    for &path in &args[1..] {
        // SAFETY: `path` is NUL-terminated.
        let fd = unsafe { libc::open(path, libc::O_RDONLY) };
        if fd < 0 {
            perror("cat", arg(path));
            status = 1;
            continue;
        }
        if !copy(fd) {
            perror("cat", arg(path));
            status = 1;
        }
        // SAFETY: `fd` is open.
        unsafe { libc::close(fd) };
    }
    status
}

fn env(_args: Args) -> i32 {
    let mut out = Out::new(1);
    // SAFETY: `environ` is a NULL-terminated array of NUL-terminated strings.
    unsafe {
        let mut entry = libc::environ;
        while !entry.is_null() && !(*entry).is_null() {
            out.bytes(libc::cstr(*entry));
            out.bytes(b"\n");
            entry = entry.add(1);
        }
    }
    0
}

fn signal_number(name: &[u8]) -> Option<i32> {
    if let Some(number) = parse_u32(name) {
        return Some(number as i32);
    }
    let name = name.strip_prefix(b"SIG").unwrap_or(name);
    Some(match name {
        b"HUP" => libc::SIGHUP,
        b"INT" => libc::SIGINT,
        b"QUIT" => libc::SIGQUIT,
        b"KILL" => libc::SIGKILL,
        b"PIPE" => libc::SIGPIPE,
        b"TERM" => libc::SIGTERM,
        _ => return None,
    })
}

fn kill(args: Args) -> i32 {
    let mut signal = libc::SIGTERM;
    let mut targets = &args[1..];
    if let Some(&first) = targets.first() {
        if let Some(name) = arg(first).strip_prefix(b"-") {
            match signal_number(name) {
                Some(number) => signal = number,
                None => {
                    write_all(2, b"kill: unknown signal\n");
                    return 2;
                }
            }
            targets = &targets[1..];
        }
    }
    if targets.is_empty() {
        write_all(2, b"usage: kill [-SIGNAL] PID\n");
        return 2;
    }
    let mut status = 0;
    for &target in targets {
        let Some(pid) = parse_u32(arg(target)) else {
            write_all(2, b"kill: PID must be a number\n");
            status = 2;
            continue;
        };
        // SAFETY: plain system call.
        if unsafe { libc::kill(pid as i32, signal) } != 0 {
            perror("kill", arg(target));
            status = 1;
        }
    }
    status
}

fn mode_text(mode: u32) -> [u8; 10] {
    let mut text = *b"----------";
    text[0] = match mode & libc::S_IFMT {
        libc::S_IFDIR => b'd',
        libc::S_IFCHR => b'c',
        _ => b'-',
    };
    for (index, flag) in b"rwxrwxrwx".iter().enumerate() {
        if mode & (0o400 >> index) != 0 {
            text[index + 1] = *flag;
        }
    }
    text
}

/// Print one `ls` entry.  `path` is the NUL-terminated path to stat.
fn list_entry(out: &mut Out, long: bool, path: *const u8, name: &[u8]) -> bool {
    if long {
        // SAFETY: an all-zero `stat` is a valid value for the out-parameter.
        let mut info: libc::stat = unsafe { core::mem::zeroed() };
        // SAFETY: `path` is NUL-terminated and `info` is writable.
        if unsafe { libc::stat(path, &mut info) } != 0 {
            return false;
        }
        out.bytes(&mode_text(info.st_mode));
        let _ = write!(out, " {:>8} ", info.st_size);
    }
    out.bytes(name);
    out.bytes(b"\n");
    true
}

fn list(out: &mut Out, long: bool, path: *const u8) -> bool {
    // SAFETY: an all-zero `stat` is a valid value for the out-parameter.
    let mut info: libc::stat = unsafe { core::mem::zeroed() };
    // SAFETY: `path` is NUL-terminated and `info` is writable.
    if unsafe { libc::stat(path, &mut info) } != 0 {
        return false;
    }
    if info.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return list_entry(out, long, path, arg(path));
    }
    // SAFETY: `path` is NUL-terminated.
    let dir = unsafe { libc::opendir(path) };
    if dir.is_null() {
        return false;
    }
    let base = arg(path);
    loop {
        // SAFETY: `dir` is an open stream.
        let entry = unsafe { libc::readdir(dir) };
        if entry.is_null() {
            break;
        }
        // SAFETY: readdir returned a valid record with a NUL-terminated name.
        let name = unsafe { libc::cstr((*entry).d_name.as_ptr()) };
        if name == b"." || name == b".." {
            continue;
        }
        // Build "<base>/<name>\0" for stat.
        let mut full = [0u8; 320];
        let needed = base.len() + 1 + name.len() + 1;
        if needed > full.len() {
            continue;
        }
        full[..base.len()].copy_from_slice(base);
        full[base.len()] = b'/';
        full[base.len() + 1..base.len() + 1 + name.len()].copy_from_slice(name);
        if !list_entry(out, long, full.as_ptr(), name) {
            out.bytes(name);
            out.bytes(b"\n");
        }
    }
    // SAFETY: `dir` is an open stream.
    unsafe { libc::closedir(dir) };
    true
}

fn ls(args: Args) -> i32 {
    let mut out = Out::new(1);
    let mut paths = &args[1..];
    let long = matches!(paths.first(), Some(&first) if arg(first) == b"-l");
    if long {
        paths = &paths[1..];
    }
    if paths.is_empty() {
        if !list(&mut out, long, b".\0".as_ptr()) {
            out.flush();
            perror("ls", b".");
            return 1;
        }
        return 0;
    }
    let mut status = 0;
    for &path in paths {
        if !list(&mut out, long, path) {
            out.flush();
            perror("ls", arg(path));
            status = 1;
        }
    }
    status
}

/// Apply `operation` to every operand and report failures.
fn each(args: Args, program: &str, operation: impl Fn(*const u8) -> i32) -> i32 {
    if args.len() < 2 {
        let mut out = Out::new(2);
        let _ = writeln!(out, "usage: {program} NAME...");
        return 2;
    }
    let mut status = 0;
    for &path in &args[1..] {
        if operation(path) != 0 {
            perror(program, arg(path));
            status = 1;
        }
    }
    status
}

fn mkdir(args: Args) -> i32 {
    // SAFETY: each path is NUL-terminated.
    each(args, "mkdir", |path| unsafe { libc::mkdir(path, 0o755) })
}

fn rmdir(args: Args) -> i32 {
    // SAFETY: each path is NUL-terminated.
    each(args, "rmdir", |path| unsafe { libc::rmdir(path) })
}

fn rm(args: Args) -> i32 {
    // SAFETY: each path is NUL-terminated.
    each(args, "rm", |path| unsafe { libc::unlink(path) })
}

fn mv(args: Args) -> i32 {
    if args.len() != 3 {
        write_all(2, b"usage: mv OLD NEW\n");
        return 2;
    }
    // SAFETY: both paths are NUL-terminated.
    if unsafe { libc::rename(args[1], args[2]) } != 0 {
        perror("mv", arg(args[1]));
        return 1;
    }
    0
}

fn pwd(_args: Args) -> i32 {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its length.
    if unsafe { libc::getcwd(buffer.as_mut_ptr(), buffer.len()) }.is_null() {
        perror("pwd", b".");
        return 1;
    }
    let mut out = Out::new(1);
    // SAFETY: getcwd NUL-terminated the buffer.
    out.bytes(unsafe { libc::cstr(buffer.as_ptr()) });
    out.bytes(b"\n");
    0
}

fn sleep(args: Args) -> i32 {
    let Some(seconds) = args.get(1).and_then(|&text| parse_u32(arg(text))) else {
        write_all(2, b"usage: sleep SECONDS\n");
        return 2;
    };
    // SAFETY: plain libc call.
    unsafe { libc::sleep(seconds) };
    0
}

fn uname(args: Args) -> i32 {
    // SAFETY: an all-zero `utsname` is a valid value for the out-parameter.
    let mut info: libc::utsname = unsafe { core::mem::zeroed() };
    // SAFETY: `info` is writable.
    if unsafe { libc::uname(&mut info) } != 0 {
        perror("uname", b"uname");
        return 1;
    }
    let all = matches!(args.get(1), Some(&flag) if arg(flag) == b"-a");
    let mut out = Out::new(1);
    // SAFETY: uname NUL-terminated every field.
    unsafe {
        out.bytes(libc::cstr(info.sysname.as_ptr()));
        if all {
            for field in [&info.nodename, &info.release, &info.version, &info.machine] {
                out.bytes(b" ");
                out.bytes(libc::cstr(field.as_ptr()));
            }
        }
    }
    out.bytes(b"\n");
    0
}

fn count(fd: i32, out: &mut Out, name: &[u8]) -> bool {
    let (mut lines, mut words, mut bytes) = (0u32, 0u32, 0u32);
    let mut in_word = false;
    let mut buffer = [0u8; 512];
    loop {
        // SAFETY: the buffer is valid for its length.
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr(), buffer.len()) };
        if read < 0 {
            return false;
        }
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read as usize] {
            bytes += 1;
            if byte == b'\n' {
                lines += 1;
            }
            let space = byte.is_ascii_whitespace();
            if !space && !in_word {
                words += 1;
            }
            in_word = !space;
        }
    }
    let _ = write!(out, "{lines:>7} {words:>7} {bytes:>7}");
    if !name.is_empty() {
        out.bytes(b" ");
        out.bytes(name);
    }
    out.bytes(b"\n");
    true
}

fn wc(args: Args) -> i32 {
    let mut out = Out::new(1);
    if args.len() == 1 {
        return !count(0, &mut out, b"") as i32;
    }
    let mut status = 0;
    for &path in &args[1..] {
        // SAFETY: `path` is NUL-terminated.
        let fd = unsafe { libc::open(path, libc::O_RDONLY) };
        if fd < 0 || !count(fd, &mut out, arg(path)) {
            out.flush();
            perror("wc", arg(path));
            status = 1;
        }
        if fd >= 0 {
            // SAFETY: `fd` is open.
            unsafe { libc::close(fd) };
        }
    }
    status
}
