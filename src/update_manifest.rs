//! Legacy Nemorous update-manifest parsing and streaming archive verification.
//!
//! A manifest is JSON with the legacy fields:
//!
//! ```json
//! {
//!   "version": "v1.4.0",
//!   "notes": "release notes",
//!   "url": "https://updates.example/nemorous/archive.bin",
//!   "size": 1234,
//!   "signature": "base64-encoded-ed25519-signature"
//! }
//! ```
//!
//! Additional fields are ignored. The asset signature is a standard Ed25519
//! signature over exactly the 32 raw bytes of SHA-256(archive), matching the
//! MyGo verification behavior.
//!
//! The API is deliberately in-memory only: no network access, download, or
//! installation is performed here.

use std::cmp::Ordering;
use std::io::Read;

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Maximum accepted serialized manifest size: 1 MiB.
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Maximum accepted archive size: 1 GiB.
pub const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

const READ_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Deserialize)]
struct RawManifest {
    version: String,
    #[serde(default)]
    notes: String,
    url: String,
    size: u64,
    signature: String,
}

/// A validated legacy Nemorous update manifest.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The version exactly as it appeared in the manifest.
    pub version: String,
    /// Release notes. Missing manifests default this to an empty string.
    pub notes: String,
    /// Validated HTTPS asset URL.
    pub url: String,
    /// Pinned archive size in bytes.
    pub size: u64,
    /// Decoded standard-base64 Ed25519 signature.
    pub signature: Signature,
    /// Parsed SemVer form of `version`, with one leading `v` or `V` removed.
    pub(crate) parsed_version: Version,
}

impl Manifest {
    /// Checks whether this manifest should be offered over `current_version`.
    pub fn newer_than(
        &self,
        current_version: &str,
        skipped_version: Option<&str>,
        allow_prerelease: bool,
    ) -> Result<bool> {
        newer_than(
            &self.version,
            current_version,
            skipped_version,
            allow_prerelease,
        )
    }

    /// Verifies this manifest's pinned archive and signature.
    pub fn verify_archive<R: Read>(&self, archive: R, key: &VerifyingKey) -> Result<()> {
        verify(archive, self.size, key, &self.signature)
    }
}

/// Parses and validates a serialized legacy Nemorous update manifest.
///
/// Rejects manifests larger than 1 MiB, invalid versions, non-HTTPS URLs,
/// URLs containing embedded credentials, archives larger than 1 GiB, invalid
/// standard base64 signatures, and signatures that are not exactly 64 bytes.
pub fn parse(bytes: &[u8]) -> Result<Manifest> {
    if bytes.len() > MAX_MANIFEST_BYTES {
        bail!(
            "update manifest is {} bytes, exceeding the {} byte limit",
            bytes.len(),
            MAX_MANIFEST_BYTES
        );
    }

    let raw: RawManifest =
        serde_json::from_slice(bytes).context("could not parse update manifest JSON")?;

    let parsed_version = parse_tagged_version(&raw.version)
        .with_context(|| format!("invalid manifest version {:?}", raw.version))?;

    validate_https_url(&raw.url)?;

    if raw.size == 0 || raw.size > MAX_ARCHIVE_BYTES {
        bail!(
            "manifest archive size {} exceeds the {} byte limit",
            raw.size,
            MAX_ARCHIVE_BYTES
        );
    }

    let signature_bytes = BASE64_STANDARD
        .decode(raw.signature.as_bytes())
        .context("manifest signature is not valid standard base64")?;
    let signature = Signature::from_slice(&signature_bytes)
        .context("manifest signature must decode to exactly 64 bytes")?;

    Ok(Manifest {
        version: raw.version,
        notes: raw.notes,
        url: raw.url,
        size: raw.size,
        signature,
        parsed_version,
    })
}

