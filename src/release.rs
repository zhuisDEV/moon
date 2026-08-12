use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, SecondsFormat};
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path};

pub const RELEASE_MANIFEST_SCHEMA: u32 = 1;
pub const BUNDLE_MANIFEST_SCHEMA: u32 = 1;
pub const SIGNATURE_SCHEMA: u32 = 1;
pub const MAX_MANIFEST_BYTES: usize = 1024 * 1024;
pub const MAX_SIGNATURE_BYTES: usize = 16 * 1024;
pub const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;

const SUPPORTED_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "x86_64-unknown-linux-gnu",
];

const REQUIRED_PAYLOAD_FILES: &[(&str, u32)] = &[
    ("bin/moon", 0o755),
    ("openclaw-plugin/README.md", 0o644),
    ("openclaw-plugin/index.js", 0o644),
    ("openclaw-plugin/openclaw.plugin.json", 0o644),
    ("openclaw-plugin/package.json", 0o644),
    ("skill/SKILL.md", 0o644),
];

const PRODUCTION_PUBLIC_KEY_DOCUMENT: &[u8] =
    include_bytes!("../assets/release-keys/moon-release-2026-01.pub");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    Stable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    pub mode: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackCompatibility {
    pub previous_release_supported: bool,
    pub database_restore_required_if_schema_changes: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleManifest {
    pub schema_version: u32,
    pub bundle_format: u32,
    pub moon_version: String,
    pub git_tag: String,
    pub git_commit: String,
    pub target: String,
    pub minimum_os_version: String,
    pub adapter_version: String,
    pub skill_version: String,
    pub database_schema_min: i64,
    pub database_schema_max: i64,
    pub openclaw_min_version: String,
    pub rollback: RollbackCompatibility,
    pub files: Vec<BundleFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveDescriptor {
    pub file_name: String,
    pub size: u64,
    pub sha256: String,
    pub bundle_manifest_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAsset {
    pub bundle: BundleManifest,
    pub archive: ArchiveDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub release_channel: ReleaseChannel,
    pub moon_version: String,
    pub git_tag: String,
    pub git_commit: String,
    pub published_at: String,
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSignature {
    pub key_id: String,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureEnvelope {
    pub schema_version: u32,
    pub algorithm: String,
    pub signatures: Vec<ManifestSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    pub key_id: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicKeyDocument {
    pub schema_version: u32,
    pub algorithm: String,
    pub key_id: String,
    pub public_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub manifest: ReleaseManifest,
    pub verified_key_ids: Vec<String>,
}

impl BundleManifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == BUNDLE_MANIFEST_SCHEMA,
            "unsupported bundle manifest schema {}",
            self.schema_version
        );
        ensure!(self.bundle_format == 1, "unsupported bundle format");
        validate_release_identity(&self.moon_version, &self.git_tag, &self.git_commit)?;
        ensure!(
            SUPPORTED_TARGETS.contains(&self.target.as_str()),
            "unsupported release target {}",
            self.target
        );
        validate_bounded_text("minimum_os_version", &self.minimum_os_version, 64)?;
        validate_version("adapter_version", &self.adapter_version)?;
        validate_version("skill_version", &self.skill_version)?;
        ensure!(
            self.adapter_version == self.moon_version,
            "adapter version must match Moon version"
        );
        ensure!(
            self.skill_version == self.moon_version,
            "skill version must match Moon version"
        );
        ensure!(
            self.database_schema_min >= 1 && self.database_schema_min <= self.database_schema_max,
            "invalid database schema compatibility range"
        );
        validate_version("openclaw_min_version", &self.openclaw_min_version)?;
        ensure!(
            self.rollback.previous_release_supported,
            "bundle must support rollback to the previous release"
        );
        ensure!(
            self.rollback.database_restore_required_if_schema_changes,
            "bundle must require database restore when schema changes"
        );
        validate_payload_files(&self.files)
    }

    pub fn verify_payload_file(&self, path: &str, mode: u32, bytes: &[u8]) -> Result<()> {
        let file = self
            .files
            .iter()
            .find(|file| file.path == path)
            .with_context(|| format!("payload file {path} is not declared"))?;
        ensure!(file.mode == mode, "payload file mode mismatch for {path}");
        ensure!(
            file.size == bytes.len() as u64,
            "payload file size mismatch for {path}"
        );
        ensure!(
            file.sha256 == sha256_hex(bytes),
            "payload file checksum mismatch for {path}"
        );
        Ok(())
    }
}

impl ReleaseAsset {
    pub fn validate(&self) -> Result<()> {
        self.bundle.validate()?;
        validate_sha256("archive sha256", &self.archive.sha256)?;
        validate_sha256(
            "bundle manifest sha256",
            &self.archive.bundle_manifest_sha256,
        )?;
        ensure!(
            self.archive.size > 0 && self.archive.size <= MAX_ARCHIVE_BYTES,
            "archive size is outside the allowed range"
        );
        let expected_name = format!(
            "moon-{}-{}.tar.gz",
            self.bundle.moon_version, self.bundle.target
        );
        ensure!(
            self.archive.file_name == expected_name,
            "archive file name does not match release identity"
        );
        ensure!(
            sha256_hex(&encode_bundle_manifest(&self.bundle)?)
                == self.archive.bundle_manifest_sha256,
            "inner bundle manifest hash does not match its signed asset entry"
        );
        Ok(())
    }

    pub fn verify_archive_bytes(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() as u64 == self.archive.size,
            "archive size does not match signed manifest"
        );
        ensure!(
            sha256_hex(bytes) == self.archive.sha256,
            "archive checksum does not match signed manifest"
        );
        Ok(())
    }

    pub fn verify_bundle_manifest_bytes(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            sha256_hex(bytes) == self.archive.bundle_manifest_sha256,
            "inner bundle manifest checksum does not match signed manifest"
        );
        let parsed = parse_bundle_manifest(bytes)?;
        ensure!(
            parsed == self.bundle,
            "inner bundle manifest content does not match signed asset"
        );
        Ok(())
    }
}

impl ReleaseManifest {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == RELEASE_MANIFEST_SCHEMA,
            "unsupported release manifest schema {}",
            self.schema_version
        );
        ensure!(
            self.release_channel == ReleaseChannel::Stable,
            "only the stable release channel is supported"
        );
        validate_release_identity(&self.moon_version, &self.git_tag, &self.git_commit)?;
        let published_at = DateTime::parse_from_rfc3339(&self.published_at)
            .context("published_at must be an RFC 3339 timestamp")?;
        ensure!(
            published_at.to_rfc3339_opts(SecondsFormat::Secs, true) == self.published_at,
            "published_at must use canonical UTC second precision"
        );
        ensure!(!self.assets.is_empty(), "release manifest has no assets");
        ensure!(
            self.assets.len() <= SUPPORTED_TARGETS.len(),
            "release manifest has too many assets"
        );

        let mut targets = BTreeSet::new();
        let mut previous_target: Option<&str> = None;
        for asset in &self.assets {
            asset.validate()?;
            ensure!(
                asset.bundle.moon_version == self.moon_version
                    && asset.bundle.git_tag == self.git_tag
                    && asset.bundle.git_commit == self.git_commit,
                "asset identity does not match release identity"
            );
            ensure!(
                targets.insert(asset.bundle.target.as_str()),
                "release manifest contains duplicate target {}",
                asset.bundle.target
            );
            if let Some(previous) = previous_target {
                ensure!(
                    previous < asset.bundle.target.as_str(),
                    "release assets must be sorted by target"
                );
            }
            previous_target = Some(asset.bundle.target.as_str());
        }
        Ok(())
    }
}

