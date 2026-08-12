use anyhow::{Context, Result, ensure};
use clap::{Args, Parser, Subcommand};
use flate2::write::GzEncoder;
use flate2::{Compression, GzBuilder};
use moon::release::{
    ArchiveDescriptor, BUNDLE_MANIFEST_SCHEMA, BundleFile, BundleManifest, MAX_ARCHIVE_BYTES,
    RELEASE_MANIFEST_SCHEMA, ReleaseAsset, ReleaseChannel, ReleaseManifest, RollbackCompatibility,
    encode_bundle_manifest, encode_release_asset, encode_release_manifest, parse_release_asset,
    production_trust_roots, sha256_hex, verify_release_manifest,
};
#[cfg(target_os = "macos")]
use moon::release::{
    ManifestSignature, PublicKeyDocument, SIGNATURE_SCHEMA, SignatureEnvelope,
    encode_public_key_document, encode_signature_envelope, parse_public_key_document,
    parse_release_manifest,
};
use moon::version::{BUNDLE_FORMAT, VersionInfo};
use serde::Deserialize;
use std::fs::{self, OpenOptions};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::{Builder, EntryType, Header, HeaderMode};

#[cfg(target_os = "macos")]
use core_foundation::array::CFArray;
#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
#[cfg(target_os = "macos")]
use ed25519_dalek::{Signer, SigningKey};
#[cfg(target_os = "macos")]
use security_framework::os::macos::access::SecAccess;
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain::SecKeychain;
#[cfg(target_os = "macos")]
use security_framework::os::macos::keychain_item::SecKeychainItem;
#[cfg(target_os = "macos")]
use security_framework::random::SecRandom;
#[cfg(target_os = "macos")]
use security_framework_sys::base::{
    SecAccessRef, SecKeychainItemRef, SecKeychainRef, errSecSuccess,
};
#[cfg(target_os = "macos")]
use zeroize::Zeroize;

const MAX_VERSION_OUTPUT_BYTES: usize = 64 * 1024;
#[cfg(target_os = "macos")]
const KEYCHAIN_SERVICE: &str = "dev.zhuis.moon.release-signing";
#[cfg(target_os = "macos")]
const DEFAULT_RELEASE_KEY_ID: &str = "moon-release-2026-01";

#[cfg(target_os = "macos")]
#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SecAccessCreate(
        descriptor: *const std::ffi::c_void,
        trusted_list: *const std::ffi::c_void,
        access: *mut SecAccessRef,
    ) -> i32;
    fn SecKeychainItemSetAccess(item: SecKeychainItemRef, access: SecAccessRef) -> i32;
    fn SecKeychainLock(keychain: SecKeychainRef) -> i32;
}

#[derive(Debug, Parser)]
#[command(
    name = "moon-release",
    about = "Build and verify deterministic Moon release inputs"
)]
struct Cli {
    #[command(subcommand)]
    command: ReleaseCommand,
}

#[derive(Debug, Subcommand)]
enum ReleaseCommand {
    /// Build one deterministic platform archive and canonical asset descriptor.
    Bundle(BundleArgs),
    /// Assemble canonical platform assets into one unsigned release manifest.
    Manifest(ManifestArgs),
    /// Verify a canonical manifest using Moon's embedded production trust roots.
    Verify(VerifyArgs),
    /// Generate one Ed25519 release key directly inside a macOS keychain.
    #[cfg(target_os = "macos")]
    Keygen(KeygenArgs),
    /// Sign a canonical release manifest with a key protected by macOS Keychain.
    #[cfg(target_os = "macos")]
    Sign(SignArgs),
}

