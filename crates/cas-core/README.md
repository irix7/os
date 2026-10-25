# cas-core

Content-addressed storage primitives for the IRIX 7 logical computer.

An object OID is the BLAKE3-256 digest of a domain separator, the object kind,
the payload length and the payload. Immutable blobs and chunks are fetched by
OID; a `Manifest` lists the chunks of a larger object. Manifests are canonical
little-endian wire objects and are themselves content-addressed.

This crate is `no_std` with `alloc`. It owns hashing and the manifest codec;
the block store, page cache, replication and garbage collector are separate
native services.
