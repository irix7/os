// SPDX-License-Identifier: GPL-3.0-only
//! Module signature verification and key-epoch rotation policy.
//!
//! The image digest binds the flat/image payload and any manifest/footer. A
//! signature record names the trust-store epoch of the Ed25519 key that signed
//! that digest. Rotations are signed by the current epoch key and can add or
//! revoke keys; a rotation cannot move backwards to an older epoch.
//!
//! This is host/verification policy. The downloadable module manager performs
//! the actual verification before load; the kernel only performs SHA-256
//! integrity checks.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;
use core::fmt;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Domain separator for a signed module image digest.
pub const IMAGE_DOMAIN: &[u8] = b"IRIX7-MODULE-IMAGE-v1";
/// Domain separator for a signed key rotation.
pub const ROTATION_DOMAIN: &[u8] = b"IRIX7-MODULE-ROTATION-v1";

/// A SHA-256 image digest.
pub type Digest = [u8; 32];

/// Signature attached to a module image/manifest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignatureRecord {
    /// Trust-store epoch of the signing key.
    pub key_epoch: u32,
    /// Ed25519 signature over the image digest message.
    pub signature: [u8; 64],
}

impl SignatureRecord {
    /// Construct a signature record.
    pub const fn new(key_epoch: u32, signature: [u8; 64]) -> Self {
        Self {
            key_epoch,
            signature,
        }
    }
}

/// A signed key-rotation request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rotation {
    /// Epoch of the key signing this rotation; must be the current epoch.
    pub signer_epoch: u32,
    /// Epoch created by this rotation; must be greater than the current epoch.
    pub new_epoch: u32,
    /// Keys added at their epochs.
    pub add: Vec<(u32, VerifyingKey)>,
    /// Key epochs revoked by this rotation.
    pub revoke: Vec<u32>,
    /// Signature over the canonical rotation message.
    pub signature: [u8; 64],
}

impl Rotation {
    /// The canonical bytes signed by the current key.
    pub fn message(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(ROTATION_DOMAIN);
        out.extend_from_slice(&self.signer_epoch.to_le_bytes());
        out.extend_from_slice(&self.new_epoch.to_le_bytes());
        out.extend_from_slice(&(self.add.len() as u32).to_le_bytes());
        for (epoch, key) in &self.add {
            out.extend_from_slice(&epoch.to_le_bytes());
            out.extend_from_slice(key.as_bytes());
        }
        out.extend_from_slice(&(self.revoke.len() as u32).to_le_bytes());
        for epoch in &self.revoke {
            out.extend_from_slice(&epoch.to_le_bytes());
        }
        out
    }
}

/// Trust-store failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustError {
    /// The key epoch is not in the store.
    UnknownKey(u32),
    /// The key epoch has been revoked.
    Revoked(u32),
    /// The Ed25519 signature is invalid.
    BadSignature,
    /// A rotation is signed by a non-current key.
    NotCurrentSigner,
    /// A rotation attempts to reuse or move backwards in epochs.
    BadEpoch,
    /// A rotation repeats an existing key epoch or revokes an unknown/current key.
    BadRotation,
}

impl fmt::Display for TrustError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownKey(epoch) => write!(f, "unknown key epoch {epoch}"),
            Self::Revoked(epoch) => write!(f, "revoked key epoch {epoch}"),
            Self::BadSignature => f.write_str("invalid Ed25519 signature"),
            Self::NotCurrentSigner => f.write_str("rotation not signed by current epoch"),
            Self::BadEpoch => f.write_str("rotation epoch is not newer"),
            Self::BadRotation => f.write_str("invalid key rotation"),
        }
    }
}

/// Ed25519 trust store with key epochs and revocation.
pub struct TrustStore {
    current_epoch: u32,
    keys: BTreeMap<u32, VerifyingKey>,
    revoked: BTreeSet<u32>,
}

impl TrustStore {
    /// Create a store with one root key at `epoch`.
    pub fn new(epoch: u32, key: VerifyingKey) -> Self {
        let mut keys = BTreeMap::new();
        keys.insert(epoch, key);
        Self {
            current_epoch: epoch,
            keys,
            revoked: BTreeSet::new(),
        }
    }

    /// Current trust-store epoch.
    pub fn current_epoch(&self) -> u32 {
        self.current_epoch
    }

    /// Whether a key epoch is currently accepted.
    pub fn is_live(&self, epoch: u32) -> bool {
        self.keys.contains_key(&epoch) && !self.revoked.contains(&epoch)
    }

    /// Verify an image signature.
    pub fn verify(&self, digest: &Digest, record: &SignatureRecord) -> Result<(), TrustError> {
        self.verify_key(record.key_epoch)?;
        let key = self
            .keys
            .get(&record.key_epoch)
            .expect("verify_key checked");
        let signature = Signature::from_bytes(&record.signature);
        key.verify(&image_message(digest), &signature)
            .map_err(|_| TrustError::BadSignature)
    }

