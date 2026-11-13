// SPDX-License-Identifier: GPL-3.0-only
//! The read-parse-execute loop.

use core::fmt::Write;

use crate::applets;
use crate::io::{parse_u32, perror, write_all, Out};
use crate::libc;
use crate::parse::{self, Lookup, Token};

/// Bytes of one input line.
const LINE_BYTES: usize = 1024;
/// Bytes of all the words of one line, terminators included.
const ARENA_BYTES: usize = 2048;
const MAX_TOKENS: usize = 96;
/// The kernel's `execve` copies at most 16 argument pointers.
const MAX_ARGS: usize = 15;
/// Commands in one pipeline.  The kernel runs eight local cells in all.
const MAX_STAGES: usize = 6;

const PROMPT: &[u8] = b"irix7$ ";

struct Shell {
    /// Exit status of the last pipeline, for `$?`.
    status: i32,
    pid: libc::pid_t,
    interactive: bool,
    /// Set by the `exit` builtin.
    exit: Option<i32>,
}

impl Lookup for Shell {
    fn expand(&mut self, name: &[u8], emit: &mut dyn FnMut(u8)) {
        let mut number = |value: u32| {
            let mut digits = [0u8; 10];
            let mut index = digits.len();
            let mut value = value;
            loop {
                index -= 1;
                digits[index] = b'0' + (value % 10) as u8;
                value /= 10;
                if value == 0 {
                    break;
                }
            }
            for &digit in &digits[index..] {
                emit(digit);
            }
        };
        match name {
            b"?" => number(self.status as u32),
            b"$" => number(self.pid as u32),
            _ => {
                let mut key = [0u8; 64];
                if name.is_empty() || name.len() >= key.len() {
                    return;
                }
                key[..name.len()].copy_from_slice(name);
                // SAFETY: `key` is NUL-terminated.
                let value = unsafe { libc::getenv(key.as_ptr()) };
                if !value.is_null() {
                    // SAFETY: getenv returns a NUL-terminated string.
                    for &byte in unsafe { libc::cstr(value) } {
                        emit(byte);
                    }
                }
            }
        }
    }
}

/// One command of a pipeline: its words and redirections.
#[derive(Clone, Copy)]
struct Command {
    argv: [*const u8; MAX_ARGS + 1],
    argc: usize,
    input: *const u8,
    output: *const u8,
    append: bool,
}

impl Command {
    const EMPTY: Self = Self {
        argv: [core::ptr::null(); MAX_ARGS + 1],
        argc: 0,
        input: core::ptr::null(),
        output: core::ptr::null(),
        append: false,
    };

    fn name(&self) -> &[u8] {
        // SAFETY: argv[0] points at a NUL-terminated word in the arena.
        unsafe { libc::cstr(self.argv[0]) }
    }

    fn redirected(&self) -> bool {
        !self.input.is_null() || !self.output.is_null()
    }
}

fn complain(message: &str) {
    let mut out = Out::new(2);
    let _ = writeln!(out, "sh: {message}");
}

/// Read one line from standard input.  `None` is end-of-file.
fn read_line(line: &mut [u8; LINE_BYTES]) -> Option<usize> {
    let mut length = 0;
    while length < line.len() {
        // SAFETY: the destination is inside `line`.
        let count = unsafe { libc::read(0, line.as_mut_ptr().add(length), line.len() - length) };
        if count < 0 {
            return None;
        }
        if count == 0 {
            // End-of-file ends a partial last line too.
            return if length == 0 { None } else { Some(length) };
        }
        length += count as usize;
        if line[length - 1] == b'\n' {
            return Some(length - 1);
        }
    }
    Some(length)
}

/// Decode a `waitpid` status into a shell exit status.
fn exit_status(status: i32) -> i32 {
    let signal = status & 0x7F;
    if signal == 0 {
        (status >> 8) & 0xFF
    } else {
        128 + signal
    }
}

fn signal_text(signal: i32) -> &'static str {
    match signal {
        libc::SIGHUP => "Hangup",
        libc::SIGINT => "Interrupt",
        libc::SIGQUIT => "Quit",
        libc::SIGKILL => "Killed",
        libc::SIGPIPE => "Broken pipe",
        libc::SIGTERM => "Terminated",
        _ => "Signal",
    }
}

