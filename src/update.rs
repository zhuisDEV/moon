use crate::release::{
    MAX_ARCHIVE_BYTES, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, ReleaseAsset, ReleaseManifest,
    VerifiedManifest, parse_bundle_manifest, production_trust_roots, sha256_hex,
    verify_release_manifest,
};
use crate::version::VersionInfo;
use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use tar::Archive;
use url::Url;
use wait_timeout::ChildExt;

pub const DEFAULT_RELEASE_BASE_URL: &str = "https://github.com/zhuisDEV/moon/releases";
pub const UPDATE_SCHEMA: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 64;
const ALLOWED_RELEASE_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
];

#[derive(Debug)]
pub struct UpdateFailure {
    pub code: &'static str,
    pub message: String,
}

impl std::fmt::Display for UpdateFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for UpdateFailure {}

pub fn fail<T>(code: &'static str, message: impl Into<String>) -> Result<T> {
    Err(UpdateFailure {
        code,
        message: message.into(),
    }
    .into())
}

pub fn error_code(error: &anyhow::Error) -> Option<&'static str> {
    error
        .downcast_ref::<UpdateFailure>()
        .map(|error| error.code)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoonIntegrationSnapshot {
    pub schema_version: u32,
    pub config_path: Option<PathBuf>,
    pub plugin_enabled: Option<bool>,
    pub plugin_config: BTreeMap<String, Value>,
    pub context_engine_slot: Option<String>,
    pub memory_slot: Option<String>,
    pub moon_path: Option<PathBuf>,
    pub moon_home: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateCheckReport {
    pub ok: bool,
    pub changed: bool,
    pub release_channel: String,
    pub running_version: String,
    pub running_executable: PathBuf,
    pub canonical_executable: PathBuf,
    pub canonical: bool,
    pub path_executable: Option<PathBuf>,
    pub configured_openclaw_executable: Option<PathBuf>,
    pub latest_version: String,
    pub target: String,
    pub database_schema: Option<i64>,
    pub database_schema_min: i64,
    pub database_schema_max: i64,
    pub adapter_version: Option<String>,
    pub skill_version: Option<String>,
    pub openclaw_min_version: String,
    pub supported: bool,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct VerifiedRelease {
    manifest_bytes: Vec<u8>,
    signature_bytes: Vec<u8>,
    manifest: ReleaseManifest,
    manifest_url: Url,
    verified_key_ids: Vec<String>,
}

impl VerifiedRelease {
    pub fn from_bytes(
        manifest_bytes: Vec<u8>,
        signature_bytes: Vec<u8>,
        manifest_url: Url,
    ) -> Result<Self> {
        let VerifiedManifest {
            manifest,
            verified_key_ids,
        } = verify_release_manifest(
            &manifest_bytes,
            &signature_bytes,
            &production_trust_roots()?,
        )
        .map_err(|error| UpdateFailure {
            code: "signature_invalid",
            message: format!("release manifest verification failed: {error:#}"),
        })?;
        Ok(Self {
            manifest_bytes,
            signature_bytes,
            manifest,
            manifest_url,
            verified_key_ids,
        })
    }

    pub fn asset_for_current_target(&self) -> Result<&ReleaseAsset> {
        let target = current_target();
        self.manifest
            .assets
            .iter()
            .find(|asset| asset.bundle.target == target)
            .ok_or_else(|| {
                UpdateFailure {
                    code: "unsupported_platform",
                    message: format!("release has no asset for {target}"),
                }
                .into()
            })
    }

    pub fn asset_url(&self, asset: &ReleaseAsset) -> Result<Url> {
        let mut url = self.manifest_url.clone();
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| anyhow::anyhow!("release manifest URL cannot be a base URL"))?;
        segments.pop().push(&asset.archive.file_name);
        drop(segments);
        validate_release_url(&url, false)?;
        Ok(url)
    }
}

#[derive(Debug, Clone)]
pub struct ReleaseClient {
    agent: ureq::Agent,
    base_url: Url,
}

impl ReleaseClient {
    pub fn production() -> Result<Self> {
        Self::new(Url::parse(DEFAULT_RELEASE_BASE_URL)?)
    }

    pub fn new(base_url: Url) -> Result<Self> {
        validate_release_url(&base_url, false)?;
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .timeout_global(Some(Duration::from_secs(30)))
            .max_redirects(0)
            .user_agent(concat!("moon/", env!("CARGO_PKG_VERSION")))
            .build();
        Ok(Self {
            agent: config.into(),
            base_url,
        })
    }

    pub fn fetch_release(&self, requested_version: Option<&str>) -> Result<VerifiedRelease> {
        let manifest_url = self.release_file_url(requested_version, "release-manifest.json")?;
        let signature_url =
            self.release_file_url(requested_version, "release-manifest.sig.json")?;
        let manifest_bytes = self.fetch_bounded(&manifest_url, MAX_MANIFEST_BYTES as u64)?;
        let signature_bytes = self.fetch_bounded(&signature_url, MAX_SIGNATURE_BYTES as u64)?;
        VerifiedRelease::from_bytes(manifest_bytes, signature_bytes, manifest_url)
    }

    pub fn fetch_archive(
        &self,
        release: &VerifiedRelease,
        asset: &ReleaseAsset,
    ) -> Result<Vec<u8>> {
        let url = release.asset_url(asset)?;
        let bytes = self.fetch_bounded(&url, asset.archive.size.min(MAX_ARCHIVE_BYTES))?;
        if let Err(error) = asset.verify_archive_bytes(&bytes) {
            return fail(
                "checksum_mismatch",
                format!("archive verification failed: {error:#}"),
            );
        }
        Ok(bytes)
    }

    fn release_file_url(&self, version: Option<&str>, file_name: &str) -> Result<Url> {
        ensure!(
            file_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-')),
            "release file name is unsafe"
        );
        let suffix = match version {
            Some(version) => {
                let version = Version::parse(version).context("requested version is invalid")?;
                format!("download/v{version}/{file_name}")
            }
            None => format!("latest/download/{file_name}"),
        };
        // Url::join treats a leading repository path differently. Build from the
        // trusted origin and validated path instead.
        let mut url = self.base_url.clone();
        url.set_path(&format!(
            "{}/{suffix}",
            self.base_url.path().trim_end_matches('/')
        ));
        url.set_query(None);
        url.set_fragment(None);
        validate_release_url(&url, false)?;
        Ok(url)
    }

    fn fetch_bounded(&self, url: &Url, limit: u64) -> Result<Vec<u8>> {
        validate_release_url(url, false)?;
        let mut current = url.clone();
        let mut redirect_count = 0_u8;
        let mut response = loop {
            let response =
                self.agent
                    .get(current.as_str())
                    .call()
                    .map_err(|error| UpdateFailure {
                        code: "release_unavailable",
                        message: format!("release download failed: {error}"),
                    })?;
            if !response.status().is_redirection() {
                break response;
            }
            let location = response
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| UpdateFailure {
                    code: "release_unavailable",
                    message: "release redirect has no valid Location header".to_owned(),
                })?;
            let redirected = current.join(location).map_err(|error| UpdateFailure {
                code: "release_unavailable",
                message: format!("release redirect URL is invalid: {error}"),
            })?;
            validate_release_url(&redirected, true).map_err(|error| UpdateFailure {
                code: "release_unavailable",
                message: format!("release redirect was refused: {error:#}"),
            })?;
            redirect_count = redirect_count.saturating_add(1);
            if redirect_count > 5 {
                return fail("release_unavailable", "too many release redirects");
            }
            current = redirected;
            if current == *url {
                return fail("release_unavailable", "release redirect loop was refused");
            }
        };
        if response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > limit)
        {
            return fail(
                "release_unavailable",
                "release response exceeds its size limit",
            );
        }
        response
            .body_mut()
            .with_config()
            .limit(limit.saturating_add(1))
            .read_to_vec()
            .map_err(|error| anyhow::anyhow!("bounded release read failed: {error}"))
            .and_then(|bytes| {
                if bytes.len() as u64 > limit {
                    fail(
                        "release_unavailable",
                        "release response exceeds its size limit",
                    )
                } else {
                    Ok(bytes)
                }
            })
    }
}

pub fn check_for_update(
    home: &Path,
    dimensions: usize,
    requested_version: Option<&str>,
    release: &VerifiedRelease,
) -> Result<UpdateCheckReport> {
    let identity = VersionInfo::current_for_home(home)?;
    let asset = release.asset_for_current_target()?;
    let current_version = Version::parse(&identity.version)?;
    let target_version = Version::parse(&release.manifest.moon_version)?;
    let snapshot = inspect_openclaw_config()?;
    let database = home.join("state/moon.sqlite");
    let database_schema = read_database_schema(&database, dimensions)?;
    let supported = database_schema.is_none_or(|schema| {
        schema >= asset.bundle.database_schema_min && schema <= asset.bundle.database_schema_max
    }) && ensure_os_version(&asset.bundle.minimum_os_version).is_ok();
    let canonical_executable = PathBuf::from(&identity.canonical_executable);
    let running_executable = PathBuf::from(&identity.executable);
    let path_executable = resolve_path_executable("moon");
    let configured_openclaw_executable = snapshot.moon_path.clone();
    let shadowed = !identity.canonical;
    let status = if shadowed {
        "shadowed_executable"
    } else if !supported {
        "unsupported_schema"
    } else if target_version > current_version {
        "update_available"
    } else if target_version == current_version {
        "current"
    } else {
        "downgrade_available"
    };
    if let Some(requested) = requested_version {
        ensure!(
            requested == release.manifest.moon_version,
            "release version does not match requested version"
        );
    }
    Ok(UpdateCheckReport {
        ok: true,
        changed: false,
        release_channel: "stable".to_owned(),
        running_version: identity.version,
        running_executable,
        canonical_executable,
        canonical: identity.canonical,
        path_executable,
        configured_openclaw_executable,
        latest_version: release.manifest.moon_version.clone(),
        target: asset.bundle.target.clone(),
        database_schema,
        database_schema_min: asset.bundle.database_schema_min,
        database_schema_max: asset.bundle.database_schema_max,
        adapter_version: installed_adapter_version(home),
        skill_version: installed_skill_version()?,
        openclaw_min_version: asset.bundle.openclaw_min_version.clone(),
        supported,
        status: status.to_owned(),
    })
}

pub fn current_target() -> &'static str {
    env!("MOON_BUILD_TARGET")
}

fn validate_release_url(url: &Url, redirected: bool) -> Result<()> {
    ensure!(url.scheme() == "https", "release URL must use HTTPS");
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "release URL must not contain credentials"
    );
    ensure!(
        url.port().is_none(),
        "release URL must use the default HTTPS port"
    );
    let host = url.host_str().context("release URL has no host")?;
    ensure!(
        ALLOWED_RELEASE_HOSTS.contains(&host),
        "{} host is not allowed: {host}",
        if redirected { "redirect" } else { "release" }
    );
    Ok(())
}

pub fn inspect_openclaw_config() -> Result<MoonIntegrationSnapshot> {
    let config_path = openclaw_config_path();
    inspect_openclaw_config_at(config_path.as_deref())
}

