// SPDX-License-Identifier: GPL-3.0-only
//! A tiny `no_std` subset of Charmbracelet's Lip Gloss.
//!
//! The upstream Rust ports (`lipgloss`, `rusty-lipgloss`, `charming-lipgloss`)
//! assume `std`, libc and terminal discovery.  The ROM cannot link any of
//! that, so this module keeps the useful part: composable ANSI styles,
//! truecolor/256/16-colour profiles, gradients and rounded box primitives.
//! Layout is deliberately explicit; the telnet UI supplies the widths.

use core::fmt::{self, Write};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Rgb(u8, u8, u8),
    Ansi256(u8),
    Ansi16(u8),
    Default,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorMode {
    TrueColor,
    Ansi256,
    Ansi16,
    None,
}

impl ColorMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "true" | "truecolor" | "24" => Some(Self::TrueColor),
            "256" => Some(Self::Ansi256),
            "16" | "ansi" => Some(Self::Ansi16),
            "off" | "none" | "plain" => Some(Self::None),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::TrueColor => "truecolor",
            Self::Ansi256 => "256",
            Self::Ansi16 => "16",
            Self::None => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Style {
    fg: Option<Color>,
    bg: Option<Color>,
    bold: bool,
    dim: bool,
    italic: bool,
    underline: bool,
    reverse: bool,
}

impl Style {
    pub const fn new() -> Self {
        Self {
            fg: None,
            bg: None,
            bold: false,
            dim: false,
            italic: false,
            underline: false,
            reverse: false,
        }
    }

    pub const fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    pub const fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    pub const fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }

    pub const fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    pub const fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    pub const fn reverse(mut self) -> Self {
        self.reverse = true;
        self
    }

    fn active(&self, mode: ColorMode) -> bool {
        mode != ColorMode::None
            && (self.fg.is_some()
                || self.bg.is_some()
                || self.bold
                || self.dim
                || self.italic
                || self.underline
                || self.reverse)
    }

    pub fn open<W: Write + ?Sized>(&self, mode: ColorMode, out: &mut W) -> fmt::Result {
        if !self.active(mode) {
            return Ok(());
        }
        out.write_str("\x1b[")?;
        let mut first = true;
        if self.bold {
            push_code(out, &mut first, "1")?;
        }
        if self.dim {
            push_code(out, &mut first, "2")?;
        }
        if self.italic {
            push_code(out, &mut first, "3")?;
        }
        if self.underline {
            push_code(out, &mut first, "4")?;
        }
        if self.reverse {
            push_code(out, &mut first, "7")?;
        }
        if let Some(color) = self.fg {
            write_color(out, &mut first, color, mode, false)?;
        }
        if let Some(color) = self.bg {
            write_color(out, &mut first, color, mode, true)?;
        }
        out.write_str("m")
    }

    pub fn close<W: Write + ?Sized>(&self, mode: ColorMode, out: &mut W) -> fmt::Result {
        if self.active(mode) {
            out.write_str("\x1b[0m")
        } else {
            Ok(())
        }
    }

    pub fn render<W: Write + ?Sized>(
        &self,
        text: &str,
        mode: ColorMode,
        out: &mut W,
    ) -> fmt::Result {
        self.open(mode, out)?;
        out.write_str(text)?;
        self.close(mode, out)
    }
}

fn push_code<W: Write + ?Sized>(out: &mut W, first: &mut bool, code: &str) -> fmt::Result {
    if !*first {
        out.write_str(";")?;
    }
    *first = false;
    out.write_str(code)
}