impl Shell {
    /// Run a builtin in the shell process.  `None` means `command` is not one.
    fn builtin(&mut self, command: &Command) -> Option<i32> {
        let args = &command.argv[..command.argc];
        // SAFETY: every argument points at a NUL-terminated word.
        let arg = |index: usize| unsafe { libc::cstr(args[index]) };
        Some(match command.name() {
            b"cd" => {
                let target = if args.len() > 1 {
                    args[1]
                } else {
                    // SAFETY: the name is NUL-terminated.
                    let home = unsafe { libc::getenv(b"HOME\0".as_ptr()) };
                    if home.is_null() {
                        b"/\0".as_ptr()
                    } else {
                        home
                    }
                };
                // SAFETY: `target` is NUL-terminated.
                if unsafe { libc::chdir(target) } != 0 {
                    // SAFETY: as above.
                    perror("cd", unsafe { libc::cstr(target) });
                    1
                } else {
                    0
                }
            }
            b"exit" => {
                let code = if args.len() > 1 {
                    match parse_u32(arg(1)) {
                        Some(code) => (code & 0xFF) as i32,
                        None => {
                            complain("exit: the status must be a number");
                            return Some(2);
                        }
                    }
                } else {
                    self.status
                };
                self.exit = Some(code);
                code
            }
            b"export" => {
                let mut status = 0;
                for index in 1..args.len() {
                    let text = arg(index);
                    let Some(equals) = text.iter().position(|&byte| byte == b'=') else {
                        // `export NAME` marks an existing variable; every
                        // variable is already in the environment here.
                        continue;
                    };
                    let mut name = [0u8; 64];
                    if equals == 0 || equals >= name.len() {
                        complain("export: bad variable name");
                        status = 1;
                        continue;
                    }
                    name[..equals].copy_from_slice(&text[..equals]);
                    // SAFETY: `name` is NUL-terminated and the value is the
                    // NUL-terminated tail of the argument.
                    if unsafe { libc::setenv(name.as_ptr(), args[index].add(equals + 1), 1) } != 0 {
                        perror("export", &text[..equals]);
                        status = 1;
                    }
                }
                status
            }
            b"unset" => {
                for &name in &args[1..] {
                    // SAFETY: `name` is NUL-terminated.
                    unsafe { libc::unsetenv(name) };
                }
                0
            }
            b"help" => {
                let mut out = Out::new(1);
                out.bytes(b"Builtins:\n");
                out.bytes(b"  cd [DIR]             change the working directory\n");
                out.bytes(b"  exit [STATUS]        leave the shell\n");
                out.bytes(b"  export NAME=VALUE    set an environment variable\n");
                out.bytes(b"  unset NAME...        remove environment variables\n");
                out.bytes(b"  help                 print this list\n");
                out.bytes(b"Utilities:\n");
                applets::help(&mut out);
                out.bytes(b"Syntax: cmd | cmd, < file, > file, >> file, ;, &&, ||, 'quotes', $NAME, $?\n");
                0
            }
            _ => return None,
        })
    }

    /// Open the redirections of `command` onto descriptors 0 and 1.
    fn redirect(command: &Command) -> bool {
        if !command.input.is_null() {
            // SAFETY: the path is NUL-terminated.
            let fd = unsafe { libc::open(command.input, libc::O_RDONLY, 0) };
            if fd < 0 {
                // SAFETY: as above.
                perror("sh", unsafe { libc::cstr(command.input) });
                return false;
            }
            // SAFETY: `fd` is open.
            unsafe {
                libc::dup2(fd, 0);
                libc::close(fd);
            }
        }
        if !command.output.is_null() {
            let mode = if command.append {
                libc::O_APPEND
            } else {
                libc::O_TRUNC
            };
            // SAFETY: the path is NUL-terminated.
            let fd = unsafe {
                libc::open(
                    command.output,
                    libc::O_WRONLY | libc::O_CREAT | mode,
                    0o644,
                )
            };
            if fd < 0 {
                // SAFETY: as above.
                perror("sh", unsafe { libc::cstr(command.output) });
                return false;
            }
            // SAFETY: `fd` is open.
            unsafe {
                libc::dup2(fd, 1);
                libc::close(fd);
            }
        }
        true
    }

