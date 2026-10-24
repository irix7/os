# native-abi

Provisional native IRIX 7 **n32** C ABI types and layout assertions.

This crate is the Rust home for the fixed-width C types, public struct layouts
and constants that the native POSIX.1-2024 libc and Rust `std` will use.  It is
deliberately:

- `no_std`, allocation-free and free of `unsafe`;
- layout-only — no libc functions, syscalls or implementations;
- **provisional v0** until the C oracle and the ABI review freeze every field
  and constant.

The source specification is [`docs/spec/native-libc.md`][spec].  In particular,
the `stat` size, `dirent` `NAME_MAX`, signal context and several numeric
constants are marked open there; this crate pins the recommended defaults so
changes are visible and testable.

[spec]: https://github.com/irix7/os/blob/main/docs/spec/native-libc.md

## Use

```rust
assert_eq!(core::mem::size_of::<native_abi::stat>(), 128);
assert_eq!(core::mem::offset_of!(native_abi::stat, st_mtim), 80);
assert_eq!(native_abi::EWOULDBLOCK, native_abi::EAGAIN);
```

The crate is a build-time contract for the `libc` `irix7` module, the native
libc and M9/M9.5.  It is not an ABI-stable system library and must not be
installed as one.
