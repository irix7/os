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
use alloc::vec::Vec;
use core::fmt;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sys_primitives::{Capability, NodeId, NodeKey, ObjectId, SignatureVerifier};

/// Trust-store construction or signature failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// The public or secret key is not a valid Ed25519 key.
    InvalidKey,
    /// The issuer is not trusted.
    Untrusted,
    /// The signature did not verify.
    BadSignature,
    /// The requested operation or manifest slot is unsupported.
    Unsupported,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidKey => f.write_str("invalid Ed25519 key"),
            Self::Untrusted => f.write_str("untrusted capability issuer"),
            Self::BadSignature => f.write_str("bad capability signature"),
            Self::Unsupported => f.write_str("unsupported operation"),
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

/// A node's Ed25519 signing key.
///
/// The key is a capability-engine primitive so callers do not link the
/// signature library directly: `cap-engine` owns the algorithm and the trust
/// store.  The key material never leaves the authority node in M2; the public
/// half is distributed as a `NodeKey`.
#[derive(Clone)]
pub struct SigningAuthority {
    key: SigningKey,
}

impl SigningAuthority {
    /// Build a signing authority from a 32-byte Ed25519 seed.
    pub fn from_bytes(bytes: &[u8; 32]) -> Self {
        Self {
            key: SigningKey::from_bytes(bytes),
        }
    }

    /// The public key peers use to verify this authority's signatures.
    pub fn public(&self) -> NodeKey {
        node_key(&self.key)
    }

    /// Sign an arbitrary protocol message.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message).to_bytes()
    }
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

/// Maximum authority keys in one root manifest.
pub const MAX_AUTHORITY: usize = 7;

/// A threshold-signed root authority manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootManifest {
    /// Manifest format version.
    pub version: u32,
    /// Authority epoch.
    pub epoch: u32,
    /// Required valid signatures.
    pub threshold: u8,
    /// Number of valid authority slots.
    pub count: u8,
    /// Authority public keys, with unused slots zeroed.
    pub authorities: [NodeKey; MAX_AUTHORITY],
    /// Previous manifest OID, or zero for genesis.
    pub previous: ObjectId,
    /// Signature per authority slot; unused and unsigned slots are `None`.
    pub signatures: [Option<[u8; 64]>; MAX_AUTHORITY],
}

impl RootManifest {
    /// Manifest format version understood by this crate.
    pub const VERSION: u32 = 1;

    /// Build a manifest. Authority keys must be non-zero and unique.
    pub fn new(
        epoch: u32,
        threshold: u8,
        authorities: &[NodeKey],
        previous: ObjectId,
    ) -> Result<Self, KeyError> {
        if authorities.is_empty() || authorities.len() > MAX_AUTHORITY {
            return Err(KeyError::InvalidKey);
        }
        if threshold == 0 || threshold as usize > authorities.len() {
            return Err(KeyError::InvalidKey);
        }
        let mut keys = [NodeKey([0; 32]); MAX_AUTHORITY];
        for (index, key) in authorities.iter().enumerate() {
            if key.0.iter().all(|&byte| byte == 0) {
                return Err(KeyError::InvalidKey);
            }
            for earlier in &keys[..index] {
                if earlier == key {
                    return Err(KeyError::InvalidKey);
                }
            }
            keys[index] = *key;
        }
        Ok(Self {
            version: Self::VERSION,
            epoch,
            threshold,
            count: authorities.len() as u8,
            authorities: keys,
            previous,
            signatures: [None; MAX_AUTHORITY],
        })
    }

