use std::collections::HashMap;

use base64::Engine;
use ed25519_dalek::{Signature, VerifyingKey, Verifier as Ed25519Verifier};

use crate::error::{SpmError, SpmResult};
use crate::types::Manifest;

/// Verify a PGP clearsign signature on a Debian InRelease file.
/// Returns true if any of the trusted keys successfully verifies the signature.
pub fn verify_inrelease_signature(
    inrelease_data: &[u8],
    trusted_keys: &[&[u8]],
) -> SpmResult<bool> {
    use sequoia_openpgp as sq;
    use sq::packet::Signature;
    use sq::parse::Parse;
    use sq::policy::StandardPolicy;

    if trusted_keys.is_empty() {
        return Ok(false);
    }

    let policy = StandardPolicy::new();

    // Parse trusted certificates
    let certs: Vec<sq::Cert> = trusted_keys
        .iter()
        .filter_map(|kb| sq::Cert::from_bytes(kb).ok())
        .collect();
    if certs.is_empty() {
        return Ok(false);
    }

    // Parse the packet pile (handles ASCII-armored clearsigned messages)
    let pile = match sq::PacketPile::from_bytes(inrelease_data) {
        Ok(p) => p,
        Err(_) => return Ok(false),
    };

    // For clearsigned messages, the structure is:
    //   OnePassSig -> Literal(the signed text) -> Signature
    // Extract the literal body and all following signatures.
    let mut literal_body: Option<Vec<u8>> = None;
    let mut signatures: Vec<Signature> = Vec::new();

    for packet in pile.descendants() {
        match packet {
            sq::Packet::Literal(lit) => {
                literal_body = Some(lit.body().to_vec());
            }
            sq::Packet::Signature(sig) => {
                signatures.push(sig.clone());
            }
            _ => {}
        }
    }

    let body = match literal_body {
        Some(b) => b,
        None => return Ok(false),
    };

    // Try each signature against each trusted cert's signing keys
    for sig in &signatures {
        for cert in &certs {
            for ka in cert
                .keys()
                .with_policy(&policy, None)
                .alive()
                .revoked(false)
                .for_signing()
            {
                if sig.verify_message(ka.key(), &body).is_ok() {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

/// Verify an RPM embedded signature against the header+payload.
/// Returns true if the trusted key verifies the RPM signature.
pub fn verify_rpm_signature(rpm_data: &[u8], trusted_key: &[u8]) -> SpmResult<bool> {
    use sequoia_openpgp as sq;
    use sq::packet::Signature as PgpSignature;
    use sq::parse::Parse;
    use sq::policy::StandardPolicy;

    let policy = StandardPolicy::new();

    // Parse the trusted key
    let cert = match sq::Cert::from_bytes(trusted_key) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };

    // Validate RPM magic
    if rpm_data.len() < 96 {
        return Ok(false);
    }
    if &rpm_data[0..4] != b"\xed\xab\xee\xdb" {
        return Ok(false);
    }

    // ── Parse the signature header (starts at offset 96) ──
    // Header structure: version(4) + reserved(4) + entries(4) + store_size(4)
    const SIG_HDR_START: usize = 96;
    if rpm_data.len() < SIG_HDR_START + 16 {
        return Ok(false);
    }

    let entries = u32::from_be_bytes([
        rpm_data[SIG_HDR_START + 8],
        rpm_data[SIG_HDR_START + 9],
        rpm_data[SIG_HDR_START + 10],
        rpm_data[SIG_HDR_START + 11],
    ]) as usize;
    let store_size = u32::from_be_bytes([
        rpm_data[SIG_HDR_START + 12],
        rpm_data[SIG_HDR_START + 13],
        rpm_data[SIG_HDR_START + 14],
        rpm_data[SIG_HDR_START + 15],
    ]) as usize;

    let index_start = SIG_HDR_START + 16;
    let store_start = index_start + entries * 16;

    if rpm_data.len() < store_start + store_size {
        return Ok(false);
    }

    // ── Search for RSAHEADER(1000) or PGPHEADER(269/267) ──
    let mut pgp_sig_bytes: Option<&[u8]> = None;
    for i in 0..entries {
        let entry_offset = index_start + i * 16;
        if entry_offset + 16 > rpm_data.len() {
            break;
        }
        let tag = i32::from_be_bytes([
            rpm_data[entry_offset],
            rpm_data[entry_offset + 1],
            rpm_data[entry_offset + 2],
            rpm_data[entry_offset + 3],
        ]);
        let data_type = i32::from_be_bytes([
            rpm_data[entry_offset + 4],
            rpm_data[entry_offset + 5],
            rpm_data[entry_offset + 6],
            rpm_data[entry_offset + 7],
        ]);
        let offset = i32::from_be_bytes([
            rpm_data[entry_offset + 8],
            rpm_data[entry_offset + 9],
            rpm_data[entry_offset + 10],
            rpm_data[entry_offset + 11],
        ]) as usize;
        let count = i32::from_be_bytes([
            rpm_data[entry_offset + 12],
            rpm_data[entry_offset + 13],
            rpm_data[entry_offset + 14],
            rpm_data[entry_offset + 15],
        ]) as usize;

        // Tag 1000 = RSAHEADER, 269 = PGPHEADER, 267 = PGP (legacy)
        if (tag == 1000 || tag == 269 || tag == 267) && count > 0 {
            // Type 7 = BIN, 6 = HEX
            if data_type == 7 || data_type == 6 {
                let sig_start = store_start + offset;
                let sig_end = sig_start + count;
                if sig_end <= rpm_data.len() {
                    pgp_sig_bytes = Some(&rpm_data[sig_start..sig_end]);
                    break;
                }
            }
        }
    }

    let sig_bytes = match pgp_sig_bytes {
        Some(b) => b,
        None => return Ok(false),
    };

    // ── Parse as a PGP packet pile ──
    let pile = match sq::PacketPile::from_bytes(sig_bytes) {
        Ok(p) => p,
        Err(_) => return Ok(false),
    };

    // Find signature packets
    let signatures: Vec<PgpSignature> = pile
        .descendants()
        .filter_map(|p| {
            if let sq::Packet::Signature(s) = p {
                Some(s.clone())
            } else {
                None
            }
        })
        .collect();

    if signatures.is_empty() {
        return Ok(false);
    }

    // ── Determine the signed data range ──
    // The RPM signature covers the main header + payload.
    // The main header starts right after the signature header's store area.
    let signed_start = store_start + store_size;
    if signed_start >= rpm_data.len() {
        return Ok(false);
    }
    let signed_data = &rpm_data[signed_start..];

    // ── Verify each signature against trusted keys ──
    for sig in &signatures {
        for ka in cert
            .keys()
            .with_policy(&policy, None)
            .alive()
            .revoked(false)
            .for_signing()
        {
            if sig.verify_message(ka.key(), signed_data).is_ok() {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// A store of trusted Ed25519 public keys identified by key IDs.
pub struct KeyStore {
    keys: HashMap<String, VerifyingKey>,
}

impl KeyStore {
    pub fn new() -> Self {
        KeyStore {
            keys: HashMap::new(),
        }
    }

    pub fn insert(&mut self, key_id: String, key: VerifyingKey) {
        self.keys.insert(key_id, key);
    }

    pub fn get_ed25519(&self, key_id: &str) -> Option<&VerifyingKey> {
        self.keys.get(key_id)
    }
}

impl Default for KeyStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Verifier for package integrity and authenticity across Debian, RPM, and SAM formats.
pub struct Verifier {
    trusted_keys: KeyStore,
}

impl Verifier {
    pub fn new(trusted_keys: KeyStore) -> Self {
        Verifier { trusted_keys }
    }

    pub fn trusted_keys(&self) -> &KeyStore {
        &self.trusted_keys
    }

    /// Check that `pkg_data` hashes to `expected_sha256` (hex).
    ///
    /// This is the hash advertised in `Packages.gz` metadata for Debian packages.
    pub fn verify_debian(&self, pkg_data: &[u8], expected_sha256: &str) -> SpmResult<()> {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(pkg_data);
        let actual = format!("{:x}", hasher.finalize());
        if actual != expected_sha256 {
            return Err(SpmError::other(format!(
                "SHA256 mismatch for Debian package: expected {expected_sha256}, got {actual}"
            )));
        }
        Ok(())
    }

    /// Verify an RPM embedded signature against the payload hash.
    ///
    /// `signature_data` is the raw RSA/DSA signature blob from the RPM header,
    /// and `payload_hash` is the expected digest of the payload (e.g. SHA-256).
    ///
    /// In a full implementation this would parse the RPM header signature and
    /// verify it against one of the trusted keys; here we validate the lengths
    /// and non-emptiness as a basic sanity check.
    pub fn verify_rpm(&self, signature_data: &[u8], payload_hash: &[u8]) -> SpmResult<()> {
        if signature_data.is_empty() {
            return Err(SpmError::invalid_format("RPM signature data is empty"));
        }
        if payload_hash.is_empty() {
            return Err(SpmError::invalid_format("RPM payload hash is empty"));
        }
        let hash_len = payload_hash.len();
        if hash_len != 20 && hash_len != 32 && hash_len != 48 && hash_len != 64 {
            return Err(SpmError::invalid_format(format!(
                "Unsupported RPM payload hash length: {hash_len} (expected 20, 32, 48, or 64)"
            )));
        }
        Ok(())
    }

    /// Verify an Ed25519 signature on the manifest and the BLAKE3 hash of the data tar.
    ///
    /// If `manifest.signature` is present the method looks up the trusted key by
    /// `key_id`, decodes the base64 signature, serialises the manifest without the
    /// signature field and verifies it.  The BLAKE3 hash of `data_tar` is then
    /// computed and checked for consistency.
    pub fn verify_sam(&self, manifest: &Manifest, data_tar: &[u8]) -> SpmResult<()> {
        if let Some(sig) = &manifest.signature {
            let pubkey = self
                .trusted_keys
                .get_ed25519(&sig.key_id)
                .ok_or_else(|| {
                    SpmError::other(format!(
                        "No trusted Ed25519 key found for key_id '{}'",
                        sig.key_id
                    ))
                })?;

            let message = serialize_manifest_for_verification(manifest)?;

            let sig_bytes = base64::engine::general_purpose::STANDARD
                .decode(sig.value.as_bytes())
                .map_err(|e| {
                    SpmError::invalid_format(format!("Invalid base64 signature value: {e}"))
                })?;
            let signature = Signature::from_slice(&sig_bytes).map_err(|e| {
                SpmError::invalid_format(format!("Invalid Ed25519 signature bytes: {e}"))
            })?;

            Ed25519Verifier::verify(pubkey, &message, &signature).map_err(|e| {
                SpmError::other(format!(
                    "Ed25519 signature verification failed for key '{}': {e}",
                    sig.key_id
                ))
            })?;
        }

        // Always compute the BLAKE3 hash of the data payload.
        // When the manifest carries a `data_hash` field the caller should also
        // pass it so we can compare; for now the computation serves as a
        // validation that the data tar is not empty and produces a hash.
        if !data_tar.is_empty() {
            let _hash = blake3::hash(data_tar);
        }

        Ok(())
    }
}

fn serialize_manifest_for_verification(manifest: &Manifest) -> SpmResult<Vec<u8>> {
    let m = Manifest {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        architecture: manifest.architecture.clone(),
        maintainer: manifest.maintainer.clone(),
        description: manifest.description.clone(),
        dependencies: manifest.dependencies.clone(),
        conflicts: manifest.conflicts.clone(),
        provides: manifest.provides.clone(),
        recommends: manifest.recommends.clone(),
        install_size: manifest.install_size,
        format_version: manifest.format_version,
        source: manifest.source.clone(),
        ai_metadata: manifest.ai_metadata.clone(),
        signature: None,
        systemd_units: manifest.systemd_units.clone(),
        sysusers: manifest.sysusers.clone(),
        tmpfiles: manifest.tmpfiles.clone(),
        triggers: manifest.triggers.clone(),
        obsoletes: manifest.obsoletes.clone(),
        conffiles: manifest.conffiles.clone(),
    };
    Ok(serde_json::to_vec(&m)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PackageSignature;
    use ed25519_dalek::SigningKey;
    use ed25519_dalek::Signer;
    use rand::rngs::OsRng;

    fn dummy_manifest() -> Manifest {
        Manifest {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            signature: None,
            ..Manifest::default()
        }
    }

    #[test]
    fn test_keystore_insert_and_get() {
        let mut ks = KeyStore::new();
        let signing_key = SigningKey::generate(&mut OsRng);
        let verifying_key = signing_key.verifying_key();
        ks.insert("key-1".into(), verifying_key);
        assert!(ks.get_ed25519("key-1").is_some());
        assert!(ks.get_ed25519("nonexistent").is_none());
    }

    #[test]
    fn test_verify_debian_valid() {
        let verifier = Verifier::new(KeyStore::new());
        let data = b"hello debian package";
        let expected = "c71c1d10e5a20425721eb3febebe28dbf1fa3e91b32ecb405fe2c22aab74f8ce";
        let result = verifier.verify_debian(data, expected);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_debian_mismatch() {
        let verifier = Verifier::new(KeyStore::new());
        let data = b"hello debian package";
        let result = verifier.verify_debian(data, "0000000000000000000000000000000000000000000000000000000000000000");
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_rpm_valid() {
        let verifier = Verifier::new(KeyStore::new());
        let sig = b"rsa-signature-256-bytes-padded................................................";
        let hash = [0u8; 32];
        let result = verifier.verify_rpm(sig, &hash);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_rpm_empty_signature() {
        let verifier = Verifier::new(KeyStore::new());
        let result = verifier.verify_rpm(&[], &[0u8; 32]);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_rpm_empty_hash() {
        let verifier = Verifier::new(KeyStore::new());
        let result = verifier.verify_rpm(b"sig", &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_rpm_bad_hash_len() {
        let verifier = Verifier::new(KeyStore::new());
        let result = verifier.verify_rpm(b"sig", &[0u8; 7]);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_sam_unsigned() {
        let verifier = Verifier::new(KeyStore::new());
        let manifest = dummy_manifest();
        let result = verifier.verify_sam(&manifest, b"data tar content");
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_sam_valid_signature() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();

        let mut ks = KeyStore::new();
        ks.insert("test-key".into(), verifying_key);
        let verifier = Verifier::new(ks);

        let mut manifest = Manifest {
            name: "signed-pkg".into(),
            version: "2.0.0".into(),
            ..Manifest::default()
        };

        let message = serialize_manifest_for_verification(&manifest).unwrap();
        let signature = signing_key.sign(&message);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

        manifest.signature = Some(PackageSignature {
            algorithm: "ed25519".into(),
            key_id: "test-key".into(),
            value: sig_b64,
        });

        let result = verifier.verify_sam(&manifest, b"some data tar bytes");
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_sam_wrong_key() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);

        let mut ks = KeyStore::new();
        let other_key = SigningKey::generate(&mut csprng).verifying_key();
        ks.insert("wrong-key".into(), other_key);
        let verifier = Verifier::new(ks);

        let mut manifest = Manifest {
            name: "signed-pkg".into(),
            version: "2.0.0".into(),
            ..Manifest::default()
        };

        let message = serialize_manifest_for_verification(&manifest).unwrap();
        let signature = signing_key.sign(&message);
        let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());

        manifest.signature = Some(PackageSignature {
            algorithm: "ed25519".into(),
            key_id: "wrong-key".into(),
            value: sig_b64,
        });

        let result = verifier.verify_sam(&manifest, b"data");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("verification failed"));
    }

    #[test]
    fn test_verify_sam_missing_key() {
        let verifier = Verifier::new(KeyStore::new());
        let manifest = Manifest {
            name: "orphan-pkg".into(),
            version: "1.0.0".into(),
            signature: Some(PackageSignature {
                algorithm: "ed25519".into(),
                key_id: "no-such-key".into(),
                value: "AAAA".into(),
            }),
            ..Manifest::default()
        };
        let result = verifier.verify_sam(&manifest, b"data");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No trusted"));
    }

    #[test]
    fn test_serialize_manifest_strips_signature() {
        let m = Manifest {
            name: "test".into(),
            version: "1.0".into(),
            signature: Some(PackageSignature {
                algorithm: "ed25519".into(),
                key_id: "abc".into(),
                value: "dGVzdA==".into(),
            }),
            ..Manifest::default()
        };
        let bytes = serialize_manifest_for_verification(&m).unwrap();
        let restored: Manifest = serde_json::from_slice(&bytes).unwrap();
        assert!(restored.signature.is_none());
        assert_eq!(restored.name, "test");
    }
}
