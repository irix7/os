# Embedded CA roots

The kernel TLS client verifies server certificate chains against the roots in
this directory. There is one root for now; a host whose chain does not end here
is rejected.

| File | Subject | DER SHA-256 | Source |
|---|---|---|---|
| `sslcom-ecc-root.der` | `CN=SSL.com TLS ECC Root CA 2022, O=SSL Corporation, C=US` | `c32ffd9f46f936d16c3673990959434b9ad60aafbb9e7cf33654f144cc1ba143` | Mozilla CA bundle (`/etc/ssl/certs/ca-bundle.crt`) |

Notes:

- The root is ECDSA P-384. Servers on this chain (for example `example.com`
  via Cloudflare) send a leaf and ECDSA intermediates; the `p384` feature of
  `embedded-tls` verifies the SHA-384 signatures.
- Cloudflare also appends a cross-signed copy of this root issued by
  _AAA Certificate Services_. The kernel trims that trailing entry before
  verification (`trim_cross_signed_root` in `src/tls.rs`), because the chain
  from the leaf to this root is already complete.
- Validity dates are checked only when the wall clock has been set by NTP;
  the chain signatures and the hostname are checked unconditionally.