    /// Replace the child with an external program found on `PATH`.
    fn exec(command: &Command) -> ! {
        let name = command.name();
        let mut path = [0u8; 64];
        let try_exec = |path: *const u8| {
            // SAFETY: `path` and every argv entry are NUL-terminated and the
            // argv array ends with a null pointer.
            unsafe { libc::execve(path, command.argv.as_ptr(), libc::environ) };
        };
        if name.contains(&b'/') {
            try_exec(command.argv[0]);
        } else {
            // SAFETY: the name is NUL-terminated.
            let search = unsafe { libc::getenv(b"PATH\0".as_ptr()) };
            let search: &[u8] = if search.is_null() {
                b"/bin"
            } else {
                // SAFETY: getenv returns a NUL-terminated string.
                unsafe { libc::cstr(search) }
            };
            for directory in search.split(|&byte| byte == b':') {
                let directory: &[u8] = if directory.is_empty() { b"." } else { directory };
                let slash = !directory.ends_with(b"/") as usize;
                // The kernel accepts paths of up to 63 bytes.
                if directory.len() + slash + name.len() + 1 > path.len() {
                    continue;
                }
                path[..directory.len()].copy_from_slice(directory);
                path[directory.len()] = b'/';
                let start = directory.len() + slash;
                path[start..start + name.len()].copy_from_slice(name);
                path[start + name.len()] = 0;
                try_exec(path.as_ptr());
            }
        }
        let mut out = Out::new(2);
        out.bytes(b"sh: ");
        out.bytes(name);
        out.bytes(b": command not found\n");
        out.flush();
        // SAFETY: `_exit` does not return.
        unsafe { libc::_exit(127) }
    }

    /// The body of a forked child: never returns to the shell loop.
    fn child(&mut self, command: &Command) -> ! {
        // SAFETY: plain libc calls; the child restores the default action for
        // the signals the interactive shell ignores.
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
        }
        if !Self::redirect(command) {
            // SAFETY: `_exit` does not return.
            unsafe { libc::_exit(1) }
        }
        let status = if let Some(status) = self.builtin(command) {
            status
        } else if let Some(applet) = applets::find(command.name()) {
            applet(&command.argv[..command.argc])
        } else {
            Self::exec(command)
        };
        // SAFETY: `_exit` does not return.
        unsafe { libc::_exit(status) }
    }

    /// Run one pipeline and return its exit status.
    fn pipeline(&mut self, stages: &[Command]) -> i32 {
        // A lone builtin without redirection changes the shell itself.
        if stages.len() == 1 && !stages[0].redirected() {
            if let Some(status) = self.builtin(&stages[0]) {
                return status;
            }
        }

        let mut pids = [0 as libc::pid_t; MAX_STAGES];
        let mut group: libc::pid_t = 0;
        let mut input = -1;
        let mut started = 0;
        for (index, command) in stages.iter().enumerate() {
            let mut pipe = [-1i32; 2];
            let last = index + 1 == stages.len();
            // SAFETY: `pipe` has room for two descriptors.
            if !last && unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
                perror("sh", b"pipe");
                break;
            }
            // SAFETY: plain system call.
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                perror("sh", b"fork");
                // SAFETY: both descriptors are open when the pipe was made.
                unsafe {
                    if !last {
                        libc::close(pipe[0]);
                        libc::close(pipe[1]);
                    }
                }
                break;
            }
            if pid == 0 {
                // SAFETY: descriptor plumbing in the child before it runs the
                // command.  dup2 clears close-on-exec on the new descriptor.
                unsafe {
                    libc::setpgid(0, group);
                    if input >= 0 {
                        libc::dup2(input, 0);
                        libc::close(input);
                    }
                    if !last {
                        libc::dup2(pipe[1], 1);
                        libc::close(pipe[0]);
                        libc::close(pipe[1]);
                    }
                }
                self.child(command);
            }
            if group == 0 {
                group = pid;
            }
            // SAFETY: plain system calls.  The parent sets the group too, so
            // it exists before the terminal is handed over.
            unsafe {
                libc::setpgid(pid, group);
                if input >= 0 {
                    libc::close(input);
                }
                if !last {
                    libc::close(pipe[1]);
                }
            }
            input = pipe[0];
            pids[index] = pid;
            started += 1;
        }
        if input >= 0 {
            // SAFETY: the read end of an unfinished pipeline is still open.
            unsafe { libc::close(input) };
        }
        if started == 0 {
            return 1;
        }

        // The pipeline owns the terminal while it runs, so Ctrl-C reaches it
        // and not the shell.
        if self.interactive {
            // SAFETY: plain system call.
            unsafe { libc::tcsetpgrp(0, group) };
        }
        let mut status = 1;
        for &pid in &pids[..started] {
            let mut raw = 0;
            // SAFETY: `raw` is writable.
            if unsafe { libc::waitpid(pid, &mut raw, 0) } == pid {
                status = exit_status(raw);
                let signal = raw & 0x7F;
                if signal != 0 && signal != libc::SIGPIPE {
                    let mut out = Out::new(2);
                    let _ = writeln!(out, "{}", signal_text(signal));
                }
            }
        }
        if self.interactive {
            // SAFETY: plain system call.
            unsafe { libc::tcsetpgrp(0, self.pid) };
        }
        if started != stages.len() {
            return 1;
        }
        status
    }

    /// Parse and run one line.
    fn execute(&mut self, line: &[u8]) {
        let mut arena = [0u8; ARENA_BYTES];
        let mut tokens = [Token::Semi; MAX_TOKENS];
        let count = match parse::tokenize(line, &mut arena, &mut tokens, self) {
            Ok(count) => count,
            Err(error) => {
                complain(error.message());
                self.status = 2;
                return;
            }
        };
        let tokens = &tokens[..count];

        // Check the whole line before running any of it.
        let mut stages = [Command::EMPTY; MAX_STAGES];
        let mut position = 0;
        // Whether the next pipeline runs: `&&` and `||` look at `$?`.
        let mut run = true;
        while position < tokens.len() {
            let mut stage = 0usize;
            stages[0] = Command::EMPTY;
            let mut connector = Token::Semi;
            let mut ended = false;
            while position < tokens.len() && !ended {
                let token = tokens[position];
                position += 1;
                match token {
                    Token::Word(offset) => {
                        let command = &mut stages[stage];
                        if command.argc == MAX_ARGS {
                            complain("too many arguments");
                            self.status = 2;
                            return;
                        }
                        command.argv[command.argc] = arena[offset..].as_ptr();
                        command.argc += 1;
                    }
                    Token::Less | Token::Great | Token::Append => {
                        let Some(&Token::Word(offset)) = tokens.get(position) else {
                            complain("syntax error: a redirection needs a file name");
                            self.status = 2;
                            return;
                        };
                        position += 1;
                        let command = &mut stages[stage];
                        let path = arena[offset..].as_ptr();
                        if token == Token::Less {
                            command.input = path;
                        } else {
                            command.output = path;
                            command.append = token == Token::Append;
                        }
                    }
                    Token::Pipe => {
                        if stages[stage].argc == 0 || stage + 1 == MAX_STAGES {
                            complain(if stages[stage].argc == 0 {
                                "syntax error near |"
                            } else {
                                "too many commands in the pipeline"
                            });
                            self.status = 2;
                            return;
                        }
                        stage += 1;
                        stages[stage] = Command::EMPTY;
                    }
                    Token::Semi | Token::And | Token::Or => {
                        connector = token;
                        ended = true;
                    }
                }
            }
            if stages[stage].argc == 0 {
                // An empty command is fine after `;` and at the end of a
                // line, and a syntax error anywhere else.
                if stage != 0 || stages[stage].redirected() || connector != Token::Semi {
                    complain("syntax error: a command is missing");
                    self.status = 2;
                    return;
                }
                continue;
            }
            if run {
                self.status = self.pipeline(&stages[..=stage]);
                if self.exit.is_some() {
                    return;
                }
            }
            run = match connector {
                Token::And => self.status == 0,
                Token::Or => self.status != 0,
                _ => true,
            };
        }
    }
}