    /// Canonical bytes covered by the threshold signatures.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 4 + 1 + 1 + MAX_AUTHORITY * 32 + 32);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.push(self.threshold);
        out.push(self.count);
        for key in &self.authorities {
            out.extend_from_slice(&key.0);
        }
        out.extend_from_slice(self.previous.as_bytes());
        out
    }

    /// Sign one authority slot with its key.
    pub fn sign(&mut self, index: usize, signing: &SigningKey) -> Result<(), KeyError> {
        if index >= self.count as usize {
            return Err(KeyError::Unsupported);
        }
        if signing.verifying_key().to_bytes() != self.authorities[index].0 {
            return Err(KeyError::InvalidKey);
        }
        let signature = signing.sign(&self.signed_bytes());
        self.signatures[index] = Some(signature.to_bytes());
        Ok(())
    }

    /// Verify the threshold of signatures over the manifest.
    pub fn verify(&self) -> Result<(), KeyError> {
        if self.version != Self::VERSION || self.count as usize > MAX_AUTHORITY {
            return Err(KeyError::InvalidKey);
        }
        if self.threshold == 0 || self.threshold > self.count {
            return Err(KeyError::InvalidKey);
        }
        let message = self.signed_bytes();
        let mut valid = 0u8;
        for index in 0..self.count as usize {
            if let Some(signature) = self.signatures[index] {
                if verify(&self.authorities[index], &message, &signature).is_ok() {
                    valid = valid.saturating_add(1);
                } else {
                    return Err(KeyError::BadSignature);
                }
            }
        }
        if valid < self.threshold {
            return Err(KeyError::BadSignature);
        }
        Ok(())
    }
}

/// A home certificate for a shard range, signed by the root threshold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeCertificate {
    /// Certificate format version.
    pub version: u32,
    /// Home node authorised for the range.
    pub node: NodeId,
    /// Inclusive start of the 64-bit OID prefix range.
    pub shard_start: u64,
    /// Inclusive end of the 64-bit OID prefix range.
    pub shard_end: u64,
    /// Shard generation.
    pub epoch: u32,
    /// Root manifest epoch at issue.
    pub root_epoch: u32,
    /// Expiry as Unix nanoseconds.
    pub not_after: u64,
    /// Signature per root authority slot.
    pub signatures: [Option<[u8; 64]>; MAX_AUTHORITY],
}

impl HomeCertificate {
    /// Certificate format version.
    pub const VERSION: u32 = 1;

    /// Build an unsigned certificate for a root manifest and range.
    pub fn new(
        root: &RootManifest,
        node: NodeId,
        shard_start: u64,
        shard_end: u64,
        epoch: u32,
        not_after: u64,
    ) -> Result<Self, KeyError> {
        if root.verify().is_err() || shard_start > shard_end || node.is_zero() {
            return Err(KeyError::InvalidKey);
        }
        Ok(Self {
            version: Self::VERSION,
            node,
            shard_start,
            shard_end,
            epoch,
            root_epoch: root.epoch,
            not_after,
            signatures: [None; MAX_AUTHORITY],
        })
    }

