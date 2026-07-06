use crate::error::SpmResult;

/// Verify a PGP clearsign signature on a Debian InRelease file.
/// Returns true if any of the trusted keys successfully verifies the signature.
#[allow(dead_code)]
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
#[allow(dead_code)]
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

#[cfg(test)]
mod tests {
}
