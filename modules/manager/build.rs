// SPDX-License-Identifier: GPL-3.0-only
//! Embed the release trust policy in the manager image.
//!
//! `IRIX7_TRUST_POLICY` overrides the checked-in release policy for a test rig.
//! The override is a build-time input, so an unsigned or test policy cannot be
//! selected at runtime without rebuilding and re-signing the manager image.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=IRIX7_TRUST_POLICY");
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let source = env::var_os("IRIX7_TRUST_POLICY")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("../../packages/release-trust.conf"));
    println!("cargo:rerun-if-changed={}", source.display());
    let policy = fs::read(&source)
        .unwrap_or_else(|error| panic!("read trust policy {}: {error}", source.display()));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("out dir"));
    fs::write(out.join("trust.conf"), policy).expect("write embedded trust policy");
}