#[derive(Debug, Args)]
struct BundleArgs {
    #[arg(long)]
    binary: PathBuf,
    #[arg(long, default_value = "assets/openclaw-plugin")]
    adapter_dir: PathBuf,
    #[arg(long, default_value = "SKILL.md")]
    skill: PathBuf,
    #[arg(long)]
    output_dir: PathBuf,
    #[arg(long)]
    minimum_os_version: String,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    git_commit: Option<String>,
    #[arg(long)]
    target: Option<String>,
    #[arg(long, default_value_t = 7)]
    database_schema_min: i64,
    #[arg(long, default_value_t = 7)]
    database_schema_max: i64,
    #[arg(long, default_value = "2026.7.1")]
    openclaw_min_version: String,
    /// Permit development fixtures from dirty or unverifiable source state.
    #[arg(long)]
    allow_dirty: bool,
}

#[derive(Debug, Args)]
struct ManifestArgs {
    #[arg(long, required = true)]
    asset: Vec<PathBuf>,
    #[arg(long)]
    published_at: String,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Debug, Args)]
struct VerifyArgs {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    signature: PathBuf,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Args)]
struct KeygenArgs {
    #[arg(long)]
    keychain: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_RELEASE_KEY_ID)]
    key_id: String,
    #[arg(long)]
    public_key_output: PathBuf,
}

#[cfg(target_os = "macos")]
#[derive(Debug, Args)]
struct SignArgs {
    #[arg(long)]
    manifest: PathBuf,
    #[arg(long)]
    signature_output: PathBuf,
    #[arg(long)]
    keychain: Option<PathBuf>,
    #[arg(long, default_value = DEFAULT_RELEASE_KEY_ID)]
    key_id: String,
    #[arg(long, default_value = "assets/release-keys/moon-release-2026-01.pub")]
    public_key: PathBuf,
}

#[derive(Debug, Deserialize)]
struct AdapterPackage {
    name: String,
    version: String,
}

#[derive(Debug, Clone)]
struct PayloadFile {
    path: &'static str,
    mode: u32,
    bytes: Vec<u8>,
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("moon-release error: {error:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        ReleaseCommand::Bundle(args) => build_bundle(args),
        ReleaseCommand::Manifest(args) => build_manifest(args),
        ReleaseCommand::Verify(args) => verify_manifest(args),
        #[cfg(target_os = "macos")]
        ReleaseCommand::Keygen(args) => generate_key(args),
        #[cfg(target_os = "macos")]
        ReleaseCommand::Sign(args) => sign_manifest(args),
    }
}

#[cfg(target_os = "macos")]
struct UnlockedKeychain {
    inner: SecKeychain,
    locked: bool,
}

#[cfg(target_os = "macos")]
impl UnlockedKeychain {
    fn open(path: &Path) -> Result<Self> {
        ensure_regular_file(path)?;
        let mut inner = SecKeychain::open(path)
            .with_context(|| format!("failed to open keychain {}", path.display()))?;
        inner
            .unlock(None)
            .context("failed to unlock release signing keychain")?;
        Ok(Self {
            inner,
            locked: false,
        })
    }

    fn keychain(&self) -> &SecKeychain {
        &self.inner
    }

