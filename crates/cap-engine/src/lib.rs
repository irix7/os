// SPDX-License-Identifier: GPL-3.0-only
//! Ed25519 capability verification for the IRIX 7 object fabric.
//!
//! The capability engine is deliberately small: it verifies signatures for
//! issuers in a trust store and signs already-built capability bodies. Root
//! threshold policy, key rotation and key custody belong to the native shim
//! and the authority service.

#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::BTreeSet;
use core::fmt;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sys_primitives::{Capability, NodeKey, SignatureVerifier};

/// Trust-store construction or signature failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The public or secret key is not a valid Ed25519 key.
    InvalidKey,
    /// The issuer is not trusted.
    Untrusted,
    /// The signature did not verify.
    BadSignature,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => f.write_str("invalid Ed25519 key"),
            Self::Untrusted => f.write_str("untrusted capability issuer"),
            Self::BadSignature => f.write_str("bad capability signature"),
        }
    }
}

/// A set of trusted node public keys.
#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    keys: BTreeSet<[u8; 32]>,
}

impl TrustStore {
    /// Create an empty store.
    pub const fn new() -> Self {
        Self {
            keys: BTreeSet::new(),
        }
    }

    /// Trust one node key.
    pub fn insert(&mut self, key: NodeKey) -> bool {
        self.keys.insert(key.0)
    }

    /// Remove one node key.
    pub fn remove(&mut self, key: &NodeKey) -> bool {
        self.keys.remove(&key.0)
    }

    /// Whether a key is trusted.
    pub fn contains(&self, key: &NodeKey) -> bool {
        self.keys.contains(&key.0)
    }

    /// Number of trusted keys.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Verify a signature only from a trusted issuer.
    pub fn verify_trusted(
        &self,
        issuer: &NodeKey,
        message: &[u8],
        signature: &[u8; 64],
    ) -> Result<(), KeyError> {
        if !self.contains(issuer) {
            return Err(KeyError::Untrusted);
        }
        verify(issuer, message, signature)
    }
}

impl SignatureVerifier for TrustStore {
    fn verify(&self, issuer: &NodeKey, message: &[u8], signature: &[u8; 64]) -> bool {
        self.verify_trusted(issuer, message, signature).is_ok()
    }
}

/// Verify an Ed25519 signature.
pub fn verify(issuer: &NodeKey, message: &[u8], signature: &[u8; 64]) -> Result<(), KeyError> {
    let key = VerifyingKey::from_bytes(&issuer.0).map_err(|_| KeyError::InvalidKey)?;
    let signature = Signature::from_bytes(signature);
    key.verify_strict(message, &signature)
        .map_err(|_| KeyError::BadSignature)
}

/// Derive the public `NodeKey` for a signing key.
pub fn node_key(signing: &SigningKey) -> NodeKey {
    NodeKey(signing.verifying_key().to_bytes())
}

/// Sign a capability's 256-byte signed prefix in place.
pub fn sign_capability(signing: &SigningKey, capability: &mut Capability) {
    let signature = signing.sign(&capability.signed_bytes());
    capability.signature = signature.to_bytes();
}

/// Verify a capability's signature against a trust store.
pub fn verify_capability(store: &TrustStore, capability: &Capability) -> Result<(), KeyError> {
    store.verify_trusted(
        &capability.issuer,
        &capability.signed_bytes(),
        &capability.signature,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sys_primitives::{NodeKey, ObjectId, PrincipalId, Rights};

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[7u8; 32])
    }

    fn capability(key: &NodeKey) -> Capability {
        Capability {
            magic: Capability::MAGIC,
            version: Capability::VERSION,
            rights: Rights::READ,
            flags: 0,
            object: ObjectId([1; 32]),
            issuer: *key,
            subject: PrincipalId([2; 32]),
            home: sys_primitives::NodeId([3; 16]),
            not_before: 10,
            not_after: 20,
            object_epoch: 1,
            home_epoch: 1,
            root_epoch: 1,
            reserved0: 0,
            nonce: [4; 16],
            parent: ObjectId::ZERO,
            delegation: ObjectId([5; 32]),
            reserved1: [0; 24],
            signature: [0; 64],
        }
    }

    #[test]
    fn signs_and_verifies_a_capability() {
        let signing = signing_key();
        let public = node_key(&signing);
        let mut token = capability(&public);
        sign_capability(&signing, &mut token);
        let mut store = TrustStore::new();
        assert!(store.insert(public));
        assert!(verify_capability(&store, &token).is_ok());
        assert!(token
            .verify(&store, 15, Rights::READ, ObjectId([1; 32]))
            .is_ok());
    }

    #[test]
    fn rejects_untrusted_and_tampered_tokens() {
        let signing = signing_key();
        let public = node_key(&signing);
        let mut token = capability(&public);
        sign_capability(&signing, &mut token);
        let empty = TrustStore::new();
        assert_eq!(verify_capability(&empty, &token), Err(KeyError::Untrusted));

        token.object = ObjectId([9; 32]);
        let mut store = TrustStore::new();
        store.insert(public);
        assert_eq!(
            verify_capability(&store, &token),
            Err(KeyError::BadSignature)
        );
    }

    #[test]
    fn rejects_invalid_public_key() {
        let key = NodeKey([0; 32]);
        assert!(matches!(
            verify(&key, b"x", &[0; 64]),
            Err(KeyError::InvalidKey) | Err(KeyError::BadSignature)
        ));
    }

    #[test]
    fn trust_store_removal() {
        let signing = signing_key();
        let public = node_key(&signing);
        let mut store = TrustStore::new();
        store.insert(public);
        assert!(store.contains(&public));
        assert!(store.remove(&public));
        assert!(store.is_empty());
    }
}
