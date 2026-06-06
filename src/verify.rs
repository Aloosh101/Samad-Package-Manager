use std::fs;
use std::io::{IsTerminal, Write};

use ed25519_dalek::{Signature, VerifyingKey};
use ed25519_dalek::Verifier;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};
use base64::Engine;

use crate::types::Manifest;
#[allow(unused_imports)]
use crate::types::PackageSignature;

pub(crate) enum SignatureStatus {
    NotSigned,
    NoKey(String),
    Valid,
}

pub(crate) fn verify_manifest_signature(manifest: &Manifest) -> SpmResult<SignatureStatus> {
    let sig = match &manifest.signature {
        Some(s) => s,
        None => return Ok(SignatureStatus::NotSigned),
    };

    let key_path = paths::trusted_keys_dir().join(format!("{}.pub", sig.key_id));
    if !key_path.exists() {
        return Ok(SignatureStatus::NoKey(sig.key_id.clone()));
    }

    let key_bytes = fs::read(&key_path)?;
    let key_arr: [u8; 32] = key_bytes.as_slice().try_into()
        .map_err(|_| SpmError::invalid_format(format!(
            "Trusted key '{}' has invalid length (expected 32 bytes, got {})",
            sig.key_id, key_bytes.len(),
        )))?;
    let verifying_key = VerifyingKey::from_bytes(&key_arr)
        .map_err(|e| SpmError::invalid_format(format!("Invalid Ed25519 key '{}': {e}", sig.key_id)))?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(sig.value.as_bytes())
        .map_err(|e| SpmError::invalid_format(format!("Invalid base64 signature: {e}")))?;
    let signature = Signature::from_slice(&sig_bytes)
        .map_err(|e| SpmError::invalid_format(format!("Invalid signature bytes: {e}")))?;

    let message = serialize_manifest_for_verification(manifest)?;

    verifying_key.verify(&message, &signature)
        .map(|_| Ok(SignatureStatus::Valid))
        .unwrap_or_else(|e| Err(SpmError::other(format!("Signature verification failed for key '{}': {e}", sig.key_id))))
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

pub(crate) fn prompt_before_install(packages: &[&str], deps: &[&str], skip_prompt: bool) -> SpmResult<bool> {
    if skip_prompt || !std::io::stdout().is_terminal() {
        return Ok(true);
    }

    eprintln!("\n  {} Packages to install:", crate::output::bold("Packages:"));
    for pkg in packages {
        eprintln!("    {} {}", crate::output::cyan("⬇"), crate::output::bold(pkg));
    }
    for dep in deps {
        if !packages.contains(dep) {
            eprintln!("      {} {}", crate::output::dim("└─"), dep);
        }
    }

    print!("  {} Continue? [Y/n]: ", crate::output::green("?"));
    let _ = std::io::stdout().flush();

    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    let input = input.trim().to_lowercase();
    Ok(input.is_empty() || input == "y" || input == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_manifest_no_signature() {
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

    #[test]
    fn test_verify_unsigned_manifest() {
        let m = Manifest::default();
        let status = verify_manifest_signature(&m).unwrap();
        assert!(matches!(status, SignatureStatus::NotSigned));
    }

    #[test]
    fn test_prompt_skipped_with_flag() {
        let result = prompt_before_install(&["pkg-a"], &[], true).unwrap();
        assert!(result);
    }
}