    fn lock(&mut self) -> Result<()> {
        let status = unsafe { SecKeychainLock(self.inner.as_concrete_TypeRef()) };
        ensure!(
            status == errSecSuccess,
            "failed to re-lock release signing keychain with status {status}"
        );
        self.locked = true;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for UnlockedKeychain {
    fn drop(&mut self) {
        // This is intentionally best-effort in Drop: the operation cannot
        // return an error, but the keychain also has a five-minute timeout and
        // lock-on-sleep policy as a second boundary.
        if !self.locked {
            unsafe {
                SecKeychainLock(self.inner.as_concrete_TypeRef());
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn generate_key(args: KeygenArgs) -> Result<()> {
    ensure!(
        !args.public_key_output.exists(),
        "refusing to overwrite {}",
        args.public_key_output.display()
    );
    let keychain_path = args.keychain.unwrap_or(default_release_keychain()?);
    let mut keychain = UnlockedKeychain::open(&keychain_path)?;

    let mut seed = [0_u8; 32];
    SecRandom::default()
        .copy_bytes(&mut seed)
        .context("failed to generate secure key material")?;
    let signing_key = SigningKey::from_bytes(&seed);
    seed.zeroize();
    let public_key = signing_key.verifying_key().to_bytes();
    let document = PublicKeyDocument {
        schema_version: SIGNATURE_SCHEMA,
        algorithm: "ed25519".to_owned(),
        key_id: args.key_id.clone(),
        public_key: hex::encode(public_key),
    };
    let public_bytes = encode_public_key_document(&document)?;

    keychain
        .keychain()
        .add_generic_password(KEYCHAIN_SERVICE, &args.key_id, signing_key.as_bytes())
        .context("failed to add release key; the key id may already exist")?;
    let stored = keychain
        .keychain()
        .find_generic_password(KEYCHAIN_SERVICE, &args.key_id)
        .context("failed to verify stored release key")?;
    drop(stored.0);
    if let Err(error) = require_confirmation(&stored.1) {
        stored.1.delete();
        return Err(error);
    }
    if let Err(error) = write_new_public_file(&args.public_key_output, &public_bytes) {
        stored.1.delete();
        return Err(error);
    }
    keychain.lock()?;

    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "key_id": args.key_id,
            "algorithm": "ed25519",
            "public_key": document.public_key,
            "public_key_file": args.public_key_output,
            "keychain": keychain_path,
            "private_key_exported": false,
            "interactive_confirmation_required": true,
        })
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn sign_manifest(args: SignArgs) -> Result<()> {
    ensure!(
        !args.signature_output.exists(),
        "refusing to overwrite {}",
        args.signature_output.display()
    );
    let manifest_bytes = read_bounded_file(&args.manifest)?;
    let manifest = parse_release_manifest(&manifest_bytes)
        .with_context(|| format!("invalid manifest {}", args.manifest.display()))?;
    let public_bytes = read_bounded_file(&args.public_key)?;
    let public_document = parse_public_key_document(&public_bytes)
        .with_context(|| format!("invalid public key {}", args.public_key.display()))?;
    ensure!(
        public_document.key_id == args.key_id,
        "public key id does not match requested signing key"
    );

    let keychain_path = args.keychain.unwrap_or(default_release_keychain()?);
    let mut keychain = UnlockedKeychain::open(&keychain_path)?;
    let (password, _) = keychain
        .keychain()
        .find_generic_password(KEYCHAIN_SERVICE, &args.key_id)
        .with_context(|| format!("release signing key {} was not found", args.key_id))?;
    ensure!(
        password.len() == 32,
        "stored release signing key is invalid"
    );
    let mut seed = [0_u8; 32];
    seed.copy_from_slice(password.as_ref());
    drop(password);
    let signing_key = SigningKey::from_bytes(&seed);
    seed.zeroize();
    ensure!(
        hex::encode(signing_key.verifying_key().to_bytes()) == public_document.public_key,
        "stored private key does not match the approved public key"
    );

    let detached = signing_key.sign(&manifest_bytes);
    let envelope = SignatureEnvelope {
        schema_version: SIGNATURE_SCHEMA,
        algorithm: "ed25519".to_owned(),
        signatures: vec![ManifestSignature {
            key_id: args.key_id.clone(),
            signature: hex::encode(detached.to_bytes()),
        }],
    };
    let signature_bytes = encode_signature_envelope(&envelope)?;
    let trusted_key = public_document.trusted_key()?;
    let verified = verify_release_manifest(&manifest_bytes, &signature_bytes, &[trusted_key])?;
    ensure!(
        verified.manifest == manifest,
        "post-signature manifest verification changed content"
    );

    let output_parent = args
        .signature_output
        .parent()
        .context("signature output path has no parent")?;
    ensure_owner_only_directory(output_parent)?;
    write_new_file(&args.signature_output, &signature_bytes)?;
    keychain.lock()?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "manifest": args.manifest,
            "manifest_sha256": sha256_hex(&manifest_bytes),
            "signature": args.signature_output,
            "key_id": args.key_id,
            "verified_key_ids": verified.verified_key_ids,
            "private_key_exported": false,
            "keychain_relocked_on_exit": true,
        })
    );
    Ok(())
}

fn verify_manifest(args: VerifyArgs) -> Result<()> {
    let manifest_bytes = read_bounded_file(&args.manifest)?;
    let signature_bytes = read_bounded_file(&args.signature)?;
    let verified = verify_release_manifest(
        &manifest_bytes,
        &signature_bytes,
        &production_trust_roots()?,
    )?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "manifest": args.manifest,
            "manifest_sha256": sha256_hex(&manifest_bytes),
            "moon_version": verified.manifest.moon_version,
            "git_commit": verified.manifest.git_commit,
            "targets": verified.manifest.assets.iter()
                .map(|asset| asset.bundle.target.as_str())
                .collect::<Vec<_>>(),
            "verified_key_ids": verified.verified_key_ids,
        })
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn default_release_keychain() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| {
            home.join("Library/Keychains")
                .join("moon-release-signing.keychain-db")
        })
        .context("home directory could not be resolved")
}