/// Returns whether `candidate_version` should be offered over `current_version`.
///
/// A single leading `v` or `V` is accepted on every version input. Build
/// metadata does not affect precedence. A candidate with the same SemVer
/// precedence as `skipped_version` is never offered. When `allow_prerelease`
/// is false, prerelease candidates are never offered.
pub fn newer_than(
    candidate_version: &str,
    current_version: &str,
    skipped_version: Option<&str>,
    allow_prerelease: bool,
) -> Result<bool> {
    let candidate = parse_tagged_version(candidate_version).context("invalid candidate version")?;
    let current = parse_tagged_version(current_version).context("invalid current version")?;
    let skipped = skipped_version.and_then(|version| parse_tagged_version(version).ok());

    if candidate.cmp_precedence(&current) != Ordering::Greater {
        return Ok(false);
    }

    if !allow_prerelease && !candidate.pre.is_empty() {
        return Ok(false);
    }

    if let Some(skipped) = skipped {
        if candidate.cmp_precedence(&skipped) == Ordering::Equal {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Streams exactly `pinned_size` bytes from `archive`, hashes them with SHA-256,
/// and verifies `signature` against the raw digest bytes.
///
/// Fails if the stream is shorter or longer than `pinned_size`, if
/// `pinned_size` exceeds 1 GiB, or if the signature is invalid.
pub fn verify<R: Read>(
    mut archive: R,
    pinned_size: u64,
    key: &VerifyingKey,
    signature: &Signature,
) -> Result<()> {
    if pinned_size > MAX_ARCHIVE_BYTES {
        bail!(
            "update archive size {} exceeds the maximum of {} bytes",
            pinned_size,
            MAX_ARCHIVE_BYTES
        );
    }

    let mut hasher = Sha256::new();
    let mut remaining = pinned_size;
    let mut buffer = [0u8; READ_BUFFER_BYTES];

    while remaining > 0 {
        let chunk_len = remaining.min(READ_BUFFER_BYTES as u64) as usize;
        let read = archive
            .read(&mut buffer[..chunk_len])
            .context("failed to read update archive")?;
        if read == 0 {
            bail!(
                "update archive is shorter than the pinned size of {} bytes",
                pinned_size
            );
        }
        hasher.update(&buffer[..read]);
        remaining -= read as u64;
    }

    let mut trailing = [0u8; 1];
    if archive
        .read(&mut trailing)
        .context("failed to check update archive length")?
        != 0
    {
        bail!(
            "update archive is larger than the pinned size of {} bytes",
            pinned_size
        );
    }

    let digest = hasher.finalize();
    key.verify_strict(&digest, signature)
        .context("update archive signature verification failed")
}

fn parse_tagged_version(raw: &str) -> Result<Version> {
    let value = raw
        .strip_prefix('v')
        .or_else(|| raw.strip_prefix('V'))
        .unwrap_or(raw);

    if value.is_empty() {
        bail!("version is empty");
    }

    Version::parse(value).with_context(|| format!("invalid semantic version {value:?}"))
}

pub(crate) fn validate_https_url(value: &str) -> Result<()> {
    let uri: ureq::http::Uri = value.parse().context("invalid update URL")?;
    if uri.scheme_str() != Some("https")
        || uri
            .authority()
            .is_none_or(|value| value.as_str().contains('@'))
        || uri.host().is_none()
        || value.contains(['#', '\\'])
    {
        bail!("updates require an HTTPS URL without credentials");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_ARCHIVE_BYTES, MAX_MANIFEST_BYTES, newer_than, parse, verify};
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use ed25519_dalek::{Signature, Signer, SigningKey};
    use semver::Version;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::io::Cursor;

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[0x42; 32])
    }

    fn archive_bytes() -> Vec<u8> {
        b"nemorous synthetic archive fixture".to_vec()
    }

    fn manifest_json(version: &str, url: &str, archive: &[u8], key: &SigningKey) -> Vec<u8> {
        let digest = Sha256::digest(archive);
        let signature = BASE64.encode(key.sign(digest.as_slice()).to_bytes());

        serde_json::to_vec(&json!({
            "version": version,
            "url": url,
            "size": archive.len(),
            "signature": signature,
            "ignored_additional_field": { "synthetic": true }
        }))
        .expect("synthetic manifest must serialize")
    }

    #[test]
    fn parses_and_verifies_a_valid_fixture() {
        let key = signing_key();
        let archive = archive_bytes();
        let manifest = parse(&manifest_json(
            "v1.4.0",
            "https://updates.example/nemorous/archive.bin",
            &archive,
            &key,
        ))
        .expect("valid manifest should parse");

        assert_eq!(manifest.version, "v1.4.0");
        assert_eq!(
            manifest.parsed_version,
            Version::parse("1.4.0").expect("known semver")
        );
        assert!(
            manifest.notes.is_empty(),
            "missing notes should default to an empty string"
        );
        assert_eq!(manifest.size, archive.len() as u64);

        manifest
            .verify_archive(Cursor::new(archive.as_slice()), &key.verifying_key())
            .expect("signed archive should verify");

        verify(
            Cursor::new(archive.as_slice()),
            manifest.size,
            &key.verifying_key(),
            &manifest.signature,
        )
        .expect("top-level verify should agree");
    }

    #[test]
    fn rejects_corrupted_truncated_and_extended_archives() {
        let key = signing_key();
        let archive = archive_bytes();
        let manifest = parse(&manifest_json(
            "v1.4.0",
            "https://updates.example/nemorous/archive.bin",
            &archive,
            &key,
        ))
        .expect("valid manifest should parse");

        let mut corrupted = archive.clone();
        corrupted[0] ^= 0x80;
        assert!(
            manifest
                .verify_archive(Cursor::new(corrupted), &key.verifying_key())
                .is_err()
        );

        assert!(
            manifest
                .verify_archive(
                    Cursor::new(&archive[..archive.len() - 1]),
                    &key.verifying_key(),
                )
                .is_err()
        );

        let mut extended = archive;
        extended.push(0);
        assert!(
            manifest
                .verify_archive(Cursor::new(extended), &key.verifying_key())
                .is_err()
        );
    }

    #[test]
    fn rejects_oversize_manifest_and_archive_fixtures() {
        let key = signing_key();
        let archive = archive_bytes();

        let mut oversized_manifest = manifest_json(
            "v1.4.0",
            "https://updates.example/nemorous/archive.bin",
            &archive,
            &key,
        );
        oversized_manifest.resize(MAX_MANIFEST_BYTES + 1, b' ');
        assert!(parse(&oversized_manifest).is_err());

        let mut manifest: serde_json::Value = serde_json::from_slice(&manifest_json(
            "v1.4.0",
            "https://updates.example/nemorous/archive.bin",
            &archive,
            &key,
        ))
        .expect("fixture must be valid JSON");
        manifest["size"] = json!(MAX_ARCHIVE_BYTES + 1);
        let serialized = serde_json::to_vec(&manifest).expect("fixture must serialize");
        assert!(parse(&serialized).is_err());

        let error = verify(
            Cursor::new(Vec::<u8>::new()),
            MAX_ARCHIVE_BYTES + 1,
            &key.verifying_key(),
            &Signature::from_bytes(&[0u8; 64]),
        )
        .expect_err("oversize pinned size must be rejected before reading");
        assert!(
            error.to_string().contains("exceeds"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn newer_than_respects_v_prefix_skip_and_prerelease_policy() {
        assert!(newer_than("v1.3.0", "1.2.9", None, false).unwrap());
        assert!(!newer_than("v1.2.9", "1.2.9", None, false).unwrap());
        assert!(!newer_than("v1.2.8", "v1.2.9", None, false).unwrap());
        assert!(!newer_than("v1.3.0", "v1.2.9", Some("1.3.0"), false).unwrap());
        assert!(!newer_than("v1.3.0-rc.1", "v1.2.9", None, false).unwrap());
        assert!(newer_than("v1.3.0-rc.1", "v1.2.9", None, true).unwrap());
    }

    #[test]
    fn rejects_insecure_or_credentialed_urls_and_invalid_versions() {
        let key = signing_key();
        let archive = archive_bytes();

        for url in [
            "http://updates.example/nemorous/archive.bin",
            "https://user:secret@updates.example/nemorous/archive.bin",
            "https://updates.example\\@evil.example/nemorous/archive.bin",
        ] {
            let serialized = manifest_json("v1.4.0", url, &archive, &key);
            assert!(parse(&serialized).is_err(), "url must be rejected: {url}");
        }

        let invalid_version = manifest_json(
            "1.4",
            "https://updates.example/nemorous/archive.bin",
            &archive,
            &key,
        );
        assert!(parse(&invalid_version).is_err());
    }
}
