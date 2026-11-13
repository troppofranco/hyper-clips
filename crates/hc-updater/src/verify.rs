//! SHA-256 and minisign verification. An update is installed only if both pass.

use crate::{Result, UpdateError, PUBLIC_KEY};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parses `sha256sum`-style text: `<64 hex>  <file name>` per line.
pub fn parse_checksums(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim().split_once(char::is_whitespace)?;
            let name = name.trim().trim_start_matches('*');
            (hash.len() == 64 && hash.chars().all(|c| c.is_ascii_hexdigit()) && !name.is_empty())
                .then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

/// Checks `data` against the entry for `file_name` in a checksums file.
pub fn verify_checksum(data: &[u8], file_name: &str, checksums_text: &str) -> Result<()> {
    let expected = parse_checksums(checksums_text)
        .remove(file_name)
        .ok_or_else(|| {
            UpdateError::Verify(format!("{file_name} is not listed in checksums.txt"))
        })?;
    let actual = sha256_hex(data);
    if actual == expected {
        Ok(())
    } else {
        Err(UpdateError::Verify(format!(
            "SHA-256 mismatch for {file_name}"
        )))
    }
}

/// The embedded trusted key, or `NoKey` while the project has not set one.
pub fn trusted_key() -> Result<&'static str> {
    let key = PUBLIC_KEY.trim();
    if key.is_empty() {
        Err(UpdateError::NoKey)
    } else {
        Ok(key)
    }
}

/// Verifies a minisign signature of `data` with the given public key text
/// (the two-line `.pub` file, or just the base64 key line).
pub fn verify_signature_with(public_key: &str, signature_text: &str, data: &[u8]) -> Result<()> {
    let key_line = public_key
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("untrusted comment"))
        .ok_or_else(|| UpdateError::Verify("empty public key".into()))?;
    let key = minisign_verify::PublicKey::from_base64(key_line)
        .map_err(|e| UpdateError::Verify(format!("bad public key: {e}")))?;
    let sig = minisign_verify::Signature::decode(signature_text)
        .map_err(|e| UpdateError::Verify(format!("bad signature file: {e}")))?;
    key.verify(data, &sig, true)
        .map_err(|e| UpdateError::Verify(format!("signature does not match: {e}")))
}

/// Verifies `data` against the key embedded in this build.
pub fn verify_signature(signature_text: &str, data: &[u8]) -> Result<()> {
    verify_signature_with(trusted_key()?, signature_text, data)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sha256_known_vectors() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn checksum_file_is_parsed_and_enforced() {
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        let text = format!("{abc}  HyperClips-Setup-1.exe\nnot a hash line\n{abc} *other.zip\n");
        let map = parse_checksums(&text);
        assert_eq!(map.len(), 2);
        assert!(verify_checksum(b"abc", "HyperClips-Setup-1.exe", &text).is_ok());
        assert!(verify_checksum(b"abd", "HyperClips-Setup-1.exe", &text).is_err());
        assert!(verify_checksum(b"abc", "unlisted.exe", &text).is_err());
    }

    #[test]
    fn signature_garbage_is_rejected_never_accepted() {
        assert!(verify_signature_with("", "x", b"data").is_err());
        assert!(verify_signature_with("RWQnotavalidkey", "x", b"data").is_err());
        assert!(verify_signature_with(
            "untrusted comment: k\nRWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3",
            "not a signature",
            b"data"
        )
        .is_err());
    }

    #[test]
    fn empty_embedded_key_disables_updates() {
        // The repository ships with an empty key file until the owner sets one.
        if PUBLIC_KEY.trim().is_empty() {
            assert!(matches!(trusted_key(), Err(UpdateError::NoKey)));
        }
    }
}
