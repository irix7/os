# Embedded CA roots

The kernel TLS client verifies server certificate chains against the roots in
this directory.  A chain that does not terminate at one of these roots is
rejected.

| File | Subject | DER SHA-256 | Source |
|---|---|---|---|
| `sslcom-ecc-root.der` | `CN=SSL.com TLS ECC Root CA 2022, O=SSL Corporation, C=US` | `c32ffd9f46f936d16c3673990959434b9ad60aafbb9e7cf33654f144cc1ba143` | Mozilla CA bundle (`/etc/ssl/certs/ca-bundle.crt`) |
| `usertrust-ecc-root.der` | `CN=USERTrust ECC Certification Authority, O=The USERTRUST Network, C=US` | `4ff460d54b9c86dabfbcfc5712e0400d2bed3fbc4d4fbdaa86e06adcd2a9ad7a` | Mozilla CA bundle |
| `isrg-root-x1.der` | `CN=ISRG Root X1, O=Internet Security Research Group, C=US` | `96bcec06264976f37460779acf28c5a7cfe8a3c0aae11a8ffcee05c0bddf08c6` | Mozilla CA bundle |

Notes:

- The SSL.com root is ECDSA P-384.  Servers on this chain (for example
  `example.com` via Cloudflare) send a leaf and ECDSA intermediates; the
  `p384` feature of `embedded-tls` verifies the SHA-384 signatures.
- `github.com` uses a Sectigo ECC chain.  The server sends
  `Sectigo Public Server Authentication Root E46` cross-signed by USERTrust;
  the verifier trusts the USERTrust ECC root and accepts that chain.
- GitHub's raw, release-asset and Pages endpoints use a Let's Encrypt chain:
  `*.github.io` -> `YR1` -> `Root YR` cross-signed by ISRG Root X1.  The
  kernel selects ISRG Root X1 from the last certificate's issuer and verifies
  the cross-signed `Root YR` against it.  This is needed for both the boot
  configuration and module downloads.
- The Let's Encrypt certificate chains use RSA PKCS#1 SHA-256 signatures.  The
  kernel enables the `rsa` feature of `embedded-tls`; the Sectigo and SSL.com
  chains use ECDSA and need `p384`.
- Servers may append a self-signed root or a cross-signed copy of it.  The
  kernel selects the embedded root by matching the issuer of the last chain
  entry (`root_for` in `src/tls.rs`), then hands the original chain to the
  verifier so the TLS CertificateVerify check still has its raw entries.
- Validity dates are checked only when the wall clock has been set by NTP;
  the chain signatures and the hostname are checked unconditionally.
