//! Cryptographic utilities for Ed25519 signing.
//!
//! Follows the Nix signing format: `{keyName}:{base64Payload}`

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey};
use rand_core::OsRng;

use crate::error::{WorkerError, WorkerResult};

/// Size of Ed25519 keypair (secret key + public key).
const KEYPAIR_BYTES: usize = 64;

/// Generates a new Ed25519 keypair in Nix format.
///
/// Returns the keypair as `{name}:{base64(secret_key + public_key)}`.
pub fn generate_keypair(name: &str) -> WorkerResult<String> {
    validate_key_name(name)?;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();

    // Nix format: 64 bytes = 32-byte secret key + 32-byte public key
    let mut keypair_bytes = [0u8; KEYPAIR_BYTES];
    keypair_bytes[..32].copy_from_slice(signing_key.as_bytes());
    keypair_bytes[32..].copy_from_slice(verifying_key.as_bytes());

    Ok(format!(
        "{}:{}",
        name,
        BASE64_STANDARD.encode(keypair_bytes)
    ))
}

/// Extracts the public key from a keypair string.
///
/// Input: `{name}:{base64(secret_key + public_key)}`
/// Output: `{name}:{base64(public_key)}`
pub fn extract_public_key(keypair: &str) -> WorkerResult<String> {
    let (name, keypair_bytes) = decode_keypair(keypair)?;

    // Public key is the last 32 bytes
    let public_key = &keypair_bytes[32..64];

    Ok(format!("{}:{}", name, BASE64_STANDARD.encode(public_key)))
}

/// Validates a key name.
///
/// A valid name cannot be empty and must not contain colons.
fn validate_key_name(name: &str) -> WorkerResult<()> {
    if name.is_empty() {
        return Err(WorkerError::BadRequest(
            "Key name cannot be empty".to_string(),
        ));
    }
    if name.contains(':') {
        return Err(WorkerError::BadRequest(
            "Key name cannot contain colons".to_string(),
        ));
    }
    Ok(())
}

/// Decodes a keypair string into (name, bytes).
fn decode_keypair(keypair: &str) -> WorkerResult<(&str, Vec<u8>)> {
    let colon_pos = keypair
        .find(':')
        .ok_or_else(|| WorkerError::BadRequest("Keypair missing colon separator".to_string()))?;

    let (name, payload) = keypair.split_at(colon_pos);
    let payload = &payload[1..]; // Skip the colon

    validate_key_name(name)?;

    let bytes = BASE64_STANDARD
        .decode(payload)
        .map_err(|e| WorkerError::BadRequest(format!("Invalid base64 in keypair: {}", e)))?;

    if bytes.len() != KEYPAIR_BYTES {
        return Err(WorkerError::BadRequest(format!(
            "Invalid keypair length: expected {}, got {}",
            KEYPAIR_BYTES,
            bytes.len()
        )));
    }

    Ok((name, bytes))
}

/// Signs a message with the given keypair.
///
/// Input keypair: `{name}:{base64(secret_key + public_key)}`
/// Output signature: `{name}:{base64(signature)}`
pub fn sign_message(keypair: &str, message: &[u8]) -> WorkerResult<String> {
    let (name, keypair_bytes) = decode_keypair(keypair)?;

    // Extract the 32-byte secret key (first half of keypair)
    let secret_bytes: [u8; 32] = keypair_bytes[..32]
        .try_into()
        .map_err(|_| WorkerError::Internal("Failed to extract secret key".to_string()))?;

    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let signature: Signature = signing_key.sign(message);

    Ok(format!(
        "{}:{}",
        name,
        BASE64_STANDARD.encode(signature.to_bytes())
    ))
}

/// Computes the fingerprint for a store path.
///
/// Format: `1;{storePath};{narHash};{narSize};{commaDelimitedReferences}`
///
/// The narHash must be in typed base32 format (e.g., "sha256:1abc...").
pub fn compute_fingerprint(
    store_path: &str,
    nar_hash: &str,
    nar_size: i64,
    references: &[String],
) -> Vec<u8> {
    let mut fingerprint = b"1;".to_vec();

    // storePath (full path including store dir)
    fingerprint.extend(store_path.as_bytes());
    fingerprint.extend(b";");

    // narHash in typed base32 format
    // If the hash is in hex format (sha256:64hexchars), convert to base32
    let nar_hash_base32 = convert_hash_to_base32(nar_hash);
    fingerprint.extend(nar_hash_base32.as_bytes());
    fingerprint.extend(b";");

    // narSize
    fingerprint.extend(nar_size.to_string().as_bytes());
    fingerprint.extend(b";");

    // commaDelimitedReferences (full paths)
    // References in the object are base names, we need to prepend store dir
    let store_dir = extract_store_dir(store_path);
    let mut iter = references.iter().peekable();
    while let Some(reference) = iter.next() {
        // If reference is already a full path, use it; otherwise prepend store dir
        if reference.starts_with('/') {
            fingerprint.extend(reference.as_bytes());
        } else {
            fingerprint.extend(store_dir.as_bytes());
            fingerprint.extend(b"/");
            fingerprint.extend(reference.as_bytes());
        }

        if iter.peek().is_some() {
            fingerprint.extend(b",");
        }
    }

    fingerprint
}

