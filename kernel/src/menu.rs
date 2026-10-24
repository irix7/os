// SPDX-License-Identifier: GPL-3.0-only
//! The graphical boot menu the kernel shows after the ROM hands over.
//!
//! Laid out like the ARCS PROM's "System Maintenance Menu": a framed panel
//! with a title, a list of boot options, and a status line.  The selection
//! is marked with a `>` in the left gutter.

use crate::gfx;

// Colours (8 bits per channel).
const BG: (u32, u32, u32) = (0x0C, 0x14, 0x28);
const PANEL: (u32, u32, u32) = (0x18, 0x24, 0x40);
const BORDER: (u32, u32, u32) = (0x60, 0x78, 0xA0);
const TITLE: (u32, u32, u32) = (0xFF, 0xD8, 0x60);
const TEXT: (u32, u32, u32) = (0xD0, 0xD8, 0xE8);
const HIGHLIGHT: (u32, u32, u32) = (0x60, 0xC0, 0xFF);
const DIM: (u32, u32, u32) = (0x88, 0x94, 0xB0);

const MENU_X: i32 = 220;
const MENU_Y: i32 = 250;
const MENU_W: i32 = 840;
const MENU_H: i32 = 330;

/// The boot options, in the order the PROM listed them.
pub const OPTIONS: [&str; 5] = [
    "Boot IRIX 6.5",
    "Single-user maintenance shell",
    "Enter command interpreter",
    "Run diagnostics",
    "Boot with previous kernel arguments",
];

/// Currently highlighted option.
static mut SELECTED: usize = 0;

/// The highlighted option index.
pub fn selected() -> usize {
    unsafe { SELECTED }
}

/// Select an option by index.
pub fn select(index: usize) {
    unsafe {
        if index < OPTIONS.len() {
            SELECTED = index;
        }
    }
}

/// Move the selection up or down, wrapping at both ends.
pub fn move_selection(delta: i32) {
    unsafe {
        let n = OPTIONS.len() as i32;
        let mut s = SELECTED as i32 + delta;
        while s < 0 {
            s += n;
        }
        while s >= n {
            s -= n;
        }
        SELECTED = s as usize;
    }
}

/// Draw the frame around the menu panel.
fn draw_frame() {
    let (r, g, b) = BORDER;
    let x0 = MENU_X;
    let y0 = MENU_Y;
    let x1 = MENU_X + MENU_W;
    let y1 = MENU_Y + MENU_H;

    gfx::hline(x0, x1, y0, r, g, b);
    gfx::hline(x0, x1, y1, r, g, b);
    for y in (y0 + 1)..y1 {
        gfx::fill_span(x0, x0, y, r, g, b);
        gfx::fill_span(x1, x1, y, r, g, b);
    }
}

/// Draw the menu panel with the current selection.
pub fn draw() {
    // Background.
    gfx::fill_rect(
        0,
        0,
        gfx::SCREEN_WIDTH - 1,
        gfx::SCREEN_HEIGHT - 1,
        BG.0,
        BG.1,
        BG.2,
    );

    // Panel fill.
    gfx::fill_rect(
        MENU_X + 1,
        MENU_Y + 1,
        MENU_X + MENU_W - 1,
        MENU_Y + MENU_H - 1,
        PANEL.0,
        PANEL.1,
        PANEL.2,
    );

    draw_frame();

    // Title.
    let title = "SGI Indy  --  ARCS Boot Menu";
    let tw = title.len() as i32 * 9;
    gfx::draw_str(
        MENU_X + (MENU_W - tw) / 2,
        MENU_Y + 18,
        title,
        TITLE.0,
        TITLE.1,
        TITLE.2,
    );

    // Separator under the title.
    gfx::hline(
        MENU_X + 8,
        MENU_X + MENU_W - 8,
        MENU_Y + 44,
        BORDER.0,
        BORDER.1,
        BORDER.2,
    );

    // Options.
    let mut y = MENU_Y + 70;
    for (i, opt) in OPTIONS.iter().enumerate() {
        let sel = i == selected();
        let (r, g, b) = if sel { HIGHLIGHT } else { TEXT };
        let marker = if sel { ">" } else { " " };
        gfx::draw_str(MENU_X + 40, y, marker, r, g, b);
        gfx::draw_str(MENU_X + 62, y, opt, r, g, b);
        y += 30;
    }

    // Separator above the status line.
    gfx::hline(
        MENU_X + 8,
        MENU_X + MENU_W - 8,
        MENU_Y + MENU_H - 46,
        BORDER.0,
        BORDER.1,
        BORDER.2,
    );

    // Status line: show the live network address when DHCP has completed.
    let status = match crate::net::ip_addr() {
        Some(ip) => {
            let o = ip.octets();
            alloc::format!(
                "{}.{}.{}.{}   telnet 23   arrows/Enter to boot",
                o[0],
                o[1],
                o[2],
                o[3]
            )
        }
        None => alloc::string::String::from("No network   arrows/Enter to boot"),
    };
    gfx::draw_str(
        MENU_X + 40,
        MENU_Y + MENU_H - 30,
        &status,
        DIM.0,
        DIM.1,
        DIM.2,
    );

    // The console pane sits below the menu and the menu fill covers the whole
    // screen, so repaint it after every menu redraw.
    crate::graphics_console::flush();
}
