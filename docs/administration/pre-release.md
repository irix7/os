# Pre-release compatibility policy

**Status:** active project policy
**Applies until:** the project declares a stable base by ADR

IRIX 7 has no deployed users, no installed base and no stable release contract yet.
Nobody is running this OS as a supported system. Every internal interface should therefore
be treated as **Version 1**: the next design may replace it outright.

## Rule

Do not spend effort on compatibility with an earlier in-tree version of any internal
interface. Break it, delete the old path and update every in-tree user atomically.

This applies across the whole system, including:

- ROM/kernel handoff, kernel ABIs, syscalls and trap frames;
- the module service table, module ABI, loaders, manifests, footers, package/index formats
  and signing formats;
- mesh transport, framing, join, homes, routing, leases, membership, migration and the
  MCP wire/API;
- capability, namespace, CAS, actor, process, thread and IPC interfaces;
- filesystems, on-disk formats, cache and installer/update metadata;
- libc, POSIX shims, native libs, window/display protocols and device interfaces;
- configuration keys, NVRAM schemas, tooling formats and test fixtures.

In particular, do **not** add:

- old-peer, old-client or mixed-version fallback;
- dual-stack/dual-format code paths;
- format migration, schema migration or compatibility shims;
- deprecated aliases kept only for an earlier internal version;
- version negotiation where both ends are built from this repository;
- legacy tests whose only purpose is proving an old internal format still works.

A version number may still be recorded for reproducibility and evidence. It is not a
compatibility promise.

## What must be preserved

- Correctness, safety and security invariants.
- Explicit hardware requirements and external standards named by the product spec, such as
  POSIX/IRIX application ABIs, TLS/HTTP/DNS, filesystem formats or SGI hardware protocols.
  Those are external contracts, not permission to preserve our own obsolete internal
  versions.
- The M1/M2/M3 marker and regression evidence needed to prove the current system has not
  regressed.
- Clean-room provenance and publication rules.

External compatibility is evaluated on its own product merits. It is never a reason to
keep an internal format, wire version, API or code path that only exists because an earlier
IRIX 7 build used it.

## Review checklist

Before adding any compatibility code, ask:

1. Is there a deployed user or external consumer outside this repository? If no, remove the
   compatibility path.
2. Can every in-tree producer and consumer be changed together? If yes, do that.
3. Is this an external standard explicitly required by the product? If yes, implement the
   standard; do not confuse it with our old internal format.
4. Does the change preserve a marker, safety invariant or hardware requirement? Keep only
   that test/evidence.

If a proposed exception is genuinely necessary, record it in an ADR with the external
consumer, the removal date and the cost. Until then, assume the answer is no.

## Stable base

A stable base is a deliberate event, not an accident of elapsed time. It requires an ADR
that names the supported consumer set, the interface inventory and the compatibility
window. Until that ADR lands, every interface in this repository is pre-stable and may be
replaced without notice.
