// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the memory-controller bank decoding used by a kernel
//! entered from the SGI factory PROM.

#![no_std]
#![allow(dead_code)]

#[path = "../src/factory_prom.rs"]
mod factory_prom;
