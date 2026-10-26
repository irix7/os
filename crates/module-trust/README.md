# module-trust

Host-side module signature and key-epoch policy for #217.

- `SignatureRecord` names the key epoch and carries a 64-byte Ed25519 signature
  over the domain-separated image digest.
- `TrustStore` keeps keys by epoch and a revoked-epoch set. Verification fails
  closed for unknown, revoked or invalid signatures.
- `Rotation` is signed by the current epoch key, must advance the epoch, must
  install a live key at the new epoch and may revoke the old key.
- This crate does not implement package-index signatures or loader integration;
  the downloadable module manager will call it before executing a module.