fn inspect_openclaw_config_at(config_path: Option<&Path>) -> Result<MoonIntegrationSnapshot> {
    let Some(path) = config_path else {
        return Ok(empty_snapshot(None));
    };
    if !path.is_file() {
        return Ok(empty_snapshot(Some(path.to_path_buf())));
    }
    let bytes = read_bounded_file(path, MAX_CONFIG_BYTES)?;
    let text = std::str::from_utf8(&bytes).context("OpenClaw config is not UTF-8")?;
    let root: Value = json5::from_str(text).context("OpenClaw config is invalid")?;
    let plugin_entry = root.pointer("/plugins/entries/moon");
    let plugin_enabled = plugin_entry
        .and_then(|entry| entry.get("enabled"))
        .and_then(Value::as_bool);
    let mut plugin_config = BTreeMap::new();
    if let Some(config) = plugin_entry
        .and_then(|entry| entry.get("config"))
        .and_then(Value::as_object)
    {
        for key in allowed_moon_config_keys() {
            if let Some(value) = config.get(*key) {
                plugin_config.insert((*key).to_owned(), value.clone());
            }
        }
    }
    let moon_path = plugin_config
        .get("moonPath")
        .and_then(Value::as_str)
        .map(expand_tilde)
        .transpose()?;
    let moon_home = plugin_config
        .get("moonHome")
        .and_then(Value::as_str)
        .map(expand_tilde)
        .transpose()?;
    Ok(MoonIntegrationSnapshot {
        schema_version: UPDATE_SCHEMA,
        config_path: Some(path.to_path_buf()),
        plugin_enabled,
        plugin_config,
        context_engine_slot: root
            .pointer("/plugins/slots/contextEngine")
            .and_then(Value::as_str)
            .map(str::to_owned),
        memory_slot: root
            .pointer("/plugins/slots/memory")
            .and_then(Value::as_str)
            .map(str::to_owned),
        moon_path,
        moon_home,
    })
}

fn allowed_moon_config_keys() -> &'static [&'static str] {
    &[
        "moonPath",
        "moonHome",
        "mode",
        "codexProvider",
        "codexModel",
        "codexReasoning",
        "modelTimeoutMs",
        "dimensions",
        "scope",
        "limit",
        "maxChars",
        "evidencePerMemory",
        "timeoutMs",
        "failOpen",
        "learningEnabled",
        "learningModel",
        "learningReasoning",
        "learningTimeoutMs",
        "learningScope",
        "learningMaxMemories",
        "learningMinConfidence",
        "learningMinImportance",
        "embeddingEnabled",
        "embeddingBatchSize",
        "embeddingTimeoutMs",
    ]
}

fn empty_snapshot(config_path: Option<PathBuf>) -> MoonIntegrationSnapshot {
    MoonIntegrationSnapshot {
        schema_version: UPDATE_SCHEMA,
        config_path,
        plugin_enabled: None,
        plugin_config: BTreeMap::new(),
        context_engine_slot: None,
        memory_slot: None,
        moon_path: None,
        moon_home: None,
    }
}

fn openclaw_config_path() -> Option<PathBuf> {
    env::var_os("OPENCLAW_CONFIG_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("OPENCLAW_STATE_DIR")
                .map(PathBuf::from)
                .map(|path| path.join("openclaw.json"))
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".openclaw/openclaw.json")))
}

fn installed_adapter_version(home: &Path) -> Option<String> {
    let path = home.join("openclaw-plugin/package.json");
    let bytes = read_bounded_file(&path, MAX_CONFIG_BYTES).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("version")?.as_str().map(str::to_owned)
}

fn installed_skill_version() -> Result<Option<String>> {
    let path = default_skill_path()?;
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = read_bounded_file(&path, MAX_CONFIG_BYTES)?;
    let text = std::str::from_utf8(&bytes).context("Moon skill is not UTF-8")?;
    Ok(text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("<!-- moon-version:")
            .and_then(|value| value.strip_suffix("-->"))
            .map(|value| value.trim().to_owned())
    }))
}

pub fn default_skill_path() -> Result<PathBuf> {
    env::var_os("MOON_SKILL_PATH")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".openclaw/skills/moon/SKILL.md")))
        .context("Moon skill path could not be resolved")
}

fn read_database_schema(database: &Path, dimensions: usize) -> Result<Option<i64>> {
    if !database.is_file() {
        return Ok(None);
    }
    let store = crate::Store::open_existing(database, dimensions)?;
    Ok(Some(store.health()?.schema_version))
}

fn resolve_path_executable(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|directory| directory.join(name))
            .find(|candidate| candidate.is_file())
            .map(|candidate| fs::canonicalize(&candidate).unwrap_or(candidate))
    })
}

fn expand_tilde(value: &str) -> Result<PathBuf> {
    if value == "~" {
        return dirs::home_dir().context("home directory is unavailable");
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return dirs::home_dir()
            .map(|home| home.join(rest))
            .context("home directory is unavailable");
    }
    Ok(PathBuf::from(value))
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    ensure!(
        metadata.file_type().is_file(),
        "{} is not a regular file",
        path.display()
    );
    ensure!(
        metadata.len() <= limit,
        "{} exceeds its size limit",
        path.display()
    );
    fs::read(path).with_context(|| format!("failed to read {}", path.display()))
}

