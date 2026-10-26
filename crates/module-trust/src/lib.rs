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
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// Domain separator for a signed module image digest.
pub const IMAGE_DOMAIN: &[u8] = b"IRIX7-MODULE-IMAGE-v1";
/// Domain separator for a signed key rotation.
pub const ROTATION_DOMAIN: &[u8] = b"IRIX7-MODULE-ROTATION-v1";
/// Release trust-policy text format version.
pub const POLICY_VERSION: u16 = 1;

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

    /// Parse the release record `epoch:hex-signature` (128 hex digits).
    ///
    /// This is the detached signature text used by the package index and the
    /// `manager.index.sig`/`manager.dep.sig` configuration values.
    pub fn parse(text: &str) -> Result<Self, SignatureTextError> {
        let (epoch, signature) = text.split_once(':').ok_or(SignatureTextError::Malformed)?;
        if epoch.is_empty() || signature.len() != 128 {
            return Err(SignatureTextError::Malformed);
        }
        let key_epoch = epoch
            .parse::<u32>()
            .map_err(|_| SignatureTextError::Malformed)?;
        let mut bytes = [0u8; 64];
        for (index, slot) in bytes.iter_mut().enumerate() {
            let hi = hex(signature.as_bytes()[index * 2]).ok_or(SignatureTextError::Malformed)?;
            let lo =
                hex(signature.as_bytes()[index * 2 + 1]).ok_or(SignatureTextError::Malformed)?;
            *slot = (hi << 4) | lo;
        }
        Ok(Self::new(key_epoch, bytes))
    }

    /// Format the release record `epoch:hex-signature`.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.key_epoch.to_string());
        out.push(':');
        for byte in self.signature {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

/// Signature text parsing failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureTextError {
    /// The text is not `epoch:hex-signature` with 128 hex digits.
    Malformed,
}

impl fmt::Display for SignatureTextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("signature record is not epoch:128-hex-digits"),
        }
    }
}

const HEX: &[u8; 16] = b"0123456789abcdef";

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// A release trust policy: the keys and revocations the manager accepts.
///
/// The text form is the release-owned trust source. It is deliberately small
/// and line-oriented so a downloadable manager can embed it and a release
/// workflow can generate it without a parser dependency:
///
/// ```text
/// version=1
/// current-epoch=2
/// require-signatures=1
/// key 1 <64 hex public key>
/// key 2 <64 hex public key>
/// revoked 1
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustPolicy {
    /// Current release key epoch.
    pub current_epoch: u32,
    /// Whether an unsigned image is rejected. A policy must explicitly set
    /// this to `false` to allow unsigned mode.
    pub require_signatures: bool,
    /// Accepted key epochs and Ed25519 public keys.
    pub keys: Vec<(u32, VerifyingKey)>,
    /// Revoked key epochs.
    pub revoked: Vec<u32>,
}

impl TrustPolicy {
    /// Construct a policy with one live key.
    pub fn new(current_epoch: u32, key: VerifyingKey, require_signatures: bool) -> Self {
        Self {
            current_epoch,
            require_signatures,
            keys: alloc::vec![(current_epoch, key)],
            revoked: Vec::new(),
        }
    }

    /// Build the verification store for this policy.
    pub fn store(&self) -> Result<TrustStore, TrustError> {
        TrustStore::from_parts(
            self.current_epoch,
            self.keys.iter().copied(),
            self.revoked.iter().copied(),
        )
    }

    /// Verify an optional signature over an image digest.
    pub fn verify(
        &self,
        digest: &Digest,
        signature: Option<&SignatureRecord>,
    ) -> Result<SignatureState, TrustError> {
        match signature {
            Some(record) => {
                self.store()?.verify(digest, record)?;
                Ok(SignatureState::Signed)
            }
            None if self.require_signatures => Err(TrustError::MissingSignature),
            None => Ok(SignatureState::UnsignedAllowed),
        }
    }