#[cfg(target_os = "macos")]
fn require_confirmation(item: &SecKeychainItem) -> Result<()> {
    let descriptor = CFString::new("Moon Release Signing Key");
    let trusted_applications = CFArray::<CFString>::from_CFTypes(&[]);
    let mut access_ref = std::ptr::null_mut();
    // An empty trusted-application list makes every private-key read require
    // explicit Keychain confirmation instead of trusting a mutable file path.
    let status = unsafe {
        SecAccessCreate(
            descriptor.as_CFTypeRef().cast(),
            trusted_applications.as_CFTypeRef().cast(),
            &mut access_ref,
        )
    };
    ensure!(
        status == errSecSuccess,
        "failed to create restrictive key access with status {status}"
    );
    // SecAccessCreate returns an owned reference.
    let access = unsafe { SecAccess::wrap_under_create_rule(access_ref) };
    let status = unsafe {
        SecKeychainItemSetAccess(item.as_concrete_TypeRef(), access.as_concrete_TypeRef())
    };
    ensure!(
        status == errSecSuccess,
        "failed to apply restrictive key access with status {status}"
    );
    Ok(())
}

fn build_bundle(args: BundleArgs) -> Result<()> {
    let version = args
        .version
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned());
    let git_commit = args
        .git_commit
        .unwrap_or_else(|| env!("MOON_GIT_COMMIT").to_owned());
    let target = args
        .target
        .unwrap_or_else(|| env!("MOON_BUILD_TARGET").to_owned());
    let identity = inspect_binary(&args.binary)?;
    validate_binary_identity(&identity, &version, &git_commit, &target, args.allow_dirty)?;

    let payload = load_payload(&args.binary, &args.adapter_dir, &args.skill, &version)?;
    let files = payload
        .iter()
        .map(|file| BundleFile {
            path: file.path.to_owned(),
            size: file.bytes.len() as u64,
            sha256: sha256_hex(&file.bytes),
            mode: file.mode,
        })
        .collect();
    let bundle = BundleManifest {
        schema_version: BUNDLE_MANIFEST_SCHEMA,
        bundle_format: BUNDLE_FORMAT,
        moon_version: version.clone(),
        git_tag: format!("v{version}"),
        git_commit: git_commit.clone(),
        target: target.clone(),
        minimum_os_version: args.minimum_os_version,
        adapter_version: version.clone(),
        skill_version: version.clone(),
        database_schema_min: args.database_schema_min,
        database_schema_max: args.database_schema_max,
        openclaw_min_version: args.openclaw_min_version,
        rollback: RollbackCompatibility {
            previous_release_supported: true,
            database_restore_required_if_schema_changes: true,
        },
        files,
    };
    let bundle_bytes = encode_bundle_manifest(&bundle)?;
    let archive_bytes = build_archive(&payload, &bundle_bytes)?;
    let second_archive = build_archive(&payload, &bundle_bytes)?;
    ensure!(
        archive_bytes == second_archive,
        "release archive generation is not reproducible"
    );

    let archive_name = format!("moon-{version}-{target}.tar.gz");
    let asset = ReleaseAsset {
        bundle,
        archive: ArchiveDescriptor {
            file_name: archive_name.clone(),
            size: archive_bytes.len() as u64,
            sha256: sha256_hex(&archive_bytes),
            bundle_manifest_sha256: sha256_hex(&bundle_bytes),
        },
    };
    let asset_bytes = encode_release_asset(&asset)?;

    prepare_owner_only_directory(&args.output_dir)?;
    let archive_path = args.output_dir.join(&archive_name);
    let asset_path = args.output_dir.join("release-asset.json");
    ensure!(
        !archive_path.exists(),
        "refusing to overwrite {}",
        archive_path.display()
    );
    ensure!(
        !asset_path.exists(),
        "refusing to overwrite {}",
        asset_path.display()
    );
    write_new_file(&archive_path, &archive_bytes)?;
    if let Err(error) = write_new_file(&asset_path, &asset_bytes) {
        let _ = fs::remove_file(&archive_path);
        return Err(error);
    }

    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "archive": archive_path,
            "asset": asset_path,
            "archive_sha256": asset.archive.sha256,
            "bundle_manifest_sha256": asset.archive.bundle_manifest_sha256,
        })
    );
    Ok(())
}