/// Run the shell.  `sh -c COMMAND` runs one command line and exits.
pub fn run(args: &[*const u8]) -> i32 {
    // SAFETY: every argument is a NUL-terminated string from crt0.
    let command = match args {
        [_, flag, command, ..] if unsafe { libc::cstr(*flag) } == b"-c" => {
            Some(unsafe { libc::cstr(*command) })
        }
        _ => None,
    };
    let mut shell = Shell {
        status: 0,
        // SAFETY: plain system calls.
        pid: unsafe { libc::getpid() },
        interactive: command.is_none() && unsafe { libc::isatty(0) } == 1,
        exit: None,
    };
    if let Some(command) = command {
        shell.execute(command);
        return shell.exit.unwrap_or(shell.status);
    }
    if shell.interactive {
        // SAFETY: plain libc calls.  The shell leads its own group and keeps
        // running when the user types the interrupt or quit character.
        unsafe {
            libc::setpgid(0, 0);
            libc::tcsetpgrp(0, shell.pid);
            libc::signal(libc::SIGINT, libc::SIG_IGN);
            libc::signal(libc::SIGQUIT, libc::SIG_IGN);
        }
        write_all(1, b"IRIX 7 sh: type help for the command list\n");
    }
    let mut line = [0u8; LINE_BYTES];
    loop {
        if shell.interactive {
            write_all(1, PROMPT);
        }
        let Some(length) = read_line(&mut line) else {
            if shell.interactive {
                write_all(1, b"exit\n");
            }
            return shell.status;
        };
        shell.execute(&line[..length]);
        if let Some(code) = shell.exit {
            return code;
        }
    }
}