fn write_color<W: Write + ?Sized>(
    out: &mut W,
    first: &mut bool,
    color: Color,
    mode: ColorMode,
    background: bool,
) -> fmt::Result {
    if mode == ColorMode::None {
        return Ok(());
    }
    let (prefix, suffix) = if background {
        ("48", "49")
    } else {
        ("38", "39")
    };
    match color {
        Color::Default => push_code(out, first, suffix),
        Color::Ansi16(n) => {
            let n = n & 0x0f;
            let code = if n < 8 {
                (if background { 40 } else { 30 }) + n as u16
            } else {
                (if background { 100 } else { 90 }) + (n - 8) as u16
            };
            let mut buf = [0u8; 3];
            let code = format_u16(code, &mut buf);
            push_code(out, first, code)
        }
        Color::Ansi256(n) => {
            if mode == ColorMode::Ansi16 {
                return write_ansi16(out, first, n, background);
            }
            write_indexed(out, first, prefix, n)
        }
        Color::Rgb(r, g, b) => match mode {
            ColorMode::TrueColor => {
                if !*first {
                    out.write_str(";")?;
                }
                *first = false;
                write!(out, "{};2;{};{};{}", prefix, r, g, b)
            }
            ColorMode::Ansi256 => {
                let n = rgb_to_256(r, g, b);
                write_indexed(out, first, prefix, n)
            }
            ColorMode::Ansi16 => write_ansi16(out, first, rgb_to_16(r, g, b), background),
            ColorMode::None => Ok(()),
        },
    }
}

fn write_indexed<W: Write + ?Sized>(
    out: &mut W,
    first: &mut bool,
    prefix: &str,
    n: u8,
) -> fmt::Result {
    if !*first {
        out.write_str(";")?;
    }
    *first = false;
    write!(out, "{};5;{}", prefix, n)
}

fn write_ansi16<W: Write + ?Sized>(
    out: &mut W,
    first: &mut bool,
    n: u8,
    background: bool,
) -> fmt::Result {
    let n = n & 0x0f;
    let code = if n < 8 {
        (if background { 40 } else { 30 }) + n as u16
    } else {
        (if background { 100 } else { 90 }) + (n - 8) as u16
    };
    let mut buf = [0u8; 3];
    let code = format_u16(code, &mut buf);
    push_code(out, first, code)
}

fn format_u16(mut value: u16, buf: &mut [u8; 3]) -> &str {
    let mut i = buf.len();
    if value == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while value > 0 {
        i -= 1;
        buf[i] = b'0' + (value % 10) as u8;
        value /= 10;
    }
    core::str::from_utf8(&buf[i..]).unwrap_or("")
}

/// Convert an RGB triple to the nearest xterm-256 index.
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    // Greys get the dedicated 232..255 ramp when the channels are close.
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max - min < 10 {
        let grey = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        if grey < 8 {
            return 16;
        }
        if grey > 238 {
            return 231;
        }
        return 232 + (grey - 8) / 10;
    }
    16 + 36 * (r / 51) + 6 * (g / 51) + (b / 51)
}

/// Map RGB to a basic ANSI colour index (0..15).
pub fn rgb_to_16(r: u8, g: u8, b: u8) -> u8 {
    let bright = (r.max(g).max(b) > 170) as u8;
    let index = ((r > 96) as u8) | (((g > 96) as u8) << 1) | (((b > 96) as u8) << 2);
    index + bright * 8
}

/// Write `text` with a per-character truecolor gradient.  In reduced colour
/// modes the interpolated RGB is quantised by `Style`.
pub fn gradient<W: Write + ?Sized>(
    text: &str,
    start: (u8, u8, u8),
    end: (u8, u8, u8),
    mode: ColorMode,
    out: &mut W,
) -> fmt::Result {
    if mode == ColorMode::None || text.is_empty() {
        return out.write_str(text);
    }
    let len = text.chars().count();
    let span = len.saturating_sub(1).max(1) as u32;
    for (i, ch) in text.chars().enumerate() {
        let t = i as u32;
        let r = lerp(start.0, end.0, t, span);
        let g = lerp(start.1, end.1, t, span);
        let b = lerp(start.2, end.2, t, span);
        out.write_str("\x1b[")?;
        let mut first = true;
        write_color(out, &mut first, Color::Rgb(r, g, b), mode, false)?;
        out.write_str("m")?;
        write!(out, "{}", ch)?;
    }
    out.write_str("\x1b[0m")
}

