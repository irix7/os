//! Probe crate for the `-Zbuild-std=std` spike.
//!
//! It has no code and no dependencies.  Its purpose is to give Cargo a package
//! to build next to the standard library, so that `std` is compiled for the
//! target and the resulting rlib can be inspected.  Nothing here links or runs.
#![no_std]