/// Converts a hash from hex format to Nix base32 format if needed.
///
/// Input: "sha256:64hexchars" or "sha256:52base32chars"
/// Output: "sha256:52base32chars"
///
/// If the hash is already in base32 format (52 chars), it is returned as-is.
/// If the hash is in hex format (64 chars), it is converted to base32.
/// If conversion fails, the original hash is returned unchanged.
pub fn convert_hash_to_base32(hash: &str) -> String {
    // Parse the hash type and value
    let (hash_type, hash_value) = match hash.split_once(':') {
        Some((t, v)) => (t, v),
        None => return hash.to_string(), // Invalid format, return as-is
    };

    // SHA256 hex is 64 chars, base32 is 52 chars
    if hash_value.len() == 64 {
        // This looks like hex, try to convert to base32
        // First verify it's valid hex
        if hash_value.chars().all(|c| c.is_ascii_hexdigit()) {
            match hex::decode(hash_value) {
                Ok(bytes) if bytes.len() == 32 => {
                    let base32 = nix_base32::to_nix_base32(&bytes);
                    format!("{}:{}", hash_type, base32)
                }
                _ => hash.to_string(), // Invalid hex or wrong length, return as-is
            }
        } else {
            hash.to_string() // Not valid hex, return as-is
        }
    } else if hash_value.len() == 52 {
        // Already base32 format, return as-is
        hash.to_string()
    } else {
        // Unknown format, return as-is
        hash.to_string()
    }
}

/// Extracts the store directory from a full store path.
///
/// E.g., "/nix/store/abc123-hello" -> "/nix/store"
fn extract_store_dir(store_path: &str) -> &str {
    // Find the last component (after the last /)
    if let Some(pos) = store_path.rfind('/') {
        &store_path[..pos]
    } else {
        "/nix/store" // Default fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_and_extract() {
        let keypair = generate_keypair("test-cache").unwrap();
        assert!(keypair.starts_with("test-cache:"));

        let public_key = extract_public_key(&keypair).unwrap();
        assert!(public_key.starts_with("test-cache:"));

        // Public key should be shorter (32 bytes vs 64 bytes base64)
        assert!(public_key.len() < keypair.len());
    }

    #[test]
    fn test_invalid_name() {
        assert!(generate_keypair("").is_err());
        assert!(generate_keypair("has:colon").is_err());
    }

    #[test]
    fn test_sign_message() {
        let keypair = generate_keypair("test-cache").unwrap();
        let message = b"test message";

        let signature = sign_message(&keypair, message).unwrap();
        assert!(signature.starts_with("test-cache:"));

        // Signature should be 64 bytes base64 encoded
        let sig_payload = signature.split(':').nth(1).unwrap();
        let sig_bytes = BASE64_STANDARD.decode(sig_payload).unwrap();
        assert_eq!(sig_bytes.len(), 64);
    }

    #[test]
    fn test_compute_fingerprint() {
        let store_path = "/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10";
        let nar_hash = "sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci";
        let nar_size = 206104;
        let references = vec![
            "563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56".to_string(),
            "xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10".to_string(),
        ];

        let fingerprint = compute_fingerprint(store_path, nar_hash, nar_size, &references);
        let fingerprint_str = String::from_utf8(fingerprint).unwrap();

        // Expected format: 1;{storePath};{narHash};{narSize};{refs}
        assert!(fingerprint_str
            .starts_with("1;/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10;"));
        assert!(fingerprint_str.contains(";206104;"));
    }

    #[test]
    fn test_convert_hash_to_base32() {
        // Test hex to base32 conversion
        // This is the hex representation of a known hash
        let hex_hash = "sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let base32 = convert_hash_to_base32(hex_hash);
        assert!(base32.starts_with("sha256:"));
        // Base32 should be 52 chars for SHA256
        let base32_value = base32.split(':').nth(1).unwrap();
        assert_eq!(base32_value.len(), 52);

        // Test already base32 (52 chars)
        let base32_hash = "sha256:0000000000000000000000000000000000000000000000000000";
        let result = convert_hash_to_base32(base32_hash);
        assert_eq!(result, base32_hash);
    }

    #[test]
    fn test_extract_store_dir() {
        assert_eq!(extract_store_dir("/nix/store/abc123-hello"), "/nix/store");
        assert_eq!(
            extract_store_dir("/custom/store/abc123-hello"),
            "/custom/store"
        );
    }
}
