# m2-root runtime graph root

The dependent half of the M2 runtime pair. It is linked at arena region 3
(`0x88580000`) and declares an exact dependency on `m2-mesh@1.0.0`. It does
**not** link `mesh-transport`.

The manager enters `m2-mesh` first, then this image. The root:

1. resolves `irix7.mesh.transport@1` with `module_find_export`;
2. checks the returned table version, non-null table and owner handle;
3. queries the owner with `module_query` and requires region 2, a live state
   and the table inside the verified image;
4. checks the table ABI, calls the round-trip probe, checks the record length,
   the 64 KiB maximum and the framing bounds;
5. prints one line:

```text
m2-root: dep=m2-mesh@1.0.0 interface=irix7.mesh.transport version=1 owner=0x... result=ok magic=0x4d324d31 len=340 max=65536 bounds=0/-1/-2
```

A failure logs `result=fail stage=...` and returns a non-zero vtable status, so
the manager unloads the transaction; the root is never entered after a broken
dependency.

The ABI v12 `module_find` service is not used. The root relies on
`module_find_export`, which carries the verified owner handle.