    /// Verify and apply a signed key rotation.
    ///
    /// The rotation must be signed by the current epoch key. It cannot move
    /// backwards, reuse an existing epoch, revoke an unknown key, or fail to
    /// install a live key at the new epoch. The old signer key may be revoked
    /// after the new epoch key is installed.
    pub fn rotate(&mut self, rotation: &Rotation) -> Result<(), TrustError> {
        if rotation.signer_epoch != self.current_epoch {
            return Err(TrustError::NotCurrentSigner);
        }
        self.verify_key(rotation.signer_epoch)?;
        if rotation.new_epoch <= self.current_epoch {
            return Err(TrustError::BadEpoch);
        }
        if rotation
            .add
            .iter()
            .any(|(epoch, _)| *epoch <= self.current_epoch || self.keys.contains_key(epoch))
        {
            return Err(TrustError::BadRotation);
        }
        if !rotation
            .add
            .iter()
            .any(|(epoch, _)| *epoch == rotation.new_epoch)
        {
            return Err(TrustError::BadRotation);
        }
        if rotation
            .revoke
            .iter()
            .any(|epoch| !self.keys.contains_key(epoch))
        {
            return Err(TrustError::BadRotation);
        }
        let key = self
            .keys
            .get(&rotation.signer_epoch)
            .expect("verify_key checked");
        let signature = Signature::from_bytes(&rotation.signature);
        key.verify(&rotation.message(), &signature)
            .map_err(|_| TrustError::BadSignature)?;

        for (epoch, key) in &rotation.add {
            self.keys.insert(*epoch, *key);
        }
        for epoch in &rotation.revoke {
            self.revoked.insert(*epoch);
        }
        self.current_epoch = rotation.new_epoch;
        Ok(())
    }

    fn verify_key(&self, epoch: u32) -> Result<(), TrustError> {
        if !self.keys.contains_key(&epoch) {
            return Err(TrustError::UnknownKey(epoch));
        }
        if self.revoked.contains(&epoch) {
            return Err(TrustError::Revoked(epoch));
        }
        Ok(())
    }
}

/// The canonical image-digest message.
pub fn image_message(digest: &Digest) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(IMAGE_DOMAIN);
    out.extend_from_slice(digest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn key(byte: u8) -> SigningKey {
        SigningKey::from_bytes(&[byte; 32])
    }

    fn sign_image(signing: &SigningKey, epoch: u32, digest: &Digest) -> SignatureRecord {
        SignatureRecord::new(epoch, signing.sign(&image_message(digest)).to_bytes())
    }

    fn signed_rotation(
        signing: &SigningKey,
        signer_epoch: u32,
        new_epoch: u32,
        add: Vec<(u32, VerifyingKey)>,
        revoke: Vec<u32>,
    ) -> Rotation {
        let mut rotation = Rotation {
            signer_epoch,
            new_epoch,
            add,
            revoke,
            signature: [0; 64],
        };
        rotation.signature = signing.sign(&rotation.message()).to_bytes();
        rotation
    }

    #[test]
    fn verifies_known_key_and_rejects_wrong_digest() {
        let signing = key(1);
        let store = TrustStore::new(1, signing.verifying_key());
        let digest = [0x11; 32];
        let signature = sign_image(&signing, 1, &digest);
        assert_eq!(store.verify(&digest, &signature), Ok(()));
        assert_eq!(
            store.verify(&[0x22; 32], &signature),
            Err(TrustError::BadSignature)
        );
    }

    #[test]
    fn unknown_and_revoked_keys_fail() {
        let signing = key(1);
        let next = key(2);
        let store = TrustStore::new(1, signing.verifying_key());
        assert_eq!(
            store.verify(&[0x11; 32], &SignatureRecord::new(2, [0; 64])),
            Err(TrustError::UnknownKey(2))
        );
        let mut store = store;
        let rotation = signed_rotation(&signing, 1, 2, vec![(2, next.verifying_key())], vec![1]);
        store.rotate(&rotation).expect("rotate");
        assert_eq!(store.current_epoch(), 2);
        assert_eq!(
            store.verify(&[0x11; 32], &sign_image(&signing, 1, &[0x11; 32])),
            Err(TrustError::Revoked(1))
        );
        assert_eq!(
            store.verify(&[0x22; 32], &sign_image(&next, 2, &[0x22; 32])),
            Ok(())
        );
    }

    #[test]
    fn rotation_adds_a_new_key_and_advances_epoch() {
        let root = key(1);
        let next = key(2);
        let mut store = TrustStore::new(1, root.verifying_key());
        let rotation = signed_rotation(&root, 1, 2, vec![(2, next.verifying_key())], Vec::new());
        store.rotate(&rotation).expect("rotate");
        let digest = [0x33; 32];
        assert_eq!(
            store.verify(&digest, &sign_image(&next, 2, &digest)),
            Ok(())
        );
        assert_eq!(store.current_epoch(), 2);
    }

    #[test]
    fn non_current_or_stale_rotation_fails() {
        let root = key(1);
        let mut store = TrustStore::new(1, root.verifying_key());
        let next = key(2);
        let from_old = signed_rotation(&next, 1, 2, vec![(2, next.verifying_key())], Vec::new());
        assert_eq!(store.rotate(&from_old), Err(TrustError::BadSignature));
        let stale = signed_rotation(&root, 1, 1, Vec::new(), Vec::new());
        assert_eq!(store.rotate(&stale), Err(TrustError::BadEpoch));
    }

    #[test]
    fn rotation_can_revoke_the_old_key_but_must_add_a_new_epoch_key() {
        let root = key(1);
        let next = key(2);
        let mut store = TrustStore::new(1, root.verifying_key());
        let revoke_old = signed_rotation(&root, 1, 2, vec![(2, next.verifying_key())], vec![1]);
        store.rotate(&revoke_old).expect("rotate");
        let reuse = signed_rotation(&root, 1, 2, vec![(1, key(2).verifying_key())], Vec::new());
        assert_eq!(store.rotate(&reuse), Err(TrustError::NotCurrentSigner));
        let no_new_key = signed_rotation(&next, 2, 3, Vec::new(), Vec::new());
        assert_eq!(store.rotate(&no_new_key), Err(TrustError::BadRotation));
    }

    #[test]
    fn image_message_is_domain_separated() {
        let message = image_message(&[0x44; 32]);
        assert!(message.starts_with(IMAGE_DOMAIN));
        assert_eq!(message.len(), IMAGE_DOMAIN.len() + 32);
    }
}
