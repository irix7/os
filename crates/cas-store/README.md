# cas-store

In-memory content-addressed store policy: put, fetch, pin and remove.

This is the host-checkable reference for the block-backed store. Immutable
objects are keyed by their `cas-core` OID; a pin is a reference that prevents
removal. The real store adds chunking, a superblock, crash consistency and GC
behind the same policy.