pub fn extract_verified_archive(
    archive_bytes: &[u8],
    asset: &ReleaseAsset,
    destination: &Path,
) -> Result<()> {
    asset
        .verify_archive_bytes(archive_bytes)
        .map_err(|error| UpdateFailure {
            code: "checksum_mismatch",
            message: format!("archive verification failed: {error:#}"),
        })?;
    ensure!(!destination.exists(), "staging destination already exists");
    create_private_dir(destination)?;
    let decoder = GzDecoder::new(archive_bytes);
    let mut archive = Archive::new(decoder);
    let mut seen = BTreeSet::new();
    let mut extracted = BTreeMap::<String, (u32, Vec<u8>)>::new();
    let expected = asset
        .bundle
        .files
        .iter()
        .map(|file| file.path.as_str())
        .chain(std::iter::once("bundle-manifest.json"))
        .collect::<BTreeSet<_>>();
    for (index, entry) in archive.entries()?.enumerate() {
        ensure!(index < MAX_ARCHIVE_ENTRIES, "archive has too many entries");
        let mut entry = entry.context("invalid archive entry")?;
        ensure!(
            entry.header().entry_type().is_file(),
            "archive contains a non-file entry"
        );
        let path = entry.path().context("archive path is invalid")?;
        let path = path.to_str().context("archive path is not UTF-8")?;
        let relative = path
            .strip_prefix("moon-release/")
            .context("archive entry is outside moon-release/")?
            .to_owned();
        validate_relative_path(&relative)?;
        ensure!(
            expected.contains(relative.as_str()),
            "archive contains unexpected file {relative}"
        );
        ensure!(
            seen.insert(relative.clone()),
            "archive contains duplicate file {relative}"
        );
        let declared_size = entry.header().size()?;
        ensure!(
            declared_size > 0 && declared_size <= MAX_ARCHIVE_BYTES,
            "archive entry size is invalid"
        );
        let mode = entry.header().mode()? & 0o777;
        let mut bytes = Vec::with_capacity(declared_size as usize);
        (&mut entry)
            .take(declared_size.saturating_add(1))
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == declared_size,
            "archive entry size mismatch"
        );
        extracted.insert(relative, (mode, bytes));
    }
    ensure!(
        seen.len() == expected.len(),
        "archive is missing required files"
    );
    let (_, bundle_bytes) = extracted
        .get("bundle-manifest.json")
        .context("archive is missing bundle manifest")?;
    asset.verify_bundle_manifest_bytes(bundle_bytes)?;
    ensure!(
        parse_bundle_manifest(bundle_bytes)? == asset.bundle,
        "bundle manifest mismatch"
    );
    for file in &asset.bundle.files {
        let (mode, bytes) = extracted
            .get(&file.path)
            .with_context(|| format!("archive is missing {}", file.path))?;
        asset.bundle.verify_payload_file(&file.path, *mode, bytes)?;
    }
    for (relative, (mode, bytes)) in extracted {
        let output = destination.join(&relative);
        ensure!(
            output.starts_with(destination),
            "archive path escaped staging root"
        );
        if let Some(parent) = output.parent() {
            create_private_dir_all(parent)?;
        }
        write_new_file(&output, &bytes, mode)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePhase {
    Ownership,
    Preflight,
    FetchVerified,
    CandidateValidated,
    RollbackReady,
    Quiesced,
    Switched,
    Migrated,
    PostSwitchVerified,
    Committed,
    RolledBack,
    RollbackFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UpdateJournal {
    pub schema_version: u32,
    pub transaction_id: String,
    pub pid: u32,
    pub started_at: String,
    pub from_version: String,
    pub to_version: String,
    pub target: String,
    pub manifest_sha256: String,
    pub verified_key_ids: Vec<String>,
    pub phase: UpdatePhase,
    pub changed: bool,
    pub gateway_stopped: bool,
    pub current_switched: bool,
    pub schema_before: Option<i64>,
    pub schema_after: Option<i64>,
    pub prior_release: Option<PathBuf>,
    pub target_release: Option<PathBuf>,
    pub backup_path: Option<PathBuf>,
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdatePlan {
    pub from_version: String,
    pub to_version: String,
    pub target: String,
    pub archive_bytes: u64,
    pub database_schema_before: Option<i64>,
    pub database_schema_min: i64,
    pub database_schema_max: i64,
    pub canonical_executable: PathBuf,
    pub release_directory: PathBuf,
    pub restart_openclaw: bool,
    pub downgrade: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct UpdateResult {
    pub ok: bool,
    pub changed: bool,
    pub from_version: String,
    pub to_version: String,
    pub canonical_executable: PathBuf,
    pub schema_before: Option<i64>,
    pub schema_after: Option<i64>,
    pub adapter_version: String,
    pub gateway_reachable: bool,
    pub rollback_bundle: Option<PathBuf>,
    pub transaction_id: Option<String>,
    pub verified_key_ids: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ApplyContext {
    pub home: PathBuf,
    pub dimensions: usize,
    pub identity: VersionInfo,
    pub openclaw: MoonIntegrationSnapshot,
    pub skill_path: PathBuf,
    pub allow_downgrade: bool,
}

pub trait OpenClawControl {
    fn version(&self) -> Result<String>;
    fn stop(&self) -> Result<()>;
    fn start(&self) -> Result<()>;
    fn validate(&self, expected_version: &str, snapshot: &MoonIntegrationSnapshot) -> Result<()>;
    fn wait_ready(&self, timeout: Duration) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct SystemOpenClaw {
    executable: PathBuf,
}

impl SystemOpenClaw {
    pub fn discover() -> Result<Self> {
        let executable = resolve_path_executable("openclaw").ok_or_else(|| UpdateFailure {
            code: "plugin_validation_failed",
            message: "openclaw executable is not available on PATH".to_owned(),
        })?;
        Ok(Self { executable })
    }

    fn run(&self, args: &[&str]) -> Result<Output> {
        let output =
            run_output_bounded(&self.executable, args, 512 * 1024, Duration::from_secs(30))?;
        if !output.status.success() {
            let message = bounded_command_error(&output);
            return fail(
                "plugin_validation_failed",
                format!("OpenClaw command failed: {}: {message}", args.join(" ")),
            );
        }
        Ok(output)
    }
}

impl OpenClawControl for SystemOpenClaw {
    fn version(&self) -> Result<String> {
        let output = self.run(&["--version"])?;
        let text = std::str::from_utf8(&output.stdout).context("OpenClaw version is not UTF-8")?;
        let version = text
            .split_whitespace()
            .nth(1)
            .context("OpenClaw version output is invalid")?;
        Ok(version.to_owned())
    }

    fn stop(&self) -> Result<()> {
        self.run(&["gateway", "stop", "--json"])?;
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while moon_worker_is_running()? {
            if std::time::Instant::now() >= deadline {
                return fail(
                    "active_embedding_lease",
                    "a Moon worker remained active after OpenClaw stopped",
                );
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Ok(())
    }

    fn start(&self) -> Result<()> {
        self.run(&["gateway", "start", "--json"])?;
        Ok(())
    }

    fn validate(&self, expected_version: &str, snapshot: &MoonIntegrationSnapshot) -> Result<()> {
        self.run(&["config", "validate"])?;
        self.run(&["plugins", "doctor"])?;
        let output = self.run(&["plugins", "inspect", "moon", "--runtime", "--json"])?;
        ensure!(
            output.stdout.len() <= MAX_CONFIG_BYTES as usize,
            "OpenClaw plugin inspection exceeds size limit"
        );
        let value: Value = serde_json::from_slice(&output.stdout)
            .context("OpenClaw plugin inspection returned invalid JSON")?;
        ensure!(
            value.pointer("/plugin/id").and_then(Value::as_str) == Some("moon"),
            "loaded plugin id is not moon"
        );
        ensure!(
            value.pointer("/plugin/status").and_then(Value::as_str) == Some("loaded"),
            "Moon plugin is not loaded"
        );
        ensure!(
            value.pointer("/plugin/version").and_then(Value::as_str) == Some(expected_version),
            "loaded Moon adapter version does not match the release"
        );
        ensure!(
            value
                .pointer("/plugin/contextEngineIds")
                .and_then(Value::as_array)
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some("moon"))),
            "Moon context engine is not registered"
        );
        ensure!(
            value.pointer("/plugin/activated").and_then(Value::as_bool) == Some(true),
            "Moon plugin is not activated"
        );
        ensure!(
            value
                .pointer("/plugin/dependencyStatus/hasDependencies")
                .and_then(Value::as_bool)
                == Some(false),
            "Moon plugin unexpectedly has external dependencies"
        );
        ensure!(
            value
                .pointer("/diagnostics")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty),
            "Moon plugin inspection reported diagnostics"
        );
        ensure!(
            snapshot.context_engine_slot.as_deref() == Some("moon")
                && snapshot.memory_slot.as_deref() == Some("none"),
            "OpenClaw slots are not contextEngine=moon and memory=none"
        );
        if let Some(home) = snapshot.moon_home.as_deref() {
            let expected_root = fs::canonicalize(home.join("openclaw-plugin"))?;
            let loaded_root = value
                .pointer("/plugin/rootDir")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .context("OpenClaw inspection has no Moon plugin root")?;
            ensure!(
                fs::canonicalize(loaded_root)? == expected_root,
                "OpenClaw loaded Moon from an unexpected directory"
            );
        }
        Ok(())
    }

    fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let output = run_output_bounded(
                &self.executable,
                &["gateway", "health", "--json", "--timeout", "5000"],
                256 * 1024,
                Duration::from_secs(8),
            );
            if output.is_ok_and(|output| output.status.success()) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return fail(
                    "gateway_unreachable",
                    "OpenClaw gateway did not become ready before the deadline",
                );
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

pub fn plan_update(context: &ApplyContext, release: &VerifiedRelease) -> Result<UpdatePlan> {
    let asset = release.asset_for_current_target()?;
    let from = Version::parse(&context.identity.version)?;
    let to = Version::parse(&release.manifest.moon_version)?;
    if to < from && !context.allow_downgrade {
        return fail(
            "downgrade_refused",
            format!("downgrade from {from} to {to} requires --allow-downgrade"),
        );
    }
    let schema = read_database_schema(&context.home.join("state/moon.sqlite"), context.dimensions)?;
    if schema.is_some_and(|schema| {
        schema < asset.bundle.database_schema_min || schema > asset.bundle.database_schema_max
    }) {
        return fail(
            "unsupported_platform",
            "database schema is outside the release compatibility range",
        );
    }
    ensure_os_version(&asset.bundle.minimum_os_version)?;
    validate_openclaw_snapshot(context).map_err(|error| UpdateFailure {
        code: "plugin_validation_failed",
        message: format!("OpenClaw Moon integration is incompatible: {error:#}"),
    })?;
    Ok(UpdatePlan {
        from_version: from.to_string(),
        to_version: to.to_string(),
        target: asset.bundle.target.clone(),
        archive_bytes: asset.archive.size,
        database_schema_before: schema,
        database_schema_min: asset.bundle.database_schema_min,
        database_schema_max: asset.bundle.database_schema_max,
        canonical_executable: PathBuf::from(&context.identity.canonical_executable),
        release_directory: context.home.join("releases").join(to.to_string()),
        restart_openclaw: true,
        downgrade: to < from,
    })
}

pub fn preflight_update(
    context: &ApplyContext,
    release: &VerifiedRelease,
    archive_bytes: &[u8],
    openclaw: &dyn OpenClawControl,
) -> Result<UpdatePlan> {
    preflight_update_with_available_bytes(context, release, archive_bytes, openclaw, None)
}

fn preflight_update_with_available_bytes(
    context: &ApplyContext,
    release: &VerifiedRelease,
    archive_bytes: &[u8],
    openclaw: &dyn OpenClawControl,
    available_override: Option<u64>,
) -> Result<UpdatePlan> {
    let plan = plan_update(context, release)?;
    let asset = release.asset_for_current_target()?;
    asset
        .verify_archive_bytes(archive_bytes)
        .map_err(|error| UpdateFailure {
            code: "checksum_mismatch",
            message: format!("archive verification failed: {error:#}"),
        })?;
    let health = healthy_runtime(context)?;
    if health.leased_embeddings != 0 {
        return fail(
            "active_embedding_lease",
            "active embedding leases block an update",
        );
    }
    let required = required_free_space(context, asset.archive.size)?;
    let available = match available_override {
        Some(available) => available,
        None => available_bytes(&context.home)?,
    };
    if available < required {
        return fail(
            "insufficient_space",
            format!("update requires {required} free bytes but only {available} are available"),
        );
    }
    ensure_openclaw_version(&openclaw.version()?, &asset.bundle.openclaw_min_version)?;
    Ok(plan)
}

fn validate_openclaw_snapshot(context: &ApplyContext) -> Result<()> {
    ensure!(
        context.openclaw.plugin_enabled == Some(true),
        "Moon plugin is not enabled in OpenClaw"
    );
    ensure!(
        context.openclaw.context_engine_slot.as_deref() == Some("moon")
            && context.openclaw.memory_slot.as_deref() == Some("none"),
        "OpenClaw slots must be contextEngine=moon and memory=none"
    );
    let configured_home = context
        .openclaw
        .moon_home
        .as_deref()
        .context("OpenClaw Moon config has no moonHome")?;
    ensure!(
        paths_match(configured_home, &context.home),
        "OpenClaw moonHome does not match the runtime being updated"
    );
    let configured_binary = context
        .openclaw
        .moon_path
        .as_deref()
        .context("OpenClaw Moon config has no moonPath")?;
    ensure!(
        paths_match(configured_binary, &context.home.join("bin/moon")),
        "OpenClaw moonPath does not match the canonical Moon executable"
    );
    Ok(())
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub fn apply_update(
    context: &ApplyContext,
    release: &VerifiedRelease,
    archive_bytes: &[u8],
    openclaw: &dyn OpenClawControl,
) -> Result<UpdateResult> {
    apply_update_inner(context, release, archive_bytes, openclaw, None)
}

fn apply_update_inner(
    context: &ApplyContext,
    release: &VerifiedRelease,
    archive_bytes: &[u8],
    openclaw: &dyn OpenClawControl,
    crash_after: Option<UpdatePhase>,
) -> Result<UpdateResult> {
    let plan = preflight_update(context, release, archive_bytes, openclaw)?;
    if !context.identity.canonical {
        return fail(
            "shadowed_executable",
            format!(
                "updates must run through the canonical executable {}",
                context.identity.canonical_executable
            ),
        );
    }
    if context.identity.git_dirty != Some(false) {
        return fail(
            "version_identity_failed",
            "the running updater does not have clean release provenance",
        );
    }
    if plan.from_version == plan.to_version {
        return Ok(UpdateResult {
            ok: true,
            changed: false,
            from_version: plan.from_version.clone(),
            to_version: plan.to_version,
            canonical_executable: plan.canonical_executable,
            schema_before: plan.database_schema_before,
            schema_after: plan.database_schema_before,
            adapter_version: release.manifest.moon_version.clone(),
            gateway_reachable: true,
            rollback_bundle: None,
            transaction_id: None,
            verified_key_ids: release.verified_key_ids.clone(),
            warnings: duplicate_warnings(context),
        });
    }
    let asset = release.asset_for_current_target()?;

    let mut lock = UpdateLock::acquire(&context.home)?;
    recover_incomplete_update(context, openclaw)?;
    let transaction_id = lock.transaction_id.clone();
    let journal_path = context
        .home
        .join("update/journals")
        .join(format!("{transaction_id}.json"));
    create_private_dir_all(journal_path.parent().expect("journal parent"))?;
    let mut journal = UpdateJournal {
        schema_version: UPDATE_SCHEMA,
        transaction_id: transaction_id.clone(),
        pid: std::process::id(),
        started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        from_version: plan.from_version.clone(),
        to_version: plan.to_version.clone(),
        target: plan.target.clone(),
        manifest_sha256: sha256_hex(&release.manifest_bytes),
        verified_key_ids: release.verified_key_ids.clone(),
        phase: UpdatePhase::Ownership,
        changed: false,
        gateway_stopped: false,
        current_switched: false,
        schema_before: plan.database_schema_before,
        schema_after: None,
        prior_release: None,
        target_release: None,
        backup_path: None,
        error_code: None,
    };
    persist_journal(&journal_path, &journal)?;
    inject_crash(crash_after, journal.phase)?;

    let operation = (|| -> Result<UpdateResult> {
        let health_before = healthy_runtime(context)?;
        ensure!(
            health_before.leased_embeddings == 0,
            "active embedding leases block an update"
        );
        journal.phase = UpdatePhase::Preflight;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        let stage = context.home.join("update/staging").join(&transaction_id);
        create_private_dir_all(stage.parent().expect("staging parent"))?;
        extract_verified_archive(archive_bytes, asset, &stage)?;
        journal.phase = UpdatePhase::FetchVerified;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        validate_candidate(context, &stage, asset).map_err(|error| UpdateFailure {
            code: "candidate_failed",
            message: format!("isolated candidate validation failed: {error:#}"),
        })?;
        journal.phase = UpdatePhase::CandidateValidated;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        let prior_release = ensure_prior_release(context, &transaction_id)?;
        journal.prior_release = Some(prior_release.clone());
        let backup =
            create_rollback_bundle(context, release, &plan, &health_before, &transaction_id)
                .map_err(|error| UpdateFailure {
                    code: "backup_failed",
                    message: format!("rollback bundle creation failed: {error:#}"),
                })?;
        journal.backup_path = Some(backup.clone());
        journal.phase = UpdatePhase::RollbackReady;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        openclaw.stop()?;
        journal.gateway_stopped = true;
        journal.phase = UpdatePhase::Quiesced;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        let target_release = materialize_release(context, &stage, &plan.to_version)?;
        journal.target_release = Some(target_release.clone());
        switch_current(context, &target_release, &transaction_id)?;
        install_skill(
            &target_release.join("skill/SKILL.md"),
            &context.skill_path,
            &transaction_id,
        )?;
        journal.current_switched = true;
        journal.changed = true;
        journal.phase = UpdatePhase::Switched;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        run_migration(context)?;
        journal.phase = UpdatePhase::Migrated;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        verify_installed_identity(context, asset).map_err(|error| UpdateFailure {
            code: "version_identity_failed",
            message: format!("installed release identity verification failed: {error:#}"),
        })?;
        let health_after = healthy_runtime(context)?;
        ensure_queue_preserved(&health_before, &health_after)?;
        journal.schema_after = Some(health_after.schema_version);
        let snapshot_after = inspect_openclaw_config_at(context.openclaw.config_path.as_deref())?;
        ensure!(
            snapshot_after.context_engine_slot == context.openclaw.context_engine_slot
                && snapshot_after.memory_slot == context.openclaw.memory_slot,
            "OpenClaw Moon slots changed during update"
        );
        openclaw.start()?;
        openclaw.wait_ready(Duration::from_secs(60))?;
        openclaw
            .validate(&plan.to_version, &snapshot_after)
            .map_err(|error| UpdateFailure {
                code: "plugin_validation_failed",
                message: format!("OpenClaw Moon plugin validation failed: {error:#}"),
            })?;
        run_local_canary(context)?;
        journal.gateway_stopped = false;
        journal.phase = UpdatePhase::PostSwitchVerified;
        persist_journal(&journal_path, &journal)?;
        inject_crash(crash_after, journal.phase)?;

        journal.phase = UpdatePhase::Committed;
        persist_journal(&journal_path, &journal)?;
        lock.release()?;
        Ok(UpdateResult {
            ok: true,
            changed: true,
            from_version: plan.from_version.clone(),
            to_version: plan.to_version.clone(),
            canonical_executable: context.home.join("bin/moon"),
            schema_before: journal.schema_before,
            schema_after: journal.schema_after,
            adapter_version: plan.to_version.clone(),
            gateway_reachable: true,
            rollback_bundle: Some(backup),
            transaction_id: Some(transaction_id.clone()),
            verified_key_ids: release.verified_key_ids.clone(),
            warnings: duplicate_warnings(context),
        })
    })();

    match operation {
        Ok(result) => Ok(result),
        Err(error) => {
            if error_code(&error) == Some("injected_crash") {
                return Err(error);
            }
            let original_code = error_code(&error).unwrap_or("operation_failed");
            journal.error_code = Some(original_code.to_owned());
            if journal.gateway_stopped || journal.current_switched {
                match rollback_update(context, &journal, openclaw) {
                    Ok(()) => {
                        journal.phase = UpdatePhase::RolledBack;
                        journal.gateway_stopped = false;
                        persist_journal(&journal_path, &journal)?;
                        lock.release()?;
                        fail(
                            "rollback_completed",
                            format!("update failed and the prior runtime was restored: {error:#}"),
                        )
                    }
                    Err(rollback_error) => {
                        journal.phase = UpdatePhase::RollbackFailed;
                        journal.error_code = Some("rollback_failed".to_owned());
                        persist_journal(&journal_path, &journal)?;
                        fail(
                            "rollback_failed",
                            format!(
                                "update failed ({error:#}) and rollback also failed ({rollback_error:#})"
                            ),
                        )
                    }
                }
            } else {
                persist_journal(&journal_path, &journal)?;
                lock.release()?;
                Err(error)
            }
        }
    }
}

fn inject_crash(requested: Option<UpdatePhase>, current: UpdatePhase) -> Result<()> {
    if requested == Some(current) {
        fail(
            "injected_crash",
            format!("injected crash after phase {current:?}"),
        )
    } else {
        Ok(())
    }
}

#[derive(Debug)]
struct UpdateLock {
    path: PathBuf,
    transaction_id: String,
    released: bool,
}

impl UpdateLock {
    fn acquire(home: &Path) -> Result<Self> {
        let update_dir = home.join("update");
        create_private_dir_all(&update_dir)?;
        let path = update_dir.join("update.lock");
        if path.exists() {
            let bytes = read_bounded_file(&path, 16 * 1024)?;
            let existing: LockDocument =
                serde_json::from_slice(&bytes).context("existing update lock is invalid")?;
            if pid_is_alive(existing.pid) {
                return fail(
                    "update_locked",
                    format!(
                        "Moon update {} is already active under PID {}",
                        existing.transaction_id, existing.pid
                    ),
                );
            }
            fs::remove_file(&path).context("failed to reclaim stale update lock")?;
        }
        let transaction_id = format!(
            "{}-{:032x}",
            chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
            rand::random::<u128>()
        );
        let document = LockDocument {
            schema_version: UPDATE_SCHEMA,
            transaction_id: transaction_id.clone(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        };
        write_new_file(&path, &serde_json::to_vec(&document)?, 0o600)?;
        Ok(Self {
            path,
            transaction_id,
            released: false,
        })
    }

    fn release(&mut self) -> Result<()> {
        if !self.released {
            fs::remove_file(&self.path).context("failed to release update lock")?;
            self.released = true;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LockDocument {
    schema_version: u32,
    transaction_id: String,
    pid: u32,
    started_at: String,
}

fn persist_journal(path: &Path, journal: &UpdateJournal) -> Result<()> {
    let bytes = serde_json::to_vec(journal)?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    if temporary.exists() {
        fs::remove_file(&temporary).context("failed to clear stale journal temporary file")?;
    }
    write_new_file(&temporary, &bytes, 0o600)?;
    fs::rename(&temporary, path).with_context(|| format!("failed to commit {}", path.display()))?;
    sync_parent(path)
}

fn recover_incomplete_update(context: &ApplyContext, openclaw: &dyn OpenClawControl) -> Result<()> {
    let directory = context.home.join("update/journals");
    if !directory.is_dir() {
        return Ok(());
    }
    let mut paths = fs::read_dir(&directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|value| value == "json"))
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let bytes = read_bounded_file(&path, 128 * 1024)?;
        let mut journal: UpdateJournal = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid update journal {}", path.display()))?;
        if matches!(
            journal.phase,
            UpdatePhase::Committed | UpdatePhase::RolledBack | UpdatePhase::RollbackFailed
        ) {
            continue;
        }
        if journal.pid != std::process::id() && pid_is_alive(journal.pid) {
            return fail(
                "update_locked",
                format!(
                    "incomplete update {} is still active",
                    journal.transaction_id
                ),
            );
        }
        if journal.gateway_stopped || journal.current_switched {
            rollback_update(context, &journal, openclaw).map_err(|error| UpdateFailure {
                code: "rollback_failed",
                message: format!(
                    "failed to recover incomplete update {}: {error:#}",
                    journal.transaction_id
                ),
            })?;
        }
        journal.phase = UpdatePhase::RolledBack;
        journal.gateway_stopped = false;
        journal.error_code = Some("interrupted_update_recovered".to_owned());
        persist_journal(&path, &journal)?;
    }
    Ok(())
}

fn healthy_runtime(context: &ApplyContext) -> Result<crate::model::HealthReport> {
    let database = context.home.join("state/moon.sqlite");
    let store = crate::Store::open_existing(&database, context.dimensions).map_err(|error| {
        UpdateFailure {
            code: "unhealthy_runtime",
            message: format!("Moon runtime could not be opened safely: {error:#}"),
        }
    })?;
    let health = store.health().map_err(|error| UpdateFailure {
        code: "unhealthy_runtime",
        message: format!("Moon health check failed: {error:#}"),
    })?;
    if !health.ok {
        return fail("unhealthy_runtime", "Moon health is not ok");
    }
    Ok(health)
}

fn validate_candidate(context: &ApplyContext, stage: &Path, asset: &ReleaseAsset) -> Result<()> {
    let binary = stage.join("bin/moon");
    let output = run_bounded(&binary, &["--json", "--version"], 64 * 1024)?;
    let identity: VersionInfo =
        serde_json::from_slice(&output).context("candidate returned invalid version identity")?;
    ensure!(
        identity.ok && identity.name == "moon",
        "candidate is not Moon"
    );
    ensure!(
        identity.version == asset.bundle.moon_version
            && identity.git_commit == asset.bundle.git_commit
            && identity.build_target == asset.bundle.target
            && identity.bundle_format == asset.bundle.bundle_format
            && identity.build_profile == "release"
            && identity.git_dirty == Some(false),
        "candidate identity does not match the verified manifest"
    );
    let isolated_parent = context.home.join("update/candidate-homes");
    create_private_dir_all(&isolated_parent)?;
    let isolated = tempfile::Builder::new()
        .prefix("candidate-")
        .tempdir_in(&isolated_parent)?;
    let home = isolated.path().to_string_lossy().into_owned();
    let dimensions = context.dimensions.to_string();
    run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "init",
        ],
        128 * 1024,
    )?;
    let health = run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "health",
        ],
        128 * 1024,
    )?;
    let health: Value = serde_json::from_slice(&health)?;
    ensure!(
        health.get("ok").and_then(Value::as_bool) == Some(true),
        "candidate health failed"
    );
    run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "remember",
            "--content",
            "Moon isolated update canary",
            "--scope",
            "update-canary",
        ],
        128 * 1024,
    )?;
    let search = run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "search",
            "--query",
            "isolated update canary",
            "--mode",
            "lexical",
        ],
        128 * 1024,
    )?;
    let search: Value = serde_json::from_slice(&search)?;
    ensure!(
        search.as_array().is_some_and(|hits| !hits.is_empty()),
        "candidate lexical canary returned no result"
    );
    Ok(())
}

fn ensure_prior_release(context: &ApplyContext, transaction_id: &str) -> Result<PathBuf> {
    let release = context
        .home
        .join("releases")
        .join(&context.identity.version);
    if release.is_dir() {
        verify_release_root_identity(&release, &context.identity)?;
        return Ok(release);
    }
    create_private_dir_all(release.parent().expect("release parent"))?;
    let staging = context
        .home
        .join("update/staging")
        .join(format!("prior-{transaction_id}"));
    ensure!(!staging.exists(), "prior-release staging already exists");
    create_private_dir(&staging)?;
    create_private_dir_all(&staging.join("bin"))?;
    copy_regular_file(
        &PathBuf::from(&context.identity.executable),
        &staging.join("bin/moon"),
        0o755,
    )?;
    copy_tree(
        &fs::canonicalize(context.home.join("openclaw-plugin"))?,
        &staging.join("openclaw-plugin"),
    )?;
    if context.skill_path.is_file() {
        create_private_dir_all(&staging.join("skill"))?;
        copy_regular_file(&context.skill_path, &staging.join("skill/SKILL.md"), 0o644)?;
    }
    fs::rename(&staging, &release).context("failed to materialize prior release")?;
    sync_parent(&release)?;
    verify_release_root_identity(&release, &context.identity)?;
    Ok(release)
}

fn verify_release_root_identity(release: &Path, expected: &VersionInfo) -> Result<()> {
    let bytes = run_bounded(
        &release.join("bin/moon"),
        &["--json", "--version"],
        64 * 1024,
    )?;
    let identity: VersionInfo = serde_json::from_slice(&bytes)?;
    ensure!(
        identity.version == expected.version
            && identity.git_commit == expected.git_commit
            && identity.build_target == expected.build_target,
        "prior release identity does not match the running updater"
    );
    let package = read_bounded_file(
        &release.join("openclaw-plugin/package.json"),
        MAX_CONFIG_BYTES,
    )?;
    let package: Value = serde_json::from_slice(&package)?;
    ensure!(
        package.get("version").and_then(Value::as_str) == Some(expected.version.as_str()),
        "prior adapter version does not match the running updater"
    );
    Ok(())
}

fn create_rollback_bundle(
    context: &ApplyContext,
    release: &VerifiedRelease,
    plan: &UpdatePlan,
    health: &crate::model::HealthReport,
    transaction_id: &str,
) -> Result<PathBuf> {
    let backup = context.home.join("backups").join(transaction_id);
    ensure!(!backup.exists(), "rollback bundle already exists");
    create_private_dir_all(&backup)?;
    let database = context.home.join("state/moon.sqlite");
    let store = crate::Store::open_existing(&database, context.dimensions)?;
    store.backup_to(&backup.join("moon.sqlite"))?;
    store.export_memories(&backup.join("memory-export.md"))?;
    write_new_file(
        &backup.join("health.json"),
        &serde_json::to_vec(health)?,
        0o600,
    )?;
    write_new_file(&backup.join("plan.json"), &serde_json::to_vec(plan)?, 0o600)?;
    write_new_file(
        &backup.join("openclaw-moon.json"),
        &serde_json::to_vec(&context.openclaw)?,
        0o600,
    )?;
    write_new_file(
        &backup.join("release-manifest.json"),
        &release.manifest_bytes,
        0o600,
    )?;
    write_new_file(
        &backup.join("release-manifest.sig.json"),
        &release.signature_bytes,
        0o600,
    )?;
    let files = backup.join("files");
    create_private_dir_all(&files.join("bin"))?;
    copy_regular_file(
        &PathBuf::from(&context.identity.executable),
        &files.join("bin/moon"),
        0o755,
    )?;
    copy_tree(
        &fs::canonicalize(context.home.join("openclaw-plugin"))?,
        &files.join("openclaw-plugin"),
    )?;
    if context.skill_path.is_file() {
        create_private_dir_all(&files.join("skill"))?;
        copy_regular_file(&context.skill_path, &files.join("skill/SKILL.md"), 0o644)?;
    }
    let hashes = hash_tree(&files)?;
    write_new_file(
        &backup.join("file-hashes.json"),
        &serde_json::to_vec(&hashes)?,
        0o600,
    )?;
    let verified =
        crate::Store::open_existing(backup.join("moon.sqlite"), context.dimensions)?.health()?;
    ensure!(verified.ok, "rollback database verification failed");
    Ok(backup)
}

fn materialize_release(context: &ApplyContext, stage: &Path, version: &str) -> Result<PathBuf> {
    let destination = context.home.join("releases").join(version);
    if destination.exists() {
        return fail(
            "candidate_failed",
            format!(
                "target release directory already exists: {}",
                destination.display()
            ),
        );
    }
    create_private_dir_all(destination.parent().expect("release parent"))?;
    fs::rename(stage, &destination).context("failed to materialize target release")?;
    sync_parent(&destination)?;
    Ok(destination)
}

fn switch_current(
    context: &ApplyContext,
    target_release: &Path,
    transaction_id: &str,
) -> Result<()> {
    #[cfg(not(unix))]
    return fail(
        "unsupported_platform",
        "versioned switching requires Unix symlinks",
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let releases = context.home.join("releases");
        ensure!(
            target_release.starts_with(&releases),
            "release path is outside runtime"
        );
        let relative = Path::new("releases").join(
            target_release
                .file_name()
                .context("release directory has no version name")?,
        );
        let temporary = context.home.join(format!(".current-{transaction_id}"));
        symlink(&relative, &temporary)?;
        fs::rename(&temporary, context.home.join("current"))?;

        let bin = context.home.join("bin");
        create_private_dir_all(&bin)?;
        let moon_temp = bin.join(format!(".moon-{transaction_id}"));
        symlink("../current/bin/moon", &moon_temp)?;
        fs::rename(&moon_temp, bin.join("moon"))?;

        let plugin = context.home.join("openclaw-plugin");
        if !fs::symlink_metadata(&plugin).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            let retired = context
                .home
                .join("update/retired")
                .join(format!("{transaction_id}-openclaw-plugin"));
            create_private_dir_all(retired.parent().expect("retired parent"))?;
            fs::rename(&plugin, &retired).context("failed to retain prior adapter directory")?;
            let plugin_temp = context
                .home
                .join(format!(".openclaw-plugin-{transaction_id}"));
            symlink("current/openclaw-plugin", &plugin_temp)?;
            fs::rename(&plugin_temp, &plugin)?;
        }
        sync_parent(&context.home.join("current"))
    }
}

