// SPDX-License-Identifier: GPL-3.0-only
//! Host checks for the generated IRIS 10 font tables.
//!
//! The converter must strip the `/* char 0xNN */` comments from the source
//! data block; otherwise those hex literals become bogus glyph rows and every
//! offset after the first comment is wrong.

#![no_std]
#![allow(dead_code)]

#[path = "../src/font.rs"]
mod font;

#[test]
fn every_glyph_fits_in_the_data_table() {
    for (index, &(off, _width, height, _xorig, _yorig, _xmove)) in
        font::FONT_INFO.iter().enumerate()
    {
        let end = off as usize + height as usize;
        assert!(
            end <= font::FONT_DATA.len(),
            "glyph {index} ends at {end}, data has {}",
            font::FONT_DATA.len()
        );
    }
}

#[test]
fn ascii_glyphs_are_present() {
    assert!(font::FONT_INFO.len() >= 0x7f - 0x20);
    assert!(!font::FONT_DATA.is_empty());
}