impl SignatureEnvelope {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SIGNATURE_SCHEMA,
            "unsupported signature schema {}",
            self.schema_version
        );
        ensure!(
            self.algorithm == "ed25519",
            "unsupported signature algorithm"
        );
        ensure!(
            !self.signatures.is_empty() && self.signatures.len() <= 4,
            "signature set must contain between one and four signatures"
        );
        let mut key_ids = BTreeSet::new();
        let mut previous_key_id: Option<&str> = None;
        for signature in &self.signatures {
            validate_key_id(&signature.key_id)?;
            ensure!(
                key_ids.insert(signature.key_id.as_str()),
                "signature set contains duplicate key id {}",
                signature.key_id
            );
            decode_hex::<64>(&signature.signature, "Ed25519 signature")?;
            if let Some(previous) = previous_key_id {
                ensure!(
                    previous < signature.key_id.as_str(),
                    "signature set must be sorted by key id"
                );
            }
            previous_key_id = Some(signature.key_id.as_str());
        }
        Ok(())
    }
}

impl TrustedKey {
    pub fn new(key_id: impl Into<String>, public_key: [u8; 32]) -> Result<Self> {
        let key_id = key_id.into();
        validate_key_id(&key_id)?;
        VerifyingKey::from_bytes(&public_key).context("invalid Ed25519 public key")?;
        Ok(Self { key_id, public_key })
    }
}

