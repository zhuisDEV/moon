use crate::assets::{plugin_asset_contents, write_plugin_assets};
use crate::openclaw::gateway;
use crate::openclaw::paths::OpenClawPaths;
use crate::openclaw::plugin_index;
use anyhow::Result;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct PluginInstallOutcome {
    pub changed: bool,
    pub path: String,
    pub source_path: String,
    pub provenance_changed: bool,
    pub used_openclaw_installer: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct PluginInstallOptions {
    pub dry_run: bool,
    pub force_openclaw_install: bool,
}

fn plugin_dir_matches_assets(plugin_dir: &Path) -> Result<bool> {
    if !plugin_dir.exists() {
        return Ok(false);
    }

    for (name, expected) in plugin_asset_contents() {
        let file = plugin_dir.join(name);
        if !file.exists() {
            return Ok(false);
        }
        let current = fs::read_to_string(&file)?;
        if current != expected {
            return Ok(false);
        }
    }

    Ok(true)
}

pub fn install_plugin(paths: &OpenClawPaths, dry_run: bool) -> Result<PluginInstallOutcome> {
    install_plugin_with_options(
        paths,
        PluginInstallOptions {
            dry_run,
            force_openclaw_install: false,
        },
    )
}

pub fn install_plugin_with_options(
    paths: &OpenClawPaths,
    opts: PluginInstallOptions,
) -> Result<PluginInstallOutcome> {
    let source_matches = plugin_dir_matches_assets(&paths.plugin_source_dir)?;
    let target_matches = plugin_dir_matches_assets(&paths.plugin_dir)?;
    let index_matches = plugin_index::install_index_record_matches(paths).unwrap_or(false);
    let needs_source_update = !source_matches;
    let needs_install = opts.force_openclaw_install || !target_matches || !index_matches;
    let mut used_openclaw_installer = false;

    if !opts.dry_run {
        if needs_source_update {
            if paths.plugin_source_dir.exists() {
                fs::remove_dir_all(&paths.plugin_source_dir)?;
            }
            write_plugin_assets(&paths.plugin_source_dir)?;
        }

        if needs_install {
            match gateway::try_plugins_install(&paths.plugin_source_dir, true) {
                Ok(()) => {
                    used_openclaw_installer = true;
                }
                Err(_) => {
                    fs::create_dir_all(&paths.extensions_dir)?;
                    if paths.plugin_dir.exists() {
                        fs::remove_dir_all(&paths.plugin_dir)?;
                    }
                    write_plugin_assets(&paths.plugin_dir)?;
                }
            }
        }
    }

    Ok(PluginInstallOutcome {
        changed: needs_source_update || needs_install,
        path: paths.plugin_dir.display().to_string(),
        source_path: paths.plugin_source_dir.display().to_string(),
        provenance_changed: !index_matches,
        used_openclaw_installer,
    })
}
