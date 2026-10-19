//! Releases of mint published to a mint server.
//!
//! A release is a manifest listing one binary per target with its size and SHA-256, signed with
//! ed25519. The secret key stays on the publisher's machine; mint has [`PUBLIC_KEY`] compiled in
//! and installs nothing it can't verify against it, so neither the server nor a publish key can
//! push binaries on their own.

use std::collections::BTreeMap;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

/// Verifies releases. Generated with `mint_release keygen`.
pub const PUBLIC_KEY: &str = "b07d63299399226a7d04bba5754f7c5fe81811f429860501647b15c88b631fbe";

pub const LINUX: &str = "x86_64-unknown-linux-gnu";
pub const WINDOWS: &str = "x86_64-pc-windows-gnu";

/// Largest binary a server takes. Cloudflare rejects request bodies over 100 MB.
pub const MAX_FILE_SIZE: u64 = 95 * 1024 * 1024;
const MAX_FILES: usize = 8;
const MAX_NOTES_LEN: usize = 4000;

/// One binary of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseFile {
    /// File name on the server.
    pub name: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Counts up from 1; mint updates when the server's is higher than its own.
    pub release: u32,
    /// Unix time (seconds).
    pub published: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// By target triple ([`LINUX`], [`WINDOWS`]).
    pub files: BTreeMap<String, ReleaseFile>,
}

/// A manifest with its signature. `manifest` holds the exact JSON that was signed, so nothing
/// depends on serializing the same way twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedManifest {
    pub manifest: String,
    /// ed25519 signature over `manifest`, hex.
    pub signature: String,
}

/// A file name the server stores and serves: `[A-Za-z0-9._-]`, not starting with a dot.
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        if self.release == 0 {
            return Err("release numbers start at 1".into());
        }
        if self.files.is_empty() || self.files.len() > MAX_FILES {
            return Err(format!("a release needs 1-{MAX_FILES} files"));
        }
        if self.notes.chars().count() > MAX_NOTES_LEN {
            return Err(format!("notes are limited to {MAX_NOTES_LEN} characters"));
        }
        let mut names = std::collections::HashSet::new();
        for (target, file) in &self.files {
            if !valid_file_name(target) || !valid_file_name(&file.name) {
                return Err(format!("invalid target or file name for {target:?}"));
            }
            if !names.insert(&file.name) {
                return Err(format!("{} is listed twice", file.name));
            }
            if file.size == 0 || file.size > MAX_FILE_SIZE {
                return Err(format!("{} has an invalid size", file.name));
            }
            if file.sha256.len() != 64 || !file.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(format!("{} has an invalid sha256", file.name));
            }
        }
        Ok(())
    }
}

impl SignedManifest {
    pub fn sign(manifest: &Manifest, key: &SigningKey) -> Self {
        let manifest = serde_json::to_string(manifest).expect("manifest serializes");
        let signature = hex::encode(key.sign(manifest.as_bytes()).to_bytes());
        Self {
            manifest,
            signature,
        }
    }

    /// Checks the signature against `public_key` (hex) and returns the valid manifest.
    pub fn verify(&self, public_key: &str) -> Result<Manifest, String> {
        let key: [u8; 32] = hex::decode(public_key)
            .ok()
            .and_then(|k| k.try_into().ok())
            .ok_or("malformed public key")?;
        let key = VerifyingKey::from_bytes(&key).map_err(|e| format!("public key: {e}"))?;
        let signature: [u8; 64] = hex::decode(&self.signature)
            .ok()
            .and_then(|s| s.try_into().ok())
            .ok_or("malformed signature")?;
        key.verify_strict(self.manifest.as_bytes(), &Signature::from_bytes(&signature))
            .map_err(|_| "bad signature")?;
        let manifest: Manifest =
            serde_json::from_str(&self.manifest).map_err(|e| format!("manifest: {e}"))?;
        manifest.validate()?;
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            release: 3,
            published: 1_800_000_000,
            notes: "fixes".into(),
            files: [(
                LINUX.to_string(),
                ReleaseFile {
                    name: "mint-3-linux".into(),
                    size: 1234,
                    sha256: "ab".repeat(32),
                },
            )]
            .into(),
        }
    }

    #[test]
    fn signatures_verify_and_tampering_is_caught() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = hex::encode(key.verifying_key().to_bytes());
        let signed = SignedManifest::sign(&manifest(), &key);
        assert_eq!(signed.verify(&public), Ok(manifest()));

        let mut tampered = signed.clone();
        tampered.manifest = tampered.manifest.replace("\"release\":3", "\"release\":4");
        assert_eq!(tampered.verify(&public), Err("bad signature".into()));

        let other = hex::encode(SigningKey::from_bytes(&[8; 32]).verifying_key().to_bytes());
        assert_eq!(signed.verify(&other), Err("bad signature".into()));
        assert!(signed.verify(PUBLIC_KEY.trim_end_matches('0')).is_err());
    }

    #[test]
    fn manifests_are_validated() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let public = hex::encode(key.verifying_key().to_bytes());
        let bad = |f: &dyn Fn(&mut Manifest)| {
            let mut m = manifest();
            f(&mut m);
            // a validly signed but invalid manifest is still refused
            SignedManifest::sign(&m, &key).verify(&public).unwrap_err()
        };
        bad(&|m| m.release = 0);
        bad(&|m| m.files.clear());
        bad(&|m| m.files.get_mut(LINUX).unwrap().name = "../keys".into());
        bad(&|m| m.files.get_mut(LINUX).unwrap().sha256 = "xyz".into());
        bad(&|m| m.files.get_mut(LINUX).unwrap().size = MAX_FILE_SIZE + 1);
        assert!(!valid_file_name(".hidden") && valid_file_name("mint-12.exe"));
    }
}