impl PublicKeyDocument {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SIGNATURE_SCHEMA,
            "unsupported public key schema {}",
            self.schema_version
        );
        ensure!(
            self.algorithm == "ed25519",
            "unsupported public key algorithm"
        );
        validate_key_id(&self.key_id)?;
        let public_key = decode_hex::<32>(&self.public_key, "Ed25519 public key")?;
        VerifyingKey::from_bytes(&public_key).context("invalid Ed25519 public key")?;
        Ok(())
    }

    pub fn trusted_key(&self) -> Result<TrustedKey> {
        self.validate()?;
        TrustedKey::new(
            self.key_id.clone(),
            decode_hex::<32>(&self.public_key, "Ed25519 public key")?,
        )
    }
}

pub fn encode_bundle_manifest(manifest: &BundleManifest) -> Result<Vec<u8>> {
    manifest.validate()?;
    canonical_json(manifest)
}

pub fn encode_release_manifest(manifest: &ReleaseManifest) -> Result<Vec<u8>> {
    manifest.validate()?;
    canonical_json(manifest)
}

pub fn encode_release_asset(asset: &ReleaseAsset) -> Result<Vec<u8>> {
    asset.validate()?;
    canonical_json(asset)
}

pub fn encode_signature_envelope(envelope: &SignatureEnvelope) -> Result<Vec<u8>> {
    envelope.validate()?;
    canonical_json(envelope)
}

pub fn encode_public_key_document(document: &PublicKeyDocument) -> Result<Vec<u8>> {
    document.validate()?;
    canonical_json(document)
}

pub fn parse_bundle_manifest(bytes: &[u8]) -> Result<BundleManifest> {
    ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "bundle manifest exceeds size limit"
    );
    let manifest: BundleManifest =
        serde_json::from_slice(bytes).context("invalid bundle manifest JSON")?;
    manifest.validate()?;
    ensure!(
        encode_bundle_manifest(&manifest)? == bytes,
        "bundle manifest is not in canonical JSON form"
    );
    Ok(manifest)
}

pub fn parse_release_manifest(bytes: &[u8]) -> Result<ReleaseManifest> {
    ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "release manifest exceeds size limit"
    );
    let manifest: ReleaseManifest =
        serde_json::from_slice(bytes).context("invalid release manifest JSON")?;
    manifest.validate()?;
    ensure!(
        encode_release_manifest(&manifest)? == bytes,
        "release manifest is not in canonical JSON form"
    );
    Ok(manifest)
}

pub fn parse_release_asset(bytes: &[u8]) -> Result<ReleaseAsset> {
    ensure!(
        bytes.len() <= MAX_MANIFEST_BYTES,
        "release asset exceeds size limit"
    );
    let asset: ReleaseAsset =
        serde_json::from_slice(bytes).context("invalid release asset JSON")?;
    asset.validate()?;
    ensure!(
        encode_release_asset(&asset)? == bytes,
        "release asset is not in canonical JSON form"
    );
    Ok(asset)
}

pub fn parse_signature_envelope(bytes: &[u8]) -> Result<SignatureEnvelope> {
    ensure!(
        bytes.len() <= MAX_SIGNATURE_BYTES,
        "signature envelope exceeds size limit"
    );
    let envelope: SignatureEnvelope =
        serde_json::from_slice(bytes).context("invalid signature envelope JSON")?;
    envelope.validate()?;
    ensure!(
        encode_signature_envelope(&envelope)? == bytes,
        "signature envelope is not in canonical JSON form"
    );
    Ok(envelope)
}