fn build_manifest(args: ManifestArgs) -> Result<()> {
    ensure!(
        !args.output.exists(),
        "refusing to overwrite {}",
        args.output.display()
    );
    let output_parent = args.output.parent().context("output path has no parent")?;
    ensure_owner_only_directory(output_parent)?;
    let mut assets = args
        .asset
        .iter()
        .map(|path| {
            let bytes = read_bounded_file(path)?;
            parse_release_asset(&bytes).with_context(|| format!("invalid asset {}", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    assets.sort_by(|left, right| left.bundle.target.cmp(&right.bundle.target));
    let first = assets.first().context("at least one asset is required")?;
    let manifest = ReleaseManifest {
        schema_version: RELEASE_MANIFEST_SCHEMA,
        release_channel: ReleaseChannel::Stable,
        moon_version: first.bundle.moon_version.clone(),
        git_tag: first.bundle.git_tag.clone(),
        git_commit: first.bundle.git_commit.clone(),
        published_at: args.published_at,
        assets,
    };
    let bytes = encode_release_manifest(&manifest)?;
    write_new_file(&args.output, &bytes)?;
    println!(
        "{}",
        serde_json::json!({
            "ok": true,
            "manifest": args.output,
            "sha256": sha256_hex(&bytes),
            "signed": false,
        })
    );
    Ok(())
}

fn inspect_binary(binary: &Path) -> Result<VersionInfo> {
    ensure_regular_file(binary)?;
    let output = Command::new(binary)
        .args(["--json", "--version"])
        .output()
        .with_context(|| format!("failed to execute {}", binary.display()))?;
    ensure!(
        output.status.success(),
        "candidate binary version check failed"
    );
    ensure!(
        output.stderr.is_empty(),
        "candidate binary wrote version diagnostics"
    );
    ensure!(
        output.stdout.len() <= MAX_VERSION_OUTPUT_BYTES,
        "candidate binary version output exceeds size limit"
    );
    serde_json::from_slice(&output.stdout).context("candidate binary returned invalid version JSON")
}

fn validate_binary_identity(
    identity: &VersionInfo,
    version: &str,
    git_commit: &str,
    target: &str,
    allow_dirty: bool,
) -> Result<()> {
    ensure!(identity.ok, "candidate binary did not report ok");
    ensure!(identity.name == "moon", "candidate binary is not Moon");
    ensure!(
        identity.version == version,
        "candidate Moon version mismatch"
    );
    ensure!(
        identity.git_commit == git_commit,
        "candidate Git commit mismatch"
    );
    ensure!(
        identity.build_target == target,
        "candidate build target mismatch"
    );
    ensure!(
        identity.build_profile == "release",
        "candidate is not a release build"
    );
    ensure!(
        identity.bundle_format == BUNDLE_FORMAT,
        "candidate bundle format mismatch"
    );
    if !allow_dirty {
        ensure!(
            identity.git_dirty == Some(false),
            "release candidate source state must be clean and known"
        );
    }
    Ok(())
}

fn load_payload(
    binary: &Path,
    adapter_dir: &Path,
    skill: &Path,
    version: &str,
) -> Result<Vec<PayloadFile>> {
    let package_path = adapter_dir.join("package.json");
    let package_bytes = read_bounded_file(&package_path)?;
    let package: AdapterPackage =
        serde_json::from_slice(&package_bytes).context("invalid adapter package.json")?;
    ensure!(package.name == "moon", "adapter package name must be moon");
    ensure!(package.version == version, "adapter version mismatch");

    let mut payload = vec![
        PayloadFile {
            path: "bin/moon",
            mode: 0o755,
            bytes: read_bounded_file(binary)?,
        },
        PayloadFile {
            path: "openclaw-plugin/README.md",
            mode: 0o644,
            bytes: read_bounded_file(&adapter_dir.join("README.md"))?,
        },
        PayloadFile {
            path: "openclaw-plugin/index.js",
            mode: 0o644,
            bytes: read_bounded_file(&adapter_dir.join("index.js"))?,
        },
        PayloadFile {
            path: "openclaw-plugin/openclaw.plugin.json",
            mode: 0o644,
            bytes: read_bounded_file(&adapter_dir.join("openclaw.plugin.json"))?,
        },
        PayloadFile {
            path: "openclaw-plugin/package.json",
            mode: 0o644,
            bytes: package_bytes,
        },
        PayloadFile {
            path: "skill/SKILL.md",
            mode: 0o644,
            bytes: read_bounded_file(skill)?,
        },
    ];
    payload.sort_by_key(|file| file.path);
    Ok(payload)
}

fn build_archive(payload: &[PayloadFile], bundle_manifest: &[u8]) -> Result<Vec<u8>> {
    let gzip = GzBuilder::new()
        .mtime(0)
        .operating_system(255)
        .write(Vec::new(), Compression::best());
    let mut archive = Builder::new(gzip);
    archive.mode(HeaderMode::Deterministic);
    for file in payload {
        append_archive_file(
            &mut archive,
            &format!("moon-release/{}", file.path),
            file.mode,
            &file.bytes,
        )?;
    }
    append_archive_file(
        &mut archive,
        "moon-release/bundle-manifest.json",
        0o644,
        bundle_manifest,
    )?;
    let gzip = archive
        .into_inner()
        .context("failed to finish tar archive")?;
    gzip.finish().context("failed to finish gzip archive")
}

fn append_archive_file(
    archive: &mut Builder<GzEncoder<Vec<u8>>>,
    path: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<()> {
    let mut header = Header::new_gnu();
    header.set_entry_type(EntryType::Regular);
    header.set_size(bytes.len() as u64);
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_cksum();
    archive
        .append_data(&mut header, path, Cursor::new(bytes))
        .with_context(|| format!("failed to append {path}"))
}

fn ensure_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a regular file",
        path.display()
    );
    ensure!(
        metadata.len() <= MAX_ARCHIVE_BYTES,
        "{} exceeds size limit",
        path.display()
    );
    Ok(())
}

fn read_bounded_file(path: &Path) -> Result<Vec<u8>> {
    ensure_regular_file(path)?;
    fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output path has no parent")?;
    ensure!(
        parent.is_dir(),
        "output parent {} does not exist",
        parent.display()
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

#[cfg(target_os = "macos")]
fn write_new_public_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("public key path has no parent")?;
    ensure!(
        parent.is_dir(),
        "public key parent {} does not exist",
        parent.display()
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))
}

fn prepare_owner_only_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create output directory {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("failed to secure output directory {}", path.display()))?;
        }
    }
    ensure_owner_only_directory(path)
}

