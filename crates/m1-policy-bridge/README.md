# m1-policy-bridge

The fixed read-only bridge used by the M1 Wasm policy module to publish its
committed namespace, actor and capability proof to the kernel.

The bridge record lives at the end of module slot B:

`0x8848_0000 + 0x0018_0000 - sizeof(Bridge)`

The module writes it only after commit; the kernel validates the magic/version
and uses it for the userland `/etc/motd` path. If no valid bridge is present
the native `kernel/src/logical.rs` policy is used. This is an M1-specific
surface, not the final module ABI.
