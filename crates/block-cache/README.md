# block-cache

A small read-only LRU cache for byte-addressed devices.

The cache is deliberately independent of EROFS and SCSI: the block cell will
own the policy, while the filesystem reader and the disk driver provide the
`ReadAt` implementation.  It is `no_std` + `alloc` and has no `unsafe`.

```rust
use block_cache::{Lru, ReadAt};

let mut cache = Lru::<_, 16>::new(http_range_device, 4096)?;
let mut block = [0u8; 512];
cache.read_at(4096, &mut block)?;
```

`Lru::hits()` and `misses()` expose counters for tests and status output.
`clear()` invalidates the data without resetting the counters.

The host tests cover hits, LRU eviction, cross-block reads, zero-length reads,
configuration errors and device-error propagation.

This is groundwork for the M4 block cell: the HTTP read-ahead window in
`kernel/src/erofs_cmd.rs` is a single 256 KiB window; the block cell will use
this cache with the SCSI driver or a larger HTTP block source.  The `erofs` and
`mcp` modules configure the cache with 64 blocks of 4 KiB (256 KiB).