fn ensure_owner_only_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect directory {}", path.display()))?;
    ensure!(
        metadata.file_type().is_dir(),
        "{} is not a physical directory",
        path.display()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "output directory {} must be owner-only",
            path.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::GzDecoder;
    use std::collections::BTreeMap;
    use tar::Archive;

    fn fixture_payload() -> Vec<PayloadFile> {
        let mut files = vec![
            ("bin/moon", 0o755),
            ("openclaw-plugin/README.md", 0o644),
            ("openclaw-plugin/index.js", 0o644),
            ("openclaw-plugin/openclaw.plugin.json", 0o644),
            ("openclaw-plugin/package.json", 0o644),
            ("skill/SKILL.md", 0o644),
        ]
        .into_iter()
        .map(|(path, mode)| PayloadFile {
            path,
            mode,
            bytes: format!("fixture:{path}\n").into_bytes(),
        })
        .collect::<Vec<_>>();
        files.sort_by_key(|file| file.path);
        files
    }

    #[test]
    fn archive_bytes_and_metadata_are_deterministic() {
        let payload = fixture_payload();
        let manifest = b"{\"fixture\":true}\n";
        let first = build_archive(&payload, manifest).expect("first archive");
        let second = build_archive(&payload, manifest).expect("second archive");
        assert_eq!(first, second);

        let decoder = GzDecoder::new(first.as_slice());
        let mut archive = Archive::new(decoder);
        let mut entries = BTreeMap::new();
        for entry in archive.entries().expect("entries") {
            let mut entry = entry.expect("entry");
            let path = entry.path().expect("path").to_string_lossy().into_owned();
            let mode = entry.header().mode().expect("mode");
            let mtime = entry.header().mtime().expect("mtime");
            let uid = entry.header().uid().expect("uid");
            let gid = entry.header().gid().expect("gid");
            let mut bytes = Vec::new();
            std::io::copy(&mut entry, &mut bytes).expect("read entry");
            entries.insert(path, (mode, mtime, uid, gid, bytes));
        }
        assert_eq!(entries.len(), 7);
        assert_eq!(entries["moon-release/bin/moon"].0, 0o755);
        assert_eq!(entries["moon-release/bundle-manifest.json"].4, manifest);
        assert!(
            entries
                .values()
                .all(|(_, mtime, uid, gid, _)| (*mtime, *uid, *gid) == (0, 0, 0))
        );
    }

    #[test]
    fn output_writer_refuses_to_overwrite() {
        let temp = tempfile::tempdir().expect("tempdir");
        let output = temp.path().join("artifact");
        write_new_file(&output, b"first").expect("first write");
        assert!(write_new_file(&output, b"second").is_err());
        assert_eq!(fs::read(output).expect("read output"), b"first");
    }

    #[test]
    fn release_identity_rejects_dirty_candidates_without_an_override() {
        let identity = VersionInfo {
            ok: true,
            name: "moon".to_owned(),
            version: "2.1.0".to_owned(),
            git_commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            git_dirty: Some(true),
            build_target: "aarch64-apple-darwin".to_owned(),
            build_profile: "release".to_owned(),
            executable: "/tmp/moon".to_owned(),
            canonical_executable: "/tmp/moon".to_owned(),
            canonical: true,
            bundle_format: 1,
        };
        assert!(
            validate_binary_identity(
                &identity,
                &identity.version,
                &identity.git_commit,
                &identity.build_target,
                false,
            )
            .is_err()
        );
        validate_binary_identity(
            &identity,
            &identity.version,
            &identity.git_commit,
            &identity.build_target,
            true,
        )
        .expect("development override");
    }
}