    /// Canonical bytes covered by the root signatures.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(4 + 16 + 8 + 8 + 4 + 4 + 8);
        out.extend_from_slice(&self.version.to_le_bytes());
        out.extend_from_slice(self.node.as_bytes());
        out.extend_from_slice(&self.shard_start.to_le_bytes());
        out.extend_from_slice(&self.shard_end.to_le_bytes());
        out.extend_from_slice(&self.epoch.to_le_bytes());
        out.extend_from_slice(&self.root_epoch.to_le_bytes());
        out.extend_from_slice(&self.not_after.to_le_bytes());
        out
    }

    /// Sign the certificate with one root authority key.
    pub fn sign(
        &mut self,
        root: &RootManifest,
        index: usize,
        signing: &SigningKey,
    ) -> Result<(), KeyError> {
        if index >= root.count as usize
            || signing.verifying_key().to_bytes() != root.authorities[index].0
        {
            return Err(KeyError::InvalidKey);
        }
        let signature = signing.sign(&self.signed_bytes());
        self.signatures[index] = Some(signature.to_bytes());
        Ok(())
    }

    /// Verify the certificate against a valid root manifest.
    pub fn verify(&self, root: &RootManifest) -> Result<(), KeyError> {
        if self.version != Self::VERSION
            || self.root_epoch != root.epoch
            || self.node.is_zero()
            || self.shard_start > self.shard_end
        {
            return Err(KeyError::InvalidKey);
        }
        root.verify()?;
        let message = self.signed_bytes();
        let mut valid = 0u8;
        for index in 0..root.count as usize {
            if let Some(signature) = self.signatures[index] {
                if verify(&root.authorities[index], &message, &signature).is_ok() {
                    valid = valid.saturating_add(1);
                } else {
                    return Err(KeyError::BadSignature);
                }
            }
        }
        if valid < root.threshold {
            return Err(KeyError::BadSignature);
        }
        Ok(())
    }
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

    #[test]
    fn signing_authority_signs_arbitrary_messages() {
        let authority = SigningAuthority::from_bytes(&[9u8; 32]);
        let public = authority.public();
        let message = b"IRIX7-M2-EPOCH-v1 epoch=2";
        let signature = authority.sign(message);
        assert!(verify(&public, message, &signature).is_ok());
        assert_eq!(
            verify(&public, b"other", &signature),
            Err(KeyError::BadSignature)
        );
        let other = SigningAuthority::from_bytes(&[10u8; 32]);
        assert_eq!(
            verify(&other.public(), message, &signature),
            Err(KeyError::BadSignature)
        );
    }

    fn authority(i: u8) -> SigningKey {
        SigningKey::from_bytes(&[i + 1; 32])
    }

    fn authorities() -> [NodeKey; 5] {
        [
            node_key(&authority(0)),
            node_key(&authority(1)),
            node_key(&authority(2)),
            node_key(&authority(3)),
            node_key(&authority(4)),
        ]
    }

    #[test]
    fn threshold_root_manifest() {
        let keys = authorities();
        let mut manifest = RootManifest::new(1, 3, &keys, ObjectId::ZERO).expect("manifest");
        manifest.sign(0, &authority(0)).expect("sign 0");
        manifest.sign(2, &authority(2)).expect("sign 2");
        manifest.sign(4, &authority(4)).expect("sign 4");
        assert!(manifest.verify().is_ok());

        let mut weak = manifest.clone();
        weak.signatures[4] = None;
        assert_eq!(weak.verify(), Err(KeyError::BadSignature));

        let mut tampered = manifest.clone();
        tampered.epoch += 1;
        assert_eq!(tampered.verify(), Err(KeyError::BadSignature));
    }

    #[test]
    fn root_manifest_rejects_bad_shapes() {
        let keys = authorities();
        assert_eq!(
            RootManifest::new(1, 6, &keys, ObjectId::ZERO),
            Err(KeyError::InvalidKey)
        );
        assert_eq!(
            RootManifest::new(1, 0, &keys, ObjectId::ZERO),
            Err(KeyError::InvalidKey)
        );
        let duplicate = [keys[0], keys[0]];
        assert_eq!(
            RootManifest::new(1, 1, &duplicate, ObjectId::ZERO),
            Err(KeyError::InvalidKey)
        );
    }
    #[test]
    fn home_certificate_threshold() {
        let keys = authorities();
        let mut root = RootManifest::new(1, 3, &keys, ObjectId::ZERO).expect("root");
        root.sign(0, &authority(0)).expect("root 0");
        root.sign(2, &authority(2)).expect("root 2");
        root.sign(4, &authority(4)).expect("root 4");
        assert!(root.verify().is_ok());
        let mut cert =
            HomeCertificate::new(&root, sys_primitives::NodeId([9; 16]), 0, 1024, 1, 100)
                .expect("certificate");
        cert.sign(&root, 0, &authority(0)).expect("sign 0");
        cert.sign(&root, 2, &authority(2)).expect("sign 2");
        cert.sign(&root, 4, &authority(4)).expect("sign 4");
        assert!(cert.verify(&root).is_ok());

        let mut weak = cert.clone();
        weak.signatures[4] = None;
        assert_eq!(weak.verify(&root), Err(KeyError::BadSignature));

        let mut tampered = cert;
        tampered.shard_end += 1;
        assert_eq!(tampered.verify(&root), Err(KeyError::BadSignature));
    }
}