    /// Parse the release trust-policy text.
    pub fn parse(text: &str) -> Result<Self, PolicyError> {
        let mut version = None;
        let mut current_epoch = None;
        let mut require_signatures = None;
        let mut keys = Vec::new();
        let mut revoked = Vec::new();
        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(value) = line.strip_prefix("version=") {
                if version.is_some() {
                    return Err(PolicyError::Duplicate("version"));
                }
                version = Some(
                    value
                        .parse::<u16>()
                        .map_err(|_| PolicyError::BadValue(line.into()))?,
                );
            } else if let Some(value) = line.strip_prefix("current-epoch=") {
                if current_epoch.is_some() {
                    return Err(PolicyError::Duplicate("current-epoch"));
                }
                current_epoch = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| PolicyError::BadValue(line.into()))?,
                );
            } else if let Some(value) = line.strip_prefix("require-signatures=") {
                if require_signatures.is_some() {
                    return Err(PolicyError::Duplicate("require-signatures"));
                }
                require_signatures = Some(match value {
                    "0" => false,
                    "1" => true,
                    _ => return Err(PolicyError::BadValue(line.into())),
                });
            } else if let Some(rest) = line.strip_prefix("key ") {
                let mut fields = rest.split_whitespace();
                let epoch = fields
                    .next()
                    .ok_or_else(|| PolicyError::BadValue(line.into()))?
                    .parse::<u32>()
                    .map_err(|_| PolicyError::BadValue(line.into()))?;
                let key = fields
                    .next()
                    .ok_or_else(|| PolicyError::BadValue(line.into()))?;
                if fields.next().is_some() {
                    return Err(PolicyError::BadValue(line.into()));
                }
                keys.push((epoch, parse_key(key)?));
            } else if let Some(value) = line.strip_prefix("revoked ") {
                let epoch = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| PolicyError::BadValue(line.into()))?;
                revoked.push(epoch);
            } else {
                return Err(PolicyError::UnknownLine(line.into()));
            }
        }
        let version = version.ok_or(PolicyError::Missing("version"))?;
        if version != POLICY_VERSION {
            return Err(PolicyError::UnsupportedVersion(version));
        }
        let current_epoch = current_epoch.ok_or(PolicyError::Missing("current-epoch"))?;
        let require_signatures =
            require_signatures.ok_or(PolicyError::Missing("require-signatures"))?;
        if keys.is_empty() && require_signatures {
            return Err(PolicyError::Missing("key"));
        }
        let policy = Self {
            current_epoch,
            require_signatures,
            keys,
            revoked,
        };
        if policy.require_signatures {
            policy.store()?;
        }
        Ok(policy)
    }

    /// Render the release trust-policy text.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        out.push_str("version=1\n");
        out.push_str("current-epoch=");
        out.push_str(&self.current_epoch.to_string());
        out.push('\n');
        out.push_str("require-signatures=");
        out.push_str(if self.require_signatures { "1" } else { "0" });
        out.push('\n');
        let mut keys = self.keys.clone();
        keys.sort_by_key(|(epoch, _)| *epoch);
        for (epoch, key) in keys {
            out.push_str("key ");
            out.push_str(&epoch.to_string());
            out.push(' ');
            for byte in key.as_bytes() {
                out.push(HEX[(byte >> 4) as usize] as char);
                out.push(HEX[(byte & 0x0f) as usize] as char);
            }
            out.push('\n');
        }
        let mut revoked = self.revoked.clone();
        revoked.sort_unstable();
        for epoch in revoked {
            out.push_str("revoked ");
            out.push_str(&epoch.to_string());
            out.push('\n');
        }
        out
    }
}

/// Whether an image was signed or accepted under explicit unsigned mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureState {
    /// A valid signature was present.
    Signed,
    /// No signature was present and the policy explicitly allows unsigned images.
    UnsignedAllowed,
}

