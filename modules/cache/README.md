# cache module

A one-sector proof of the ABI v11 `block_write` path and the idle SCSI cache
idea. It reads SCSI target 1 LBA 0 into module RAM, hashes it with BLAKE3, then
on two idle ticks writes an `I7CACHE1` header and the sector to target 3. A
third tick reads both back and verifies the digest.

Attach a writable image at SCSI ID 3. The module is deliberately not the final
package cache: the bounded download queue, automatic lookup, and the module
multiplexer are still to come. It must not be pointed at a data disk.
