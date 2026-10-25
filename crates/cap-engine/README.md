# cap-engine

Ed25519 verification and trust-store policy for object capabilities.

`TrustStore` implements `sys_primitives::SignatureVerifier`; only issuers
present in the trust store are accepted. A signing helper fills the 64-byte
signature over a capability's 256-byte signed prefix. This crate owns no key
storage or key generation policy and has no hardware dependency.