/// Trust-policy parsing failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PolicyError {
    /// A required field is absent.
    Missing(&'static str),
    /// A field appears twice.
    Duplicate(&'static str),
    /// A line or value is malformed.
    BadValue(String),
    /// The policy format version is not supported.
    UnsupportedVersion(u16),
    /// A line is not understood.
    UnknownLine(String),
    /// A public key is not 32 bytes of hex.
    BadKey(String),
    /// The key set is inconsistent (duplicate, missing or revoked current key).
    KeySet(TrustError),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(field) => write!(f, "trust policy missing {field}"),
            Self::Duplicate(field) => write!(f, "trust policy repeats {field}"),
            Self::BadValue(value) => write!(f, "trust policy bad value: {value}"),
            Self::UnsupportedVersion(version) => {
                write!(f, "trust policy version {version} is unsupported")
            }
            Self::UnknownLine(line) => write!(f, "trust policy unknown line: {line}"),
            Self::BadKey(key) => write!(f, "trust policy bad public key: {key}"),
            Self::KeySet(error) => write!(f, "trust policy key set: {error}"),
        }
    }
}

impl From<TrustError> for PolicyError {
    fn from(error: TrustError) -> Self {
        Self::KeySet(error)
    }
}

fn parse_key(text: &str) -> Result<VerifyingKey, PolicyError> {
    if text.len() != 64 {
        return Err(PolicyError::BadKey(text.into()));
    }
    let mut bytes = [0u8; 32];
    for (index, slot) in bytes.iter_mut().enumerate() {
        let hi = hex(text.as_bytes()[index * 2]).ok_or_else(|| PolicyError::BadKey(text.into()))?;
        let lo =
            hex(text.as_bytes()[index * 2 + 1]).ok_or_else(|| PolicyError::BadKey(text.into()))?;
        *slot = (hi << 4) | lo;
    }
    VerifyingKey::from_bytes(&bytes).map_err(|_| PolicyError::BadKey(text.into()))
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
    /// An image has no signature but the policy requires one.
    MissingSignature,
    /// A key epoch appears more than once in a policy.
    DuplicateKey(u32),
    /// The current policy epoch has no live key.
    MissingCurrentKey(u32),
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
            Self::MissingSignature => f.write_str("image signature is missing"),
            Self::DuplicateKey(epoch) => write!(f, "duplicate key epoch {epoch}"),
            Self::MissingCurrentKey(epoch) => {
                write!(f, "current epoch {epoch} has no live key")
            }
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

    /// Build a store from a release policy.
    ///
    /// The current epoch must have a key and must not be revoked. Duplicate key
    /// epochs fail closed.
    pub fn from_parts(
        current_epoch: u32,
        keys: impl IntoIterator<Item = (u32, VerifyingKey)>,
        revoked: impl IntoIterator<Item = u32>,
    ) -> Result<Self, TrustError> {
        let mut map = BTreeMap::new();
        for (epoch, key) in keys {
            if map.insert(epoch, key).is_some() {
                return Err(TrustError::DuplicateKey(epoch));
            }
        }
        let revoked: BTreeSet<u32> = revoked.into_iter().collect();
        if !map.contains_key(&current_epoch) || revoked.contains(&current_epoch) {
            return Err(TrustError::MissingCurrentKey(current_epoch));
        }
        Ok(Self {
            current_epoch,
            keys: map,
            revoked,
        })
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

    fn key_hex(key: &VerifyingKey) -> String {
        let mut out = String::new();
        for byte in key.as_bytes() {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }

    #[test]
    fn signature_text_round_trips() {
        let record = SignatureRecord::new(7, [0xab; 64]);
        let text = record.to_text();
        assert_eq!(&text[..2], "7:");
        assert_eq!(SignatureRecord::parse(&text), Ok(record));
        assert_eq!(
            SignatureRecord::parse("1:00"),
            Err(SignatureTextError::Malformed)
        );
        assert_eq!(
            SignatureRecord::parse("x:0000"),
            Err(SignatureTextError::Malformed)
        );
    }

    #[test]
    fn trust_policy_text_round_trips_and_verifies() {
        let root = key(1);
        let digest = [0x55; 32];
        let signature = sign_image(&root, 1, &digest);
        let policy = TrustPolicy::new(1, root.verifying_key(), true);
        let text = policy.to_text();
        let parsed = TrustPolicy::parse(&text).expect("parse policy");
        assert_eq!(parsed, policy);
        assert_eq!(
            parsed.verify(&digest, Some(&signature)),
            Ok(SignatureState::Signed)
        );
        assert_eq!(
            parsed.verify(&digest, None),
            Err(TrustError::MissingSignature)
        );
        assert!(text.contains(&key_hex(&root.verifying_key())));
    }

    #[test]
    fn unsigned_mode_must_be_declared() {
        let root = key(1);
        let digest = [0x66; 32];
        let required = TrustPolicy::new(1, root.verifying_key(), true);
        let allowed = TrustPolicy::new(1, root.verifying_key(), false);
        assert_eq!(
            required.verify(&digest, None),
            Err(TrustError::MissingSignature)
        );
        assert_eq!(
            allowed.verify(&digest, None),
            Ok(SignatureState::UnsignedAllowed)
        );
        let unsigned_text = allowed.to_text();
        assert!(unsigned_text.contains("require-signatures=0"));
        assert!(
            !TrustPolicy::parse(&unsigned_text)
                .expect("parse unsigned policy")
                .require_signatures
        );
    }

    #[test]
    fn rotated_policy_verifies_the_new_epoch_and_rejects_the_old() {
        let root = key(1);
        let next = key(2);
        let mut store = TrustStore::new(1, root.verifying_key());
        let rotation = signed_rotation(&root, 1, 2, vec![(2, next.verifying_key())], vec![1]);
        store.rotate(&rotation).expect("rotate");

        // A release policy is the resulting trust set, not a rewrite of old
        // signatures: the old image is still verifiable by epoch 1 unless the
        // policy revoked it. This policy revokes epoch 1.
        let policy = TrustPolicy {
            current_epoch: 2,
            require_signatures: true,
            keys: vec![(1, root.verifying_key()), (2, next.verifying_key())],
            revoked: vec![1],
        };
        let parsed = TrustPolicy::parse(&policy.to_text()).expect("parse rotated policy");
        let old_digest = [0x77; 32];
        let new_digest = [0x88; 32];
        assert_eq!(
            parsed.verify(&old_digest, Some(&sign_image(&root, 1, &old_digest))),
            Err(TrustError::Revoked(1))
        );
        assert_eq!(
            parsed.verify(&new_digest, Some(&sign_image(&next, 2, &new_digest))),
            Ok(SignatureState::Signed)
        );
        assert_eq!(parsed.store().expect("store").current_epoch(), 2);
    }

    #[test]
    fn invalid_policies_fail_closed() {
        let root = key(1);
        let public = key_hex(&root.verifying_key());
        assert!(matches!(
            TrustPolicy::parse("version=2\ncurrent-epoch=1\nrequire-signatures=1\n"),
            Err(PolicyError::UnsupportedVersion(2))
        ));
        assert!(matches!(
            TrustPolicy::parse(&format!(
                "version=1\ncurrent-epoch=1\nrequire-signatures=1\nkey 1 {public}\nkey 1 {public}\n"
            )),
            Err(PolicyError::KeySet(TrustError::DuplicateKey(1)))
        ));
        assert!(matches!(
            TrustPolicy::parse("version=1\ncurrent-epoch=2\nrequire-signatures=1\nkey 1 00\n"),
            Err(PolicyError::BadKey(_))
        ));
        assert!(matches!(
            TrustPolicy::parse(&format!(
                "version=1\ncurrent-epoch=1\nrequire-signatures=1\nkey 1 {public}\nrevoked 1\n"
            )),
            Err(PolicyError::KeySet(TrustError::MissingCurrentKey(1)))
        ));
    }
}