pub fn parse_public_key_document(bytes: &[u8]) -> Result<PublicKeyDocument> {
    ensure!(
        bytes.len() <= MAX_SIGNATURE_BYTES,
        "public key document exceeds size limit"
    );
    let document: PublicKeyDocument =
        serde_json::from_slice(bytes).context("invalid public key document JSON")?;
    document.validate()?;
    ensure!(
        encode_public_key_document(&document)? == bytes,
        "public key document is not in canonical JSON form"
    );
    Ok(document)
}

pub fn production_trust_roots() -> Result<Vec<TrustedKey>> {
    let document = parse_public_key_document(PRODUCTION_PUBLIC_KEY_DOCUMENT)
        .context("embedded production release key is invalid")?;
    ensure!(
        document.key_id == "moon-release-2026-01",
        "embedded production release key id is unexpected"
    );
    Ok(vec![document.trusted_key()?])
}

pub fn verify_release_manifest(
    manifest_bytes: &[u8],
    signature_bytes: &[u8],
    trusted_keys: &[TrustedKey],
) -> Result<VerifiedManifest> {
    ensure!(
        !trusted_keys.is_empty(),
        "no release trust roots are configured"
    );
    let mut trusted_ids = BTreeSet::new();
    for key in trusted_keys {
        validate_key_id(&key.key_id)?;
        ensure!(
            trusted_ids.insert(key.key_id.as_str()),
            "duplicate trusted key id {}",
            key.key_id
        );
    }

    let envelope = parse_signature_envelope(signature_bytes)?;
    let mut verified_key_ids = Vec::new();
    for detached in envelope.signatures {
        let Some(trusted) = trusted_keys
            .iter()
            .find(|trusted| trusted.key_id == detached.key_id)
        else {
            continue;
        };
        let verifying_key = VerifyingKey::from_bytes(&trusted.public_key)
            .context("invalid trusted Ed25519 public key")?;
        let signature_bytes = decode_hex::<64>(&detached.signature, "Ed25519 signature")?;
        let signature = Signature::try_from(signature_bytes.as_slice())
            .context("invalid Ed25519 signature encoding")?;
        verifying_key
            .verify_strict(manifest_bytes, &signature)
            .with_context(|| format!("signature_invalid for key {}", trusted.key_id))?;
        verified_key_ids.push(trusted.key_id.clone());
    }
    ensure!(
        !verified_key_ids.is_empty(),
        "signature_invalid: no trusted signature verified"
    );

    let manifest = parse_release_manifest(manifest_bytes)?;
    Ok(VerifiedManifest {
        manifest,
        verified_key_ids,
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn canonical_json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn validate_release_identity(version: &str, tag: &str, commit: &str) -> Result<()> {
    validate_version("moon_version", version)?;
    ensure!(
        tag == format!("v{version}"),
        "Git tag must be v<moon_version>"
    );
    ensure!(
        (40..=64).contains(&commit.len()) && commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "git_commit must be a full hexadecimal commit id"
    );
    ensure!(
        commit.bytes().all(|byte| !byte.is_ascii_uppercase()),
        "git_commit must use lowercase hexadecimal"
    );
    Ok(())
}

fn validate_version(label: &str, value: &str) -> Result<()> {
    let version =
        Version::parse(value).with_context(|| format!("{label} must be a semantic version"))?;
    ensure!(
        version.to_string() == value,
        "{label} must use canonical semantic-version syntax"
    );
    Ok(())
}

fn validate_bounded_text(label: &str, value: &str, maximum: usize) -> Result<()> {
    ensure!(
        !value.is_empty() && value.len() <= maximum,
        "{label} must contain between one and {maximum} bytes"
    );
    ensure!(
        !value.chars().any(char::is_control),
        "{label} must not contain control characters"
    );
    Ok(())
}

fn validate_key_id(key_id: &str) -> Result<()> {
    validate_bounded_text("key_id", key_id, 64)?;
    ensure!(
        key_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        }),
        "key_id contains unsupported characters"
    );
    Ok(())
}

fn validate_payload_files(files: &[BundleFile]) -> Result<()> {
    ensure!(
        files.len() == REQUIRED_PAYLOAD_FILES.len(),
        "bundle contains an unexpected number of payload files"
    );
    let mut paths = BTreeSet::new();
    for file in files {
        validate_archive_path(&file.path)?;
        ensure!(
            paths.insert(file.path.as_str()),
            "duplicate bundle file path"
        );
        ensure!(
            file.size > 0 && file.size <= MAX_ARCHIVE_BYTES,
            "bundle file size is outside the allowed range"
        );
        validate_sha256("bundle file sha256", &file.sha256)?;
        let Some((_, expected_mode)) = REQUIRED_PAYLOAD_FILES
            .iter()
            .find(|(path, _)| *path == file.path)
        else {
            bail!("unexpected bundle file path {}", file.path);
        };
        ensure!(file.mode == *expected_mode, "unexpected bundle file mode");
    }
    for (required, _) in REQUIRED_PAYLOAD_FILES {
        ensure!(
            paths.contains(required),
            "missing required bundle file {required}"
        );
    }
    ensure!(
        files
            .iter()
            .map(|file| file.path.as_str())
            .eq(REQUIRED_PAYLOAD_FILES.iter().map(|(path, _)| *path)),
        "bundle files must use canonical path order"
    );
    Ok(())
}

fn validate_archive_path(value: &str) -> Result<()> {
    ensure!(
        !value.contains('\\'),
        "archive paths must use forward slashes"
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "archive path must be relative");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "archive path contains an unsafe component"
    );
    Ok(())
}