fn install_skill(source: &Path, destination: &Path, transaction_id: &str) -> Result<()> {
    let parent = destination.parent().context("skill path has no parent")?;
    fs::create_dir_all(parent)?;
    let bytes = read_bounded_file(source, MAX_CONFIG_BYTES)?;
    let temporary = parent.join(format!(".SKILL.md-{transaction_id}"));
    if temporary.exists() {
        fs::remove_file(&temporary)?;
    }
    write_new_file(&temporary, &bytes, 0o644)?;
    fs::rename(&temporary, destination)?;
    sync_parent(destination)
}

fn run_migration(context: &ApplyContext) -> Result<()> {
    let binary = context.home.join("bin/moon");
    let home = context.home.to_string_lossy().into_owned();
    let dimensions = context.dimensions.to_string();
    run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "init",
        ],
        128 * 1024,
    )
    .map(|_| ())
    .map_err(|error| {
        UpdateFailure {
            code: "migration_failed",
            message: format!("Moon migration failed: {error:#}"),
        }
        .into()
    })
}

fn verify_installed_identity(context: &ApplyContext, asset: &ReleaseAsset) -> Result<()> {
    let binary = context.home.join("bin/moon");
    let bytes = run_bounded(&binary, &["--json", "--version"], 64 * 1024)?;
    let identity: VersionInfo = serde_json::from_slice(&bytes)?;
    ensure!(
        identity.version == asset.bundle.moon_version
            && identity.git_commit == asset.bundle.git_commit
            && identity.build_target == asset.bundle.target
            && identity.git_dirty == Some(false),
        "installed Moon identity does not match the signed manifest"
    );
    let release_root = fs::canonicalize(context.home.join("current"))?;
    for file in &asset.bundle.files {
        let installed = release_root.join(&file.path);
        let bytes = read_bounded_file(&installed, MAX_ARCHIVE_BYTES)?;
        asset
            .bundle
            .verify_payload_file(&file.path, file_mode(&installed)?, &bytes)?;
    }
    Ok(())
}

