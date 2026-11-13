// SPDX-License-Identifier: GPL-3.0-only
//! Command-line tokeniser.
//!
//! One line becomes a list of tokens.  Words are copied into a caller-supplied
//! arena as NUL-terminated strings so they can be handed to `execve` without
//! another copy.  Quoting follows the POSIX shell: single quotes are literal,
//! double quotes allow `$` expansion and `\` before `"`, `\`, `$`; outside
//! quotes `\` escapes one byte.  `$NAME`, `${NAME}`, `$?` and `$$` expand
//! through the caller's lookup; an expansion is not split into fields.

/// One lexical token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// A word; the offset of its NUL-terminated bytes in the arena.
    Word(usize),
    /// `|`
    Pipe,
    /// `<`
    Less,
    /// `>`
    Great,
    /// `>>`
    Append,
    /// `;`
    Semi,
    /// `&&`
    And,
    /// `||`
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A quote was opened and not closed.
    UnterminatedQuote,
    /// The words do not fit the arena.
    LineTooLong,
    /// More tokens than the caller's table holds.
    TooManyTokens,
    /// `&` alone: background jobs are not implemented.
    Background,
}

impl Error {
    pub fn message(self) -> &'static str {
        match self {
            Error::UnterminatedQuote => "unterminated quote",
            Error::LineTooLong => "line too long",
            Error::TooManyTokens => "too many words",
            Error::Background => "background jobs are not supported",
        }
    }
}

/// What `$name` expands to.  The callback writes the value through `emit`.
pub trait Lookup {
    fn expand(&mut self, name: &[u8], emit: &mut dyn FnMut(u8));
}

struct Arena<'a> {
    bytes: &'a mut [u8],
    len: usize,
    overflow: bool,
}

impl Arena<'_> {
    fn push(&mut self, byte: u8) {
        if self.len < self.bytes.len() {
            self.bytes[self.len] = byte;
            self.len += 1;
        } else {
            self.overflow = true;
        }
    }
}

fn name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// Expand the `$` at `line[at]`; returns the index after the reference.
fn expand(line: &[u8], at: usize, arena: &mut Arena, lookup: &mut dyn Lookup) -> usize {
    let rest = &line[at + 1..];
    let (name, used): (&[u8], usize) = match rest.first() {
        Some(b'?') | Some(b'$') => (&rest[..1], 1),
        Some(b'{') => match rest.iter().position(|&byte| byte == b'}') {
            Some(close) => (&rest[1..close], close + 1),
            None => (&[], 0),
        },
        Some(&byte) if name_byte(byte) => {
            let end = rest
                .iter()
                .position(|&byte| !name_byte(byte))
                .unwrap_or(rest.len());
            (&rest[..end], end)
        }
        _ => (&[], 0),
    };
    if used == 0 {
        // A lone `$` is literal.
        arena.push(b'$');
        return at + 1;
    }
    lookup.expand(name, &mut |byte| arena.push(byte));
    at + 1 + used
}

/// Tokenise `line` into `tokens`, storing words in `arena`.
///
/// Returns the token count.  A `#` at the start of a word begins a comment.
pub fn tokenize(
    line: &[u8],
    arena: &mut [u8],
    tokens: &mut [Token],
    lookup: &mut dyn Lookup,
) -> Result<usize, Error> {
    let mut arena = Arena {
        bytes: arena,
        len: 0,
        overflow: false,
    };
    let mut count = 0usize;
    let mut index = 0usize;
    let mut emit = |token: Token, count: &mut usize| -> Result<(), Error> {
        if *count == tokens.len() {
            return Err(Error::TooManyTokens);
        }
        tokens[*count] = token;
        *count += 1;
        Ok(())
    };

    while index < line.len() {
        let byte = line[index];
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' => index += 1,
            b'#' => break,
            b';' => {
                emit(Token::Semi, &mut count)?;
                index += 1;
            }
            b'|' => {
                if line.get(index + 1) == Some(&b'|') {
                    emit(Token::Or, &mut count)?;
                    index += 2;
                } else {
                    emit(Token::Pipe, &mut count)?;
                    index += 1;
                }
            }
            b'&' => {
                if line.get(index + 1) == Some(&b'&') {
                    emit(Token::And, &mut count)?;
                    index += 2;
                } else {
                    return Err(Error::Background);
                }
            }
            b'<' => {
                emit(Token::Less, &mut count)?;
                index += 1;
            }
            b'>' => {
                if line.get(index + 1) == Some(&b'>') {
                    emit(Token::Append, &mut count)?;
                    index += 2;
                } else {
                    emit(Token::Great, &mut count)?;
                    index += 1;
                }
            }
            _ => {
                let start = arena.len;
                while index < line.len() {
                    let byte = line[index];
                    match byte {
                        b' ' | b'\t' | b'\r' | b'\n' | b';' | b'|' | b'&' | b'<' | b'>' => break,
                        b'\'' => {
                            index += 1;
                            loop {
                                match line.get(index) {
                                    None => return Err(Error::UnterminatedQuote),
                                    Some(b'\'') => break,
                                    Some(&byte) => arena.push(byte),
                                }
                                index += 1;
                            }
                            index += 1;
                        }
                        b'"' => {
                            index += 1;
                            loop {
                                match line.get(index) {
                                    None => return Err(Error::UnterminatedQuote),
                                    Some(b'"') => break,
                                    Some(b'\\')
                                        if matches!(
                                            line.get(index + 1),
                                            Some(b'"') | Some(b'\\') | Some(b'$')
                                        ) =>
                                    {
                                        arena.push(line[index + 1]);
                                        index += 2;
                                    }
                                    Some(b'$') => {
                                        index = expand(line, index, &mut arena, lookup);
                                    }
                                    Some(&byte) => {
                                        arena.push(byte);
                                        index += 1;
                                    }
                                }
                            }
                            index += 1;
                        }
                        b'\\' => {
                            if let Some(&next) = line.get(index + 1) {
                                arena.push(next);
                            }
                            index += 2;
                        }
                        b'$' => index = expand(line, index, &mut arena, lookup),
                        _ => {
                            arena.push(byte);
                            index += 1;
                        }
                    }
                }
                arena.push(0);
                if arena.overflow {
                    return Err(Error::LineTooLong);
                }
                emit(Token::Word(start), &mut count)?;
            }
        }
    }
    Ok(count)
}

