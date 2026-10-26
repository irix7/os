# module-trust

Module signature, release trust-policy and key-epoch support for #217.

- `SignatureRecord` names the key epoch and carries a 64-byte Ed25519 signature
  over the domain-separated image digest. Its release text form is
  `epoch:128-hex-digits`.
- `TrustStore` keeps keys by epoch and a revoked-epoch set. Verification fails
  closed for unknown, revoked or invalid signatures.
- `Rotation` is signed by the current epoch key, must advance the epoch, must
  install a live key at the new epoch and may revoke the old key.
- `TrustPolicy` is the release-owned, line-oriented trust source embedded in
  the manager or loaded through `manager.trust`. It records the current epoch,
  accepted public keys, revoked epochs and whether signatures are required.
  Missing signatures are allowed only when `require-signatures=0` is explicit.
- `tools/module-release trust` generates a policy; `modules/manager/build.rs`
  embeds `packages/release-trust.conf` or `IRIX7_TRUST_POLICY`. Rotation and
  revocation are host tests.

The crate verifies signatures over image digests; the kernel remains
responsible for SHA-256 and image copy. The manager verifies the index and
every module record before the first entry call.