fn run_local_canary(context: &ApplyContext) -> Result<()> {
    let binary = context.home.join("bin/moon");
    let home = context.home.to_string_lossy().into_owned();
    let dimensions = context.dimensions.to_string();
    run_bounded(
        &binary,
        &[
            "--home",
            &home,
            "--dimensions",
            &dimensions,
            "--json",
            "context",
            "--query",
            "Moon post-update canary",
            "--mode",
            "hybrid",
            "--provider",
            "local",
            "--max-chars",
            "512",
        ],
        256 * 1024,
    )
    .map(|_| ())
    .map_err(|error| {
        UpdateFailure {
            code: "candidate_failed",
            message: format!("post-update Moon canary failed: {error:#}"),
        }
        .into()
    })
}

fn rollback_update(
    context: &ApplyContext,
    journal: &UpdateJournal,
    openclaw: &dyn OpenClawControl,
) -> Result<()> {
    if journal.current_switched {
        let prior = journal
            .prior_release
            .as_deref()
            .context("journal has no prior release")?;
        switch_current(
            context,
            prior,
            &format!("rollback-{}", journal.transaction_id),
        )?;
        let backup = journal
            .backup_path
            .as_deref()
            .context("journal has no rollback bundle")?;
        let skill = backup.join("files/skill/SKILL.md");
        if skill.is_file() {
            install_skill(
                &skill,
                &context.skill_path,
                &format!("rollback-{}", journal.transaction_id),
            )?;
        }
        restore_database(
            context,
            &backup.join("moon.sqlite"),
            &journal.transaction_id,
        )?;
    }
    if journal.gateway_stopped {
        openclaw.start()?;
        openclaw.wait_ready(Duration::from_secs(60))?;
        openclaw.validate(&journal.from_version, &context.openclaw)?;
    }
    let health = healthy_runtime(context)?;
    ensure!(
        journal
            .schema_before
            .is_none_or(|schema| health.schema_version == schema),
        "rollback schema does not match the pre-update schema"
    );
    Ok(())
}

fn restore_database(context: &ApplyContext, backup: &Path, transaction_id: &str) -> Result<()> {
    ensure!(backup.is_file(), "rollback database is missing");
    let state = context.home.join("state");
    let database = state.join("moon.sqlite");
    let failed = state.join(format!("moon.sqlite.failed-{transaction_id}"));
    if database.exists() {
        fs::rename(&database, &failed).context("failed to preserve failed database")?;
    }
    for suffix in ["-wal", "-shm"] {
        let sidecar = state.join(format!("moon.sqlite{suffix}"));
        if sidecar.exists() {
            fs::remove_file(&sidecar)?;
        }
    }
    copy_regular_file(backup, &database, 0o600)?;
    let health = crate::Store::open_existing(&database, context.dimensions)?.health()?;
    ensure!(health.ok, "restored rollback database is unhealthy");
    Ok(())
}

fn ensure_queue_preserved(
    before: &crate::model::HealthReport,
    after: &crate::model::HealthReport,
) -> Result<()> {
    ensure!(after.ok, "post-update health is not ok");
    ensure!(
        after.failed_embeddings <= before.failed_embeddings
            && after.dead_embeddings <= before.dead_embeddings,
        "update introduced failed or dead embedding work"
    );
    Ok(())
}

fn required_free_space(context: &ApplyContext, archive_size: u64) -> Result<u64> {
    let database = context.home.join("state/moon.sqlite");
    let database_size = fs::metadata(database)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let installed_size = directory_size(&context.home.join("openclaw-plugin"))?.saturating_add(
        fs::metadata(&context.identity.executable)
            .map(|metadata| metadata.len())
            .unwrap_or(0),
    );
    Ok(archive_size
        .saturating_mul(2)
        .saturating_add(database_size.saturating_mul(2))
        .saturating_add(installed_size.saturating_mul(2))
        .saturating_add(64 * 1024 * 1024))
}

#[cfg(unix)]
fn available_bytes(path: &Path) -> Result<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    let result = unsafe { libc::statvfs(path.as_ptr(), stats.as_mut_ptr()) };
    ensure!(result == 0, "failed to inspect free disk space");
    let stats = unsafe { stats.assume_init() };
    Ok(u64::from(stats.f_bavail).saturating_mul(stats.f_frsize))
}

#[cfg(not(unix))]
fn available_bytes(_path: &Path) -> Result<u64> {
    fail(
        "unsupported_platform",
        "disk space inspection requires Unix",
    )
}

fn ensure_openclaw_version(current: &str, minimum: &str) -> Result<()> {
    let current = numeric_version_core(current)?;
    let minimum = numeric_version_core(minimum)?;
    if current < minimum {
        return fail(
            "plugin_validation_failed",
            format!("OpenClaw {current} is older than required {minimum}"),
        );
    }
    Ok(())
}

fn ensure_os_version(minimum: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let output = run_output_bounded(
            Path::new("/usr/bin/sw_vers"),
            &["-productVersion"],
            1024,
            Duration::from_secs(5),
        )?;
        ensure!(output.status.success(), "sw_vers failed");
        let current = dotted_version(std::str::from_utf8(&output.stdout)?.trim())?;
        let minimum = dotted_version(minimum)?;
        if current < minimum {
            return fail(
                "unsupported_platform",
                format!("macOS {current} is older than required {minimum}"),
            );
        }
    }
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        let required = minimum
            .strip_prefix("glibc-")
            .context("Linux minimum_os_version must use glibc-X.Y")?;
        let current = unsafe {
            std::ffi::CStr::from_ptr(libc::gnu_get_libc_version())
                .to_str()
                .context("glibc version is not UTF-8")?
        };
        let current = dotted_version(current)?;
        let required = dotted_version(required)?;
        if current < required {
            return fail(
                "unsupported_platform",
                format!("glibc {current} is older than required {required}"),
            );
        }
    }
    #[cfg(not(any(target_os = "macos", all(target_os = "linux", target_env = "gnu"))))]
    return fail(
        "unsupported_platform",
        "Moon updater does not support this operating system",
    );
    Ok(())
}