fn validate_sha256(label: &str, value: &str) -> Result<()> {
    ensure!(
        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{label} must contain 64 hexadecimal characters"
    );
    ensure!(
        value.bytes().all(|byte| !byte.is_ascii_uppercase()),
        "{label} must use lowercase hexadecimal"
    );
    Ok(())
}

fn decode_hex<const N: usize>(value: &str, label: &str) -> Result<[u8; N]> {
    let mut bytes = [0_u8; N];
    hex::decode_to_slice(value, &mut bytes).with_context(|| format!("invalid {label}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn sample_bundle() -> BundleManifest {
        let files = REQUIRED_PAYLOAD_FILES
            .iter()
            .enumerate()
            .map(|(index, (path, mode))| BundleFile {
                path: (*path).to_owned(),
                size: index as u64 + 1,
                sha256: format!("{index:064x}"),
                mode: *mode,
            })
            .collect();
        BundleManifest {
            schema_version: BUNDLE_MANIFEST_SCHEMA,
            bundle_format: 1,
            moon_version: "2.2.0".to_owned(),
            git_tag: "v2.2.0".to_owned(),
            git_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            target: "aarch64-apple-darwin".to_owned(),
            minimum_os_version: "13.0".to_owned(),
            adapter_version: "2.2.0".to_owned(),
            skill_version: "2.2.0".to_owned(),
            database_schema_min: 6,
            database_schema_max: 6,
            openclaw_min_version: "2026.7.1".to_owned(),
            rollback: RollbackCompatibility {
                previous_release_supported: true,
                database_restore_required_if_schema_changes: true,
            },
            files,
        }
    }

    fn sample_manifest() -> ReleaseManifest {
        let bundle = sample_bundle();
        let bundle_hash = sha256_hex(&encode_bundle_manifest(&bundle).expect("bundle"));
        ReleaseManifest {
            schema_version: RELEASE_MANIFEST_SCHEMA,
            release_channel: ReleaseChannel::Stable,
            moon_version: bundle.moon_version.clone(),
            git_tag: bundle.git_tag.clone(),
            git_commit: bundle.git_commit.clone(),
            published_at: "2026-08-12T00:00:00Z".to_owned(),
            assets: vec![ReleaseAsset {
                archive: ArchiveDescriptor {
                    file_name: "moon-2.2.0-aarch64-apple-darwin.tar.gz".to_owned(),
                    size: 100,
                    sha256: "a".repeat(64),
                    bundle_manifest_sha256: bundle_hash,
                },
                bundle,
            }],
        }
    }

    fn signed_fixture(manifest_bytes: &[u8]) -> (Vec<u8>, TrustedKey) {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let signature = signing_key.sign(manifest_bytes);
        let envelope = SignatureEnvelope {
            schema_version: SIGNATURE_SCHEMA,
            algorithm: "ed25519".to_owned(),
            signatures: vec![ManifestSignature {
                key_id: "test-release-1".to_owned(),
                signature: hex::encode(signature.to_bytes()),
            }],
        };
        let trusted = TrustedKey::new("test-release-1", signing_key.verifying_key().to_bytes())
            .expect("trusted key");
        (
            encode_signature_envelope(&envelope).expect("signature envelope"),
            trusted,
        )
    }

    #[test]
    fn canonical_manifests_round_trip_strictly() {
        let manifest = sample_manifest();
        let bytes = encode_release_manifest(&manifest).expect("encode");
        assert_eq!(parse_release_manifest(&bytes).expect("parse"), manifest);

        let pretty = serde_json::to_vec_pretty(&manifest).expect("pretty");
        assert!(parse_release_manifest(&pretty).is_err());
    }

    #[test]
    fn signed_manifest_verifies_with_a_trusted_key() {
        let bytes = encode_release_manifest(&sample_manifest()).expect("manifest");
        let (signature, trusted) = signed_fixture(&bytes);
        let verified = verify_release_manifest(&bytes, &signature, &[trusted]).expect("verify");
        assert_eq!(verified.manifest.moon_version, "2.2.0");
        assert_eq!(verified.verified_key_ids, ["test-release-1"]);
    }

    #[test]
    fn production_release_key_is_embedded_and_valid() {
        let keys = production_trust_roots().expect("production trust roots");
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "moon-release-2026-01");
        assert_eq!(
            hex::encode(keys[0].public_key),
            "66628fc3a3414df02c5ef5f4cd92aeaa09dbd6a79e68ed84455ef8d21bc4eec7"
        );
    }

    #[test]
    fn altered_or_unknown_signatures_are_rejected() {
        let bytes = encode_release_manifest(&sample_manifest()).expect("manifest");
        let (signature, trusted) = signed_fixture(&bytes);
        let mut altered = bytes.clone();
        altered[20] ^= 1;
        assert!(
            verify_release_manifest(&altered, &signature, std::slice::from_ref(&trusted)).is_err()
        );

        let other = SigningKey::from_bytes(&[8_u8; 32]);
        let unknown = TrustedKey::new("other-release-1", other.verifying_key().to_bytes())
            .expect("other key");
        assert!(verify_release_manifest(&bytes, &signature, &[unknown]).is_err());
    }

    #[test]
    fn signed_but_noncanonical_manifest_is_rejected() {
        let pretty = serde_json::to_vec_pretty(&sample_manifest()).expect("pretty");
        let (signature, trusted) = signed_fixture(&pretty);
        let error = verify_release_manifest(&pretty, &signature, &[trusted])
            .expect_err("noncanonical JSON must fail");
        assert!(error.to_string().contains("canonical"));
    }

    #[test]
    fn unsafe_or_incomplete_payloads_are_rejected() {
        let mut bundle = sample_bundle();
        bundle.files[0].path = "../moon".to_owned();
        assert!(bundle.validate().is_err());

        let mut bundle = sample_bundle();
        bundle.files.pop();
        assert!(bundle.validate().is_err());

        let mut bundle = sample_bundle();
        bundle.files[0].mode = 0o644;
        assert!(bundle.validate().is_err());
    }

    #[test]
    fn signed_asset_checks_archive_bundle_and_payload_bytes() {
        let manifest = sample_manifest();
        let asset = &manifest.assets[0];
        let bundle_bytes = encode_bundle_manifest(&asset.bundle).expect("bundle");
        asset
            .verify_bundle_manifest_bytes(&bundle_bytes)
            .expect("bundle bytes");
        let payload = vec![0_u8; asset.bundle.files[0].size as usize];
        assert!(
            asset
                .bundle
                .verify_payload_file(
                    &asset.bundle.files[0].path,
                    asset.bundle.files[0].mode,
                    &payload,
                )
                .is_err(),
            "fixture hash intentionally differs"
        );

        let archive = vec![0_u8; asset.archive.size as usize];
        assert!(asset.verify_archive_bytes(&archive).is_err());
    }
}