/// The NUL-terminated word at `offset`, without the terminator.
pub fn word(arena: &[u8], offset: usize) -> &[u8] {
    let rest = &arena[offset..];
    let end = rest.iter().position(|&byte| byte == 0).unwrap_or(rest.len());
    &rest[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    extern crate std;
    use std::vec::Vec;

    struct Env;

    impl Lookup for Env {
        fn expand(&mut self, name: &[u8], emit: &mut dyn FnMut(u8)) {
            let value: &[u8] = match name {
                b"HOME" => b"/",
                b"?" => b"7",
                b"A_1" => b"x y",
                _ => b"",
            };
            for &byte in value {
                emit(byte);
            }
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum T {
        W(Vec<u8>),
        Op(Token),
    }

    fn lex(line: &str) -> Result<Vec<T>, Error> {
        let mut arena = [0u8; 256];
        let mut tokens = [Token::Semi; 24];
        let count = tokenize(line.as_bytes(), &mut arena, &mut tokens, &mut Env)?;
        Ok(tokens[..count]
            .iter()
            .map(|token| match *token {
                Token::Word(offset) => T::W(word(&arena, offset).to_vec()),
                other => T::Op(other),
            })
            .collect())
    }

    fn w(text: &str) -> T {
        T::W(text.as_bytes().to_vec())
    }

    #[test]
    fn words_and_operators() {
        assert_eq!(
            lex("ls -l /etc | wc>out;echo a&&b||c >> d < e").unwrap(),
            [
                w("ls"),
                w("-l"),
                w("/etc"),
                T::Op(Token::Pipe),
                w("wc"),
                T::Op(Token::Great),
                w("out"),
                T::Op(Token::Semi),
                w("echo"),
                w("a"),
                T::Op(Token::And),
                w("b"),
                T::Op(Token::Or),
                w("c"),
                T::Op(Token::Append),
                w("d"),
                T::Op(Token::Less),
                w("e"),
            ]
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(
            lex(r#"echo 'a b' "c  d" e\ f "" 'x'"y""#).unwrap(),
            [w("echo"), w("a b"), w("c  d"), w("e f"), w(""), w("xy")]
        );
        assert_eq!(
            lex(r#"echo "a\"b\\c" '$HOME'"#).unwrap(),
            [w("echo"), w(r#"a"b\c"#), w("$HOME")]
        );
    }

    #[test]
    fn expansion() {
        assert_eq!(
            lex(r#"cd $HOME ${HOME}etc "$A_1" $? $NONE. $ "\$HOME""#).unwrap(),
            [
                w("cd"),
                w("/"),
                w("/etc"),
                w("x y"),
                w("7"),
                w("."),
                w("$"),
                w("$HOME")
            ]
        );
    }

    #[test]
    fn comments_and_blank_lines() {
        assert_eq!(lex("  # nothing").unwrap(), []);
        assert_eq!(lex("echo a # b").unwrap(), [w("echo"), w("a")]);
        assert_eq!(lex("echo a#b").unwrap(), [w("echo"), w("a#b")]);
        assert_eq!(lex("\n").unwrap(), []);
    }

    #[test]
    fn errors() {
        assert_eq!(lex("echo 'a"), Err(Error::UnterminatedQuote));
        assert_eq!(lex("echo \"a"), Err(Error::UnterminatedQuote));
        assert_eq!(lex("sleep 1 &"), Err(Error::Background));
        assert_eq!(lex(&"a ".repeat(25)), Err(Error::TooManyTokens));
        let long = "x".repeat(300);
        assert_eq!(lex(&long), Err(Error::LineTooLong));
    }
}