fn dotted_version(value: &str) -> Result<Version> {
    let mut components = value.split('.').collect::<Vec<_>>();
    ensure!(
        (2..=3).contains(&components.len()),
        "invalid operating-system version {value}"
    );
    while components.len() < 3 {
        components.push("0");
    }
    Version::parse(&components.join("."))
        .with_context(|| format!("invalid operating-system version {value}"))
}

fn numeric_version_core(value: &str) -> Result<Version> {
    let core = value
        .split_once('-')
        .map_or(value, |(core, _)| core)
        .split_once('+')
        .map_or_else(
            || value.split_once('-').map_or(value, |(core, _)| core),
            |(core, _)| core,
        );
    Version::parse(core).with_context(|| format!("invalid version {value}"))
}

fn duplicate_warnings(context: &ApplyContext) -> Vec<String> {
    let mut warnings = Vec::new();
    if let Some(path) = resolve_path_executable("moon")
        && fs::canonicalize(&path).ok()
            != fs::canonicalize(&context.identity.canonical_executable).ok()
    {
        warnings.push(format!(
            "inactive Moon executable remains at {}",
            path.display()
        ));
    }
    warnings
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    ensure!(source.is_dir(), "{} is not a directory", source.display());
    create_private_dir_all(destination)?;
    for entry in walkdir::WalkDir::new(source).follow_links(false) {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            create_private_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            let mode = file_mode(entry.path())?;
            copy_regular_file(entry.path(), &target, mode)?;
        } else {
            bail!("refusing to copy non-file {}", entry.path().display());
        }
    }
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let bytes = read_bounded_file(source, MAX_ARCHIVE_BYTES)?;
    if let Some(parent) = destination.parent() {
        create_private_dir_all(parent)?;
    }
    write_new_file(destination, &bytes, mode)
}

fn hash_tree(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .into_owned();
            hashes.insert(
                relative,
                sha256_hex(&read_bounded_file(entry.path(), MAX_ARCHIVE_BYTES)?),
            );
        }
    }
    Ok(hashes)
}

fn directory_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

