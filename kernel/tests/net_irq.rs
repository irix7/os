// SPDX-License-Identifier: GPL-3.0-only
//! Host tests for the interrupt-source policy.
//!
//! `src/net_irq.rs` is pure decode and counters with no MMIO and no stack
//! dependency, so the rules that decide IP2 versus IP7, spurious rejection
//! and which module region a ready socket wakes are testable without iris.

#![no_std]
#![allow(dead_code)]

#[path = "../src/net_irq.rs"]
mod net_irq;