fn lerp(a: u8, b: u8, i: u32, span: u32) -> u8 {
    let a = a as i32;
    let b = b as i32;
    (a + (b - a) * i as i32 / span as i32) as u8
}

pub const TOP_LEFT: char = '╭';
pub const TOP_RIGHT: char = '╮';
pub const BOTTOM_LEFT: char = '╰';
pub const BOTTOM_RIGHT: char = '╯';
pub const HORIZONTAL: char = '─';
pub const VERTICAL: char = '│';

/// Top border with a title: `╭─ TITLE ────╮`.
pub fn box_top<W: Write + ?Sized>(
    out: &mut W,
    title: &str,
    width: usize,
    border: Style,
    heading: Style,
    mode: ColorMode,
) -> fmt::Result {
    let width = width.max(title.chars().count() + 5);
    border.render("╭─ ", mode, out)?;
    heading.render(title, mode, out)?;
    border.open(mode, out)?;
    out.write_str(" ")?;
    let used = title.chars().count() + 5;
    for _ in used..width.saturating_sub(1) {
        out.write_str("─")?;
    }
    out.write_str("╮")?;
    border.close(mode, out)?;
    out.write_str("\r\n")
}

/// Bottom border: `╰────────────╯`.
pub fn box_bottom<W: Write + ?Sized>(
    out: &mut W,
    width: usize,
    border: Style,
    mode: ColorMode,
) -> fmt::Result {
    border.open(mode, out)?;
    out.write_str("╰")?;
    for _ in 0..width.saturating_sub(2) {
        out.write_str("─")?;
    }
    out.write_str("╯")?;
    border.close(mode, out)?;
    out.write_str("\r\n")
}

/// A separator inside a box: `├────────────┤`.
pub fn box_rule<W: Write + ?Sized>(
    out: &mut W,
    width: usize,
    border: Style,
    mode: ColorMode,
) -> fmt::Result {
    border.open(mode, out)?;
    out.write_str("├")?;
    for _ in 0..width.saturating_sub(2) {
        out.write_str("─")?;
    }
    out.write_str("┤")?;
    border.close(mode, out)?;
    out.write_str("\r\n")
}

/// Write `s` truncated or padded to exactly `width` characters.
pub fn pad<W: Write + ?Sized>(out: &mut W, s: &str, width: usize) -> fmt::Result {
    let count = s.chars().count();
    for ch in s.chars().take(width) {
        write!(out, "{}", ch)?;
    }
    for _ in count..width {
        out.write_str(" ")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;

    #[test]
    fn plain_mode_has_no_escapes() {
        let mut out = String::new();
        Style::new()
            .bold()
            .fg(Color::Rgb(1, 2, 3))
            .render("x", ColorMode::None, &mut out)
            .unwrap();
        assert_eq!(out, "x");
    }

    #[test]
    fn truecolor_style_renders() {
        let mut out = String::new();
        Style::new()
            .bold()
            .fg(Color::Rgb(1, 2, 3))
            .render("x", ColorMode::TrueColor, &mut out)
            .unwrap();
        assert_eq!(out, "\x1b[1;38;2;1;2;3mx\x1b[0m");
    }

    #[test]
    fn gradient_resets() {
        let mut out = String::new();
        gradient(
            "ab",
            (0, 0, 0),
            (255, 255, 255),
            ColorMode::TrueColor,
            &mut out,
        )
        .unwrap();
        assert!(out.starts_with("\x1b[38;2;0;0;0m"));
        assert!(out.ends_with("a\x1b[38;2;255;255;255mb\x1b[0m"));
    }

    #[test]
    fn box_top_has_requested_width_in_plain_mode() {
        let mut out = String::new();
        box_top(
            &mut out,
            "HI",
            20,
            Style::new(),
            Style::new(),
            ColorMode::None,
        )
        .unwrap();
        let line = out.trim_end_matches("\r\n");
        assert_eq!(line.chars().count(), 20);
    }
}