#[cfg(unix)]
fn file_mode(path: &Path) -> Result<u32> {
    use std::os::unix::fs::PermissionsExt;
    Ok(fs::metadata(path)?.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> Result<u32> {
    Ok(0o644)
}

fn run_bounded(program: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>> {
    let output = run_output_bounded(program, args, limit, Duration::from_secs(120))?;
    if !output.status.success() {
        bail!(
            "{} failed: {}",
            program.display(),
            bounded_command_error(&output)
        );
    }
    Ok(output.stdout)
}

fn run_output_bounded(
    program: &Path,
    args: &[&str],
    limit: usize,
    timeout: Duration,
) -> Result<Output> {
    use std::process::Stdio;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", program.display()))?;
    let mut stdout = child
        .stdout
        .take()
        .context("command stdout is unavailable")?;
    let mut stderr = child
        .stderr
        .take()
        .context("command stderr is unavailable")?;
    let stdout_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        (&mut stdout)
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let stderr_thread = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        (&mut stderr)
            .take(limit.saturating_add(1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let status = match child.wait_timeout(timeout)? {
        Some(status) => status,
        None => {
            child.kill()?;
            let _ = child.wait();
            bail!("{} exceeded its execution timeout", program.display());
        }
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr = stderr_thread
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    ensure!(
        stdout.len() <= limit && stderr.len() <= limit,
        "command output exceeds size limit"
    );
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn moon_worker_is_running() -> Result<bool> {
    let output = run_output_bounded(
        Path::new("/bin/ps"),
        &["-axo", "command="],
        8 * 1024 * 1024,
        Duration::from_secs(5),
    )?;
    ensure!(output.status.success(), "process inspection failed");
    let commands = String::from_utf8_lossy(&output.stdout);
    Ok(commands.lines().any(|line| {
        let words = line.split_whitespace().collect::<Vec<_>>();
        words.iter().any(|word| {
            Path::new(word)
                .file_name()
                .is_some_and(|name| name == "moon")
        }) && words.contains(&"serve")
    }))
}

fn bounded_command_error(output: &Output) -> String {
    let bytes = if output.stderr.is_empty() {
        &output.stdout
    } else {
        &output.stderr
    };
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    crate::redaction::redact_text(&text).value
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: u32) -> bool {
    true
}

fn sync_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty() && !value.contains('\\'),
        "archive path is unsafe"
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "archive path must be relative");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "archive path contains unsafe components"
    );
    Ok(())
}

fn create_private_dir(path: &Path) -> Result<()> {
    fs::create_dir(path).with_context(|| format!("failed to create {}", path.display()))?;
    set_mode(path, 0o700)
}

fn create_private_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    set_mode(path, 0o700)
}

fn write_new_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)?;
    file.sync_all()?;
    set_mode(path, mode)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("failed to set permissions on {}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::{
        ArchiveDescriptor, BUNDLE_MANIFEST_SCHEMA, BundleFile, BundleManifest,
        RELEASE_MANIFEST_SCHEMA, ReleaseChannel, RollbackCompatibility, encode_bundle_manifest,
    };
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::sync::Mutex;
    use tar::{Builder, EntryType, Header};

    #[test]
    fn openclaw_snapshot_excludes_credentials_and_unrelated_fields() {
        let root: Value = json5::from_str(
            r#"{
              plugins: {
                entries: { moon: { enabled: true, config: { moonPath: "/moon", mode: "hybrid", token: "secret" } } },
                slots: { contextEngine: "moon", memory: "none" }
              },
              secrets: { token: "never-copy-me" }
            }"#,
        )
        .expect("fixture");
        let config = root
            .pointer("/plugins/entries/moon/config")
            .unwrap()
            .as_object()
            .unwrap();
        let copied = allowed_moon_config_keys()
            .iter()
            .filter_map(|key| {
                config
                    .get(*key)
                    .map(|value| ((*key).to_owned(), value.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let serialized = serde_json::to_string(&copied).unwrap();
        assert!(serialized.contains("moonPath"));
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("token"));
    }

    #[test]
    fn release_urls_are_restricted_to_https_github_hosts() {
        assert!(
            validate_release_url(
                &Url::parse("https://github.com/zhuisDEV/moon/releases").unwrap(),
                false
            )
            .is_ok()
        );
        assert!(
            validate_release_url(
                &Url::parse("http://github.com/zhuisDEV/moon/releases").unwrap(),
                false
            )
            .is_err()
        );
        assert!(
            validate_release_url(&Url::parse("https://example.com/moon").unwrap(), false).is_err()
        );
        assert!(
            validate_release_url(
                &Url::parse("https://user:pass@github.com/moon").unwrap(),
                false
            )
            .is_err()
        );
    }

    #[test]
    fn archive_paths_reject_traversal_and_platform_separators() {
        assert!(validate_relative_path("bin/moon").is_ok());
        assert!(validate_relative_path("../moon").is_err());
        assert!(validate_relative_path("bin\\moon").is_err());
        assert!(validate_relative_path("/bin/moon").is_err());
    }

    #[test]
    fn extractor_rejects_symlinks_duplicates_and_checksum_changes() {
        let mut symlink_archive = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_link_name("../../outside").unwrap();
        header.set_cksum();
        symlink_archive
            .append_data(&mut header, "moon-release/bin/moon", &[][..])
            .unwrap();
        let bytes = symlink_archive.into_inner().unwrap().finish().unwrap();
        let mut asset = sample_asset();
        asset.archive.size = bytes.len() as u64;
        asset.archive.sha256 = sha256_hex(&bytes);
        let temp = tempfile::tempdir().unwrap();
        assert!(extract_verified_archive(&bytes, &asset, &temp.path().join("symlink")).is_err());

        let mut duplicate_archive = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
        append_fixture(&mut duplicate_archive, "moon-release/bin/moon", 0o755, b"x");
        append_fixture(&mut duplicate_archive, "moon-release/bin/moon", 0o755, b"x");
        let bytes = duplicate_archive.into_inner().unwrap().finish().unwrap();
        asset.archive.size = bytes.len() as u64;
        asset.archive.sha256 = sha256_hex(&bytes);
        assert!(extract_verified_archive(&bytes, &asset, &temp.path().join("duplicate")).is_err());

        let mut altered = bytes;
        altered[0] ^= 1;
        let error = extract_verified_archive(&altered, &asset, &temp.path().join("altered"))
            .expect_err("altered archive");
        assert_eq!(error_code(&error), Some("checksum_mismatch"));
    }

    fn sample_asset() -> ReleaseAsset {
        let files = [
            ("bin/moon", 0o755),
            ("openclaw-plugin/README.md", 0o644),
            ("openclaw-plugin/index.js", 0o644),
            ("openclaw-plugin/openclaw.plugin.json", 0o644),
            ("openclaw-plugin/package.json", 0o644),
            ("skill/SKILL.md", 0o644),
        ]
        .iter()
        .map(|(path, mode)| BundleFile {
            path: (*path).to_owned(),
            size: 1,
            sha256: sha256_hex(b"x"),
            mode: *mode,
        })
        .collect();
        let bundle = BundleManifest {
            schema_version: BUNDLE_MANIFEST_SCHEMA,
            bundle_format: 1,
            moon_version: "2.2.0".to_owned(),
            git_tag: "v2.2.0".to_owned(),
            git_commit: "a".repeat(40),
            target: current_target().to_owned(),
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
        };
        let bundle_bytes = crate::release::encode_bundle_manifest(&bundle).unwrap();
        ReleaseAsset {
            archive: ArchiveDescriptor {
                file_name: format!("moon-2.2.0-{}.tar.gz", current_target()),
                size: 1,
                sha256: sha256_hex(b"x"),
                bundle_manifest_sha256: sha256_hex(&bundle_bytes),
            },
            bundle,
        }
    }

    #[test]
    fn current_target_has_a_supported_manifest_shape() {
        let asset = sample_asset();
        assert!(asset.bundle.validate().is_ok());
        let manifest = ReleaseManifest {
            schema_version: RELEASE_MANIFEST_SCHEMA,
            release_channel: ReleaseChannel::Stable,
            moon_version: asset.bundle.moon_version.clone(),
            git_tag: asset.bundle.git_tag.clone(),
            git_commit: asset.bundle.git_commit.clone(),
            published_at: "2026-08-12T00:00:00Z".to_owned(),
            assets: vec![asset],
        };
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn update_check_is_read_only_for_a_missing_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("missing moon home");
        let asset = sample_asset();
        let manifest = ReleaseManifest {
            schema_version: RELEASE_MANIFEST_SCHEMA,
            release_channel: ReleaseChannel::Stable,
            moon_version: asset.bundle.moon_version.clone(),
            git_tag: asset.bundle.git_tag.clone(),
            git_commit: asset.bundle.git_commit.clone(),
            published_at: "2026-08-12T00:00:00Z".to_owned(),
            assets: vec![asset],
        };
        let release = VerifiedRelease {
            manifest_bytes: crate::release::encode_release_manifest(&manifest).unwrap(),
            signature_bytes: vec![],
            manifest,
            manifest_url: Url::parse(
                "https://github.com/zhuisDEV/moon/releases/latest/download/release-manifest.json",
            )
            .unwrap(),
            verified_key_ids: vec!["test".to_owned()],
        };
        let report = check_for_update(&home, 64, None, &release).unwrap();
        assert_eq!(report.database_schema, None);
        assert!(!home.exists());
    }

    #[test]
    fn active_and_stale_update_locks_are_distinguished() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("moon");
        fs::create_dir_all(&home).unwrap();
        let mut first = UpdateLock::acquire(&home).unwrap();
        let error = UpdateLock::acquire(&home).expect_err("active lock");
        assert_eq!(error_code(&error), Some("update_locked"));
        let mut document: LockDocument =
            serde_json::from_slice(&fs::read(&first.path).unwrap()).unwrap();
        document.pid = i32::MAX as u32;
        fs::write(&first.path, serde_json::to_vec(&document).unwrap()).unwrap();
        first.released = true;
        let mut reclaimed = UpdateLock::acquire(&home).expect("stale lock reclaimed");
        reclaimed.release().unwrap();
    }

    #[derive(Default)]
    struct MockOpenClaw {
        events: Mutex<Vec<String>>,
        validation_failures: Mutex<usize>,
        ready_failures: Mutex<usize>,
    }

    impl OpenClawControl for MockOpenClaw {
        fn version(&self) -> Result<String> {
            Ok("2026.7.1-2".to_owned())
        }

        fn stop(&self) -> Result<()> {
            self.events.lock().unwrap().push("stop".to_owned());
            Ok(())
        }

        fn start(&self) -> Result<()> {
            self.events.lock().unwrap().push("start".to_owned());
            Ok(())
        }

        fn validate(
            &self,
            expected_version: &str,
            snapshot: &MoonIntegrationSnapshot,
        ) -> Result<()> {
            self.events
                .lock()
                .unwrap()
                .push(format!("validate:{expected_version}"));
            ensure!(
                snapshot.context_engine_slot.as_deref() == Some("moon"),
                "wrong context slot"
            );
            let mut failures = self.validation_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                fail(
                    "plugin_validation_failed",
                    "injected plugin validation failure",
                )
            } else {
                Ok(())
            }
        }

        fn wait_ready(&self, _timeout: Duration) -> Result<()> {
            self.events.lock().unwrap().push("ready".to_owned());
            let mut failures = self.ready_failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                fail("gateway_unreachable", "injected readiness failure")
            } else {
                Ok(())
            }
        }
    }

    struct TransactionFixture {
        _root: tempfile::TempDir,
        context: ApplyContext,
        release: VerifiedRelease,
        archive: Vec<u8>,
    }

    fn transaction_fixture() -> TransactionFixture {
        transaction_fixture_with_failures(false, false)
    }

    fn transaction_fixture_with_candidate_failure(fail_candidate: bool) -> TransactionFixture {
        transaction_fixture_with_failures(fail_candidate, false)
    }

    fn transaction_fixture_with_failures(
        fail_candidate: bool,
        fail_migration: bool,
    ) -> TransactionFixture {
        let root = tempfile::tempdir().expect("tempdir");
        let home = root.path().join("moon home");
        fs::create_dir_all(home.join("bin")).expect("bin");
        let old_commit = "a".repeat(40);
        let old_script = format!(
            "#!/bin/sh\nprintf '%s\\n' '{{\"ok\":true,\"name\":\"moon\",\"version\":\"2.1.0\",\"git_commit\":\"{old_commit}\",\"git_dirty\":false,\"build_target\":\"{}\",\"build_profile\":\"release\",\"executable\":\"fixture-old\",\"canonical_executable\":\"fixture-old\",\"canonical\":true,\"bundle_format\":1}}'\n",
            current_target()
        );
        fs::write(home.join("bin/moon"), old_script).expect("old binary");
        set_mode(&home.join("bin/moon"), 0o755).expect("mode");
        for (path, bytes) in [
            ("README.md", b"old adapter".as_slice()),
            ("index.js", b"export default {};".as_slice()),
            ("openclaw.plugin.json", br#"{"id":"moon"}"#.as_slice()),
            (
                "package.json",
                br#"{"name":"moon","version":"2.1.0"}"#.as_slice(),
            ),
        ] {
            let destination = home.join("openclaw-plugin").join(path);
            fs::create_dir_all(destination.parent().unwrap()).expect("plugin parent");
            fs::write(destination, bytes).expect("plugin file");
        }
        let skill = root.path().join("openclaw/skills/moon/SKILL.md");
        fs::create_dir_all(skill.parent().unwrap()).expect("skill parent");
        fs::write(&skill, "<!-- moon-version: 2.1.0 -->\n# Moon\n").expect("skill");
        let config = root.path().join("openclaw/openclaw.json");
        fs::write(
            &config,
            serde_json::to_vec(&serde_json::json!({
                "plugins": {
                    "entries": {"moon": {"enabled": true, "config": {
                        "moonPath": home.join("bin/moon"),
                        "moonHome": home,
                        "mode": "lexical"
                    }}},
                    "slots": {"contextEngine": "moon", "memory": "none"}
                },
                "secrets": {"sentinel": "must-never-be-backed-up"}
            }))
            .unwrap(),
        )
        .expect("config");
        let mut store = crate::Store::open(home.join("state/moon.sqlite"), 64).expect("store");
        store
            .remember(crate::MemoryInput {
                memory_kind: "fact".to_owned(),
                scope: "test".to_owned(),
                title: Some("fixture".to_owned()),
                content: "rollback data".to_owned(),
                importance: 0.5,
                confidence: 1.0,
                pinned: false,
            })
            .expect("memory");
        drop(store);

        let commit = "b".repeat(40);
        let candidate_failure = if fail_candidate {
            "  if [ \"$value\" = \"remember\" ]; then exit 9; fi\n"
        } else {
            ""
        };
        let migration_failure = if fail_migration {
            format!(
                "  if [ \"$value\" = '{}' ]; then production_home=1; fi\n",
                home.display()
            )
        } else {
            String::new()
        };
        let migration_exit = if fail_migration {
            "if [ \"${production_home:-0}\" = 1 ]; then\n  for value in \"$@\"; do\n    if [ \"$value\" = \"init\" ]; then exit 10; fi\n  done\nfi\n"
        } else {
            ""
        };
        let script = format!(
            "#!/bin/sh\nfor value in \"$@\"; do\n  if [ \"$value\" = \"--version\" ]; then\n    printf '%s\\n' '{{\"ok\":true,\"name\":\"moon\",\"version\":\"2.2.0\",\"git_commit\":\"{commit}\",\"git_dirty\":false,\"build_target\":\"{}\",\"build_profile\":\"release\",\"executable\":\"fixture\",\"canonical_executable\":\"fixture\",\"canonical\":true,\"bundle_format\":1}}'\n    exit 0\n  fi\n{migration_failure}done\n{migration_exit}for value in \"$@\"; do\n{candidate_failure}  if [ \"$value\" = \"health\" ]; then printf '%s\\n' '{{\"ok\":true}}'; exit 0; fi\n  if [ \"$value\" = \"search\" ]; then printf '%s\\n' '[{{\"content\":\"Moon isolated update canary\"}}]'; exit 0; fi\ndone\nprintf '%s\\n' '{{\"ok\":true}}'\n",
            current_target()
        )
        .into_bytes();
        let payload = BTreeMap::from([
            ("bin/moon".to_owned(), (0o755, script)),
            (
                "openclaw-plugin/README.md".to_owned(),
                (0o644, b"new adapter".to_vec()),
            ),
            (
                "openclaw-plugin/index.js".to_owned(),
                (0o644, b"export default {};\n".to_vec()),
            ),
            (
                "openclaw-plugin/openclaw.plugin.json".to_owned(),
                (0o644, br#"{"id":"moon","kind":"context-engine"}"#.to_vec()),
            ),
            (
                "openclaw-plugin/package.json".to_owned(),
                (0o644, br#"{"name":"moon","version":"2.2.0"}"#.to_vec()),
            ),
            (
                "skill/SKILL.md".to_owned(),
                (0o644, b"<!-- moon-version: 2.2.0 -->\n# Moon\n".to_vec()),
            ),
        ]);
        let files = payload
            .iter()
            .map(|(path, (mode, bytes))| BundleFile {
                path: path.clone(),
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
                mode: *mode,
            })
            .collect::<Vec<_>>();
        let bundle = BundleManifest {
            schema_version: BUNDLE_MANIFEST_SCHEMA,
            bundle_format: 1,
            moon_version: "2.2.0".to_owned(),
            git_tag: "v2.2.0".to_owned(),
            git_commit: commit,
            target: current_target().to_owned(),
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
        };
        let bundle_bytes = encode_bundle_manifest(&bundle).expect("bundle");
        let archive = fixture_archive(&payload, &bundle_bytes);
        let asset = ReleaseAsset {
            archive: ArchiveDescriptor {
                file_name: format!("moon-2.2.0-{}.tar.gz", current_target()),
                size: archive.len() as u64,
                sha256: sha256_hex(&archive),
                bundle_manifest_sha256: sha256_hex(&bundle_bytes),
            },
            bundle,
        };
        let manifest = ReleaseManifest {
            schema_version: RELEASE_MANIFEST_SCHEMA,
            release_channel: ReleaseChannel::Stable,
            moon_version: "2.2.0".to_owned(),
            git_tag: "v2.2.0".to_owned(),
            git_commit: "b".repeat(40),
            published_at: "2026-08-12T00:00:00Z".to_owned(),
            assets: vec![asset],
        };
        manifest.validate().expect("manifest");
        let identity = VersionInfo {
            ok: true,
            name: "moon".to_owned(),
            version: "2.1.0".to_owned(),
            git_commit: old_commit,
            git_dirty: Some(false),
            build_target: current_target().to_owned(),
            build_profile: "release".to_owned(),
            executable: home.join("bin/moon").to_string_lossy().into_owned(),
            canonical_executable: home.join("bin/moon").to_string_lossy().into_owned(),
            canonical: true,
            bundle_format: 1,
        };
        let snapshot = inspect_openclaw_config_at(Some(&config)).expect("snapshot");
        let release = VerifiedRelease {
            manifest_bytes: crate::release::encode_release_manifest(&manifest).expect("manifest"),
            signature_bytes: b"test-signature-fixture".to_vec(),
            manifest,
            manifest_url: Url::parse(
                "https://github.com/zhuisDEV/moon/releases/download/v2.2.0/release-manifest.json",
            )
            .unwrap(),
            verified_key_ids: vec!["test-release-key".to_owned()],
        };
        TransactionFixture {
            _root: root,
            context: ApplyContext {
                home,
                dimensions: 64,
                identity,
                openclaw: snapshot,
                skill_path: skill,
                allow_downgrade: false,
            },
            release,
            archive,
        }
    }

    fn fixture_archive(payload: &BTreeMap<String, (u32, Vec<u8>)>, bundle: &[u8]) -> Vec<u8> {
        let gzip = GzEncoder::new(Vec::new(), Compression::best());
        let mut archive = Builder::new(gzip);
        for (path, (mode, bytes)) in payload {
            append_fixture(&mut archive, &format!("moon-release/{path}"), *mode, bytes);
        }
        append_fixture(
            &mut archive,
            "moon-release/bundle-manifest.json",
            0o644,
            bundle,
        );
        archive.into_inner().unwrap().finish().unwrap()
    }

    fn append_fixture(
        archive: &mut Builder<GzEncoder<Vec<u8>>>,
        path: &str,
        mode: u32,
        bytes: &[u8],
    ) {
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(&mut header, path, bytes).unwrap();
    }

    #[test]
    fn isolated_transaction_switches_complete_bundle_and_retains_rollback() {
        let fixture = transaction_fixture();
        let openclaw = MockOpenClaw::default();
        let health_before = healthy_runtime(&fixture.context).unwrap();
        let result = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect("apply update");
        assert!(result.ok && result.changed && result.gateway_reachable);
        assert_eq!(result.from_version, "2.1.0");
        assert_eq!(result.to_version, "2.2.0");
        assert_eq!(result.verified_key_ids, ["test-release-key"]);
        assert!(fixture.context.home.join("current").is_symlink());
        assert!(fixture.context.home.join("bin/moon").is_symlink());
        assert!(fixture.context.home.join("openclaw-plugin").is_symlink());
        assert!(
            fixture
                .context
                .home
                .join("releases/2.1.0/bin/moon")
                .is_file()
        );
        assert!(
            fixture
                .context
                .home
                .join("releases/2.2.0/bin/moon")
                .is_file()
        );
        let backup = result.rollback_bundle.expect("rollback bundle");
        assert!(backup.join("moon.sqlite").is_file());
        assert_eq!(file_mode(&backup).unwrap(), 0o700);
        assert_eq!(file_mode(&backup.join("moon.sqlite")).unwrap(), 0o600);
        let snapshot = fs::read_to_string(backup.join("openclaw-moon.json")).unwrap();
        assert!(!snapshot.contains("must-never-be-backed-up"));
        let health_after = healthy_runtime(&fixture.context).unwrap();
        assert_eq!(
            health_after.pending_embeddings,
            health_before.pending_embeddings
        );
        assert!(health_after.failed_embeddings <= health_before.failed_embeddings);
        assert!(health_after.dead_embeddings <= health_before.dead_embeddings);
        assert!(!fixture.context.home.join("update/update.lock").exists());
        assert_eq!(
            openclaw.events.lock().unwrap().as_slice(),
            ["stop", "start", "ready", "validate:2.2.0"]
        );
    }

    #[test]
    fn post_switch_failure_restores_prior_release_database_and_gateway() {
        let fixture = transaction_fixture();
        let openclaw = MockOpenClaw {
            validation_failures: Mutex::new(1),
            ..MockOpenClaw::default()
        };
        let error = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect_err("injected failure");
        assert_eq!(error_code(&error), Some("rollback_completed"));
        assert_eq!(
            fs::canonicalize(fixture.context.home.join("current")).unwrap(),
            fs::canonicalize(fixture.context.home.join("releases/2.1.0")).unwrap()
        );
        assert!(healthy_runtime(&fixture.context).unwrap().ok);
        assert!(!fixture.context.home.join("update/update.lock").exists());
        let events = openclaw.events.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            [
                "stop",
                "start",
                "ready",
                "validate:2.2.0",
                "start",
                "ready",
                "validate:2.1.0"
            ]
        );
    }

    #[test]
    fn gateway_readiness_failure_rolls_back_and_restores_connectivity() {
        let fixture = transaction_fixture();
        let openclaw = MockOpenClaw {
            ready_failures: Mutex::new(1),
            ..MockOpenClaw::default()
        };
        let error = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect_err("readiness failure");
        assert_eq!(error_code(&error), Some("rollback_completed"));
        assert_eq!(
            fs::canonicalize(fixture.context.home.join("current")).unwrap(),
            fs::canonicalize(fixture.context.home.join("releases/2.1.0")).unwrap()
        );
        let events = openclaw.events.lock().unwrap();
        assert_eq!(
            events.as_slice(),
            ["stop", "start", "ready", "start", "ready", "validate:2.1.0"]
        );
    }

    #[test]
    fn migration_failure_restores_prior_release_database_and_gateway() {
        let fixture = transaction_fixture_with_failures(false, true);
        let openclaw = MockOpenClaw::default();
        let error = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect_err("migration failure");
        assert_eq!(error_code(&error), Some("rollback_completed"));
        assert_eq!(
            fs::canonicalize(fixture.context.home.join("current")).unwrap(),
            fs::canonicalize(fixture.context.home.join("releases/2.1.0")).unwrap()
        );
        assert!(healthy_runtime(&fixture.context).unwrap().ok);
        let journal_path = fs::read_dir(fixture.context.home.join("update/journals"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let journal: UpdateJournal =
            serde_json::from_slice(&fs::read(journal_path).unwrap()).unwrap();
        assert_eq!(journal.phase, UpdatePhase::RolledBack);
        assert_eq!(journal.error_code.as_deref(), Some("migration_failed"));
        assert_eq!(
            openclaw.events.lock().unwrap().as_slice(),
            ["stop", "start", "ready", "validate:2.1.0"]
        );
    }

    #[test]
    fn preflight_rejects_shadow_downgrade_active_lease_and_corruption_without_writes() {
        let fixture = transaction_fixture();
        let openclaw = MockOpenClaw::default();

        let mut shadow = fixture.context.clone();
        shadow.identity.canonical = false;
        let error = apply_update(&shadow, &fixture.release, &fixture.archive, &openclaw)
            .expect_err("shadowed updater");
        assert_eq!(error_code(&error), Some("shadowed_executable"));

        let mut downgrade = fixture.release.clone();
        downgrade.manifest.moon_version = "2.0.0".to_owned();
        downgrade.manifest.git_tag = "v2.0.0".to_owned();
        let asset = &mut downgrade.manifest.assets[0];
        asset.bundle.moon_version = "2.0.0".to_owned();
        asset.bundle.git_tag = "v2.0.0".to_owned();
        asset.bundle.adapter_version = "2.0.0".to_owned();
        asset.bundle.skill_version = "2.0.0".to_owned();
        asset.archive.file_name = format!("moon-2.0.0-{}.tar.gz", current_target());
        let error = plan_update(&fixture.context, &downgrade).expect_err("downgrade");
        assert_eq!(error_code(&error), Some("downgrade_refused"));

        let mut wrong_platform = fixture.release.clone();
        wrong_platform.manifest.assets[0].bundle.target = match current_target() {
            "aarch64-apple-darwin" => "x86_64-apple-darwin",
            _ => "aarch64-apple-darwin",
        }
        .to_owned();
        let error = plan_update(&fixture.context, &wrong_platform).expect_err("wrong platform");
        assert_eq!(error_code(&error), Some("unsupported_platform"));
        assert!(!fixture.context.home.join("update/update.lock").exists());

        let connection =
            rusqlite::Connection::open(fixture.context.home.join("state/moon.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE embedding_queue SET claimed_by = 'fixture', lease_until_ms = ?1",
                [chrono::Utc::now().timestamp_millis() + 60_000],
            )
            .unwrap();
        drop(connection);
        let error = preflight_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect_err("active lease");
        assert_eq!(error_code(&error), Some("active_embedding_lease"));
        assert!(!fixture.context.home.join("update/update.lock").exists());

        let connection =
            rusqlite::Connection::open(fixture.context.home.join("state/moon.sqlite")).unwrap();
        connection
            .execute(
                "UPDATE embedding_queue SET claimed_by = NULL, lease_until_ms = NULL",
                [],
            )
            .unwrap();
        drop(connection);
        let error = preflight_update_with_available_bytes(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
            Some(0),
        )
        .expect_err("insufficient space");
        assert_eq!(error_code(&error), Some("insufficient_space"));
        assert!(!fixture.context.home.join("update/update.lock").exists());

        let mut corrupted = fixture.archive.clone();
        corrupted[0] ^= 1;
        let error = preflight_update(&fixture.context, &fixture.release, &corrupted, &openclaw)
            .expect_err("checksum mismatch");
        assert_eq!(error_code(&error), Some("checksum_mismatch"));
        assert!(!fixture.context.home.join("update/update.lock").exists());
    }

    #[test]
    fn candidate_failure_leaves_live_compatibility_set_unchanged() {
        let fixture = transaction_fixture_with_candidate_failure(true);
        let openclaw = MockOpenClaw::default();
        let binary_before = sha256_hex(&fs::read(&fixture.context.identity.executable).unwrap());
        let plugin_before =
            sha256_hex(&fs::read(fixture.context.home.join("openclaw-plugin/index.js")).unwrap());
        let database_before =
            sha256_hex(&fs::read(fixture.context.home.join("state/moon.sqlite")).unwrap());
        let error = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect_err("candidate failure");
        assert_eq!(error_code(&error), Some("candidate_failed"));
        assert_eq!(
            sha256_hex(&fs::read(&fixture.context.identity.executable).unwrap()),
            binary_before
        );
        assert_eq!(
            sha256_hex(&fs::read(fixture.context.home.join("openclaw-plugin/index.js")).unwrap()),
            plugin_before
        );
        assert_eq!(
            sha256_hex(&fs::read(fixture.context.home.join("state/moon.sqlite")).unwrap()),
            database_before
        );
        assert!(!fixture.context.home.join("current").exists());
        assert!(openclaw.events.lock().unwrap().is_empty());
        assert!(!fixture.context.home.join("update/update.lock").exists());
    }

    #[test]
    fn exact_current_version_is_an_idempotent_no_op() {
        let mut fixture = transaction_fixture();
        fixture.context.identity.version = "2.2.0".to_owned();
        fixture.context.identity.git_commit = "b".repeat(40);
        let openclaw = MockOpenClaw::default();
        let result = apply_update(
            &fixture.context,
            &fixture.release,
            &fixture.archive,
            &openclaw,
        )
        .expect("no-op");
        assert!(result.ok && !result.changed);
        assert_eq!(result.verified_key_ids, ["test-release-key"]);
        assert!(result.rollback_bundle.is_none());
        assert!(!fixture.context.home.join("update").exists());
        assert!(openclaw.events.lock().unwrap().is_empty());
    }

    #[test]
    fn interrupted_journal_phases_recover_deterministically() {
        let phases = [
            UpdatePhase::Ownership,
            UpdatePhase::Preflight,
            UpdatePhase::FetchVerified,
            UpdatePhase::CandidateValidated,
            UpdatePhase::RollbackReady,
            UpdatePhase::Quiesced,
            UpdatePhase::Switched,
            UpdatePhase::Migrated,
            UpdatePhase::PostSwitchVerified,
        ];
        for phase in phases {
            let fixture = transaction_fixture();
            let openclaw = MockOpenClaw::default();
            let error = apply_update_inner(
                &fixture.context,
                &fixture.release,
                &fixture.archive,
                &openclaw,
                Some(phase),
            )
            .expect_err("injected crash");
            assert_eq!(
                error_code(&error),
                Some("injected_crash"),
                "phase {phase:?}"
            );

            let lock_path = fixture.context.home.join("update/update.lock");
            let mut lock_document: LockDocument =
                serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
            lock_document.pid = i32::MAX as u32;
            fs::write(&lock_path, serde_json::to_vec(&lock_document).unwrap()).unwrap();

            let journal_path = fs::read_dir(fixture.context.home.join("update/journals"))
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            let mut journal: UpdateJournal =
                serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
            journal.pid = i32::MAX as u32;
            persist_journal(&journal_path, &journal).unwrap();

            let mut recovery_lock = UpdateLock::acquire(&fixture.context.home).unwrap();
            recover_incomplete_update(&fixture.context, &openclaw).unwrap();
            recovery_lock.release().unwrap();
            let recovered: UpdateJournal =
                serde_json::from_slice(&fs::read(&journal_path).unwrap()).unwrap();
            assert_eq!(recovered.phase, UpdatePhase::RolledBack, "phase {phase:?}");
            assert!(
                healthy_runtime(&fixture.context).unwrap().ok,
                "phase {phase:?}"
            );
            if phase >= UpdatePhase::Switched {
                assert_eq!(
                    fs::canonicalize(fixture.context.home.join("current")).unwrap(),
                    fs::canonicalize(fixture.context.home.join("releases/2.1.0")).unwrap(),
                    "phase {phase:?}"
                );
            }
        }
    }
}
