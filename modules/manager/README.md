# runtime manager

Downloadable runtime module manager. It is a flat slot-B image that must fit
the first 256 KiB of slot B (`0x88480000..0x884c0000`); the five 256 KiB arena
regions start at `0x884c0000`.

Normal path (NVRAM):

```text
manager.index=<url>,<sha256>
manager.index.sig=<epoch>:<128-hex Ed25519 signature>
manager.root=<name>@<version>
manager.trust=<url>,<sha256>          # optional; else the embedded policy
```

The manager loads the index into the reserved metadata region (region 4) with
`module_load`, verifies its digest and detached signature, parses the v2 index,
resolves the exact graph and verifies every record signature. It then loads each
image into the region named by its index record, parses the manifest v2 footer,
checks identity/ABI/dependencies/exports, verifies the entry offset and export
registration, and executes dependencies before the root. Any failure unloads
every image from the transaction and publishes `status=failed`; no dependent is
entered.

The legacy `manager.dep=<url>,<sha256>` path remains as a single-image fallback.
Its signature is `manager.dep.sig`; the module name and version default to
`demo-dep`/`1.0.0` and may be set with `manager.dep.name` and
`manager.dep.version`.

The embedded trust policy is `packages/release-trust.conf`, or the file named by
`IRIX7_TRUST_POLICY` at build time. The checked-in release policy requires
signatures; unsigned mode exists only when a policy explicitly says
`require-signatures=0`.

`ModuleStatus` at `0x883fe000` carries the bounded result read by MCP
`GET /modules`:

```text
status=ok root=demo-root@1.0.0 nodes=2 signatures=ed25519
order=demo-dep@1.0.0,demo-root@1.0.0
demo-dep@1.0.0 region=0 handle=... entry=0 status=0
export=demo-dep@1.0.0 interface=irix7.demo.service version=1 offset=136
demo-root@1.0.0 region=1 handle=... entry=0 status=0
```

The image may be built with a test policy and loaded with `module b <url>
<sha256>`; `tools/module-release` produces the wrapped, signed image and the
index/trust files. `modules/demo-root` and `modules/demo-dep` are the CI graph.
