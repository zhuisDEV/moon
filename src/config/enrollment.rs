use super::{ConfigFile, MAX_CONFIG_BYTES};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, value};

/// Ordered by proof strength. An enrolled sender can only move up this order.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Deserialize,
    Serialize,
    clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
pub enum EnrollmentSource {
    FirstHuman,
    #[default]
    Allowlist,
    Gateway,
}

impl EnrollmentSource {
    fn name(self) -> &'static str {
        match self {
            Self::Gateway => "gateway",
            Self::Allowlist => "allowlist",
            Self::FirstHuman => "first-human",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MasterChannel {
    pub channel: String,
    pub account_id: String,
    pub sender_id: String,
    #[serde(default)]
    pub source: EnrollmentSource,
}

impl MasterChannel {
    fn validate(&self) -> Result<()> {
        // Store only opaque IDs with a bounded TOML literal representation.
        // The adapter checks the provider-specific identity and human sender.
        for field in [&self.channel, &self.account_id, &self.sender_id] {
            ensure!(
                !field.is_empty()
                    && field.len() <= 256
                    && field
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.:+@".contains(&b)),
                "invalid master channel identity"
            );
        }
        ensure!(
            self.channel
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
            "invalid channel name"
        );
        Ok(())
    }
}

pub(super) fn validate_channels(channels: &[MasterChannel]) -> Result<()> {
    let mut accounts = HashSet::new();
    for channel in channels {
        channel.validate()?;
        ensure!(
            accounts.insert((&channel.channel, &channel.account_id)),
            "duplicate master channel account"
        );
    }
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct Enrollment {
    pub enrolled: bool,
    pub matches: bool,
    pub source: EnrollmentSource,
}

/// A local operator/adapter action. The caller attests the channel's trusted
/// sender. It confers no Gateway role and cannot replace an existing binding.
pub fn enroll_master_channel(
    home: &Path,
    profile_id: &str,
    identity: MasterChannel,
) -> Result<Enrollment> {
    identity.validate()?;
    let path = home.join("moon.toml");
    ensure!(
        fs::symlink_metadata(&path)?.is_file(),
        "enrollment requires a regular moon.toml"
    );
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    // The config inode is replaced by rename; cooperating processes must lock
    // a stable inode, including when enrolling different accounts.
    let lock = options.open(home.join(".moon-master-channels.lock"))?;
    lock.lock()
        .context("could not lock master channel configuration")?;
    ensure!(
        fs::metadata(&path)?.len() <= MAX_CONFIG_BYTES,
        "moon.toml exceeds size limit"
    );
    let original = fs::read_to_string(&path)?;
    let mut config: ConfigFile =
        toml::from_str(&original).map_err(|_| anyhow::anyhow!("invalid moon.toml"))?;
    config.learning.validate(home)?;
    let master = config
        .master
        .context("master profile is not configured")?
        .resolve()?;
    ensure!(master.profile_id == profile_id, "master profile changed");
    ensure!(
        identity.source != EnrollmentSource::FirstHuman || master.enroll_first_direct_sender,
        "first direct sender enrollment is disabled"
    );
    let index = master
        .channels
        .iter()
        .position(|c| c.channel == identity.channel && c.account_id == identity.account_id);
    if let Some(i) = index {
        let existing = &master.channels[i];
        let matches = existing.sender_id == identity.sender_id;
        if !matches || identity.source <= existing.source {
            return Ok(Enrollment {
                enrolled: false,
                matches,
                source: existing.source,
            });
        }
    }

    let mut doc: DocumentMut = original
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid moon.toml"))?;
    let master_table = doc
        .get_mut("master")
        .and_then(Item::as_table_like_mut)
        .context("master table missing")?;
    if !master_table.contains_key("channels") {
        master_table.insert("channels", value(Array::new()));
    }
    let channels = master_table
        .get_mut("channels")
        .context("master channels missing")?;
    if let Some(tables) = channels.as_array_of_tables_mut() {
        if let Some(i) = index {
            tables
                .get_mut(i)
                .context("channel entry missing")?
                .insert("source", value(identity.source.name()));
        } else {
            let mut table = Table::new();
            table.insert("channel", value(&identity.channel));
            table.insert("account_id", value(&identity.account_id));
            table.insert("sender_id", value(&identity.sender_id));
            table.insert("source", value(identity.source.name()));
            tables.push(table);
        }
    } else {
        let rows = channels.as_array_mut().context("invalid master.channels")?;
        if let Some(i) = index {
            rows.get_mut(i)
                .and_then(|v| v.as_inline_table_mut())
                .context("invalid master channel")?
                .insert("source", identity.source.name().into());
        } else {
            let mut row = InlineTable::new();
            row.insert("channel", identity.channel.as_str().into());
            row.insert("account_id", identity.account_id.as_str().into());
            row.insert("sender_id", identity.sender_id.as_str().into());
            row.insert("source", identity.source.name().into());
            rows.push(row);
        }
    }
    let updated = doc.to_string();
    ensure!(
        updated.len() as u64 <= MAX_CONFIG_BYTES,
        "moon.toml exceeds size limit"
    );
    let mut staged = tempfile::NamedTempFile::new_in(home)?;
    staged.write_all(updated.as_bytes())?;
    staged.as_file().sync_all()?;
    ensure!(
        fs::read_to_string(&path)? == original,
        "moon.toml changed during enrollment"
    );
    staged.persist(&path)?;
    File::open(home)?.sync_all()?;
    Ok(Enrollment {
        enrolled: true,
        matches: true,
        source: identity.source,
    })
}
