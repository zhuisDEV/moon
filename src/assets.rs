use anyhow::Result;
use std::fs;
use std::path::Path;

const PACKAGE_JSON: &str = include_str!("../assets/plugin/package.json");
const MANIFEST_JSON: &str = include_str!("../assets/plugin/openclaw.plugin.json");
const INDEX_JS: &str = include_str!("../assets/plugin/index.js");
const README_MD: &str = include_str!("../assets/plugin/README.md");
const RUNTIME_README_MD: &str = include_str!("../README.md");
const RUNTIME_TROUBLESHOOTING_MD: &str = include_str!("../docs/troubleshooting.md");
const RUNTIME_ENV_EXAMPLE: &str = include_str!("../.env.example");
const RUNTIME_MOON_TOML_EXAMPLE: &str = include_str!("../moon.toml.example");
const ADMIN_SKILL_MD: &str = include_str!("../SKILL.md");
const SUBAGENT_SKILL_MD: &str = include_str!("../SKILL_SUBAGENT.md");

pub fn plugin_asset_contents() -> [(&'static str, &'static str); 4] {
    [
        ("package.json", PACKAGE_JSON),
        ("openclaw.plugin.json", MANIFEST_JSON),
        ("index.js", INDEX_JS),
        ("README.md", README_MD),
    ]
}

pub fn write_plugin_assets(target_dir: &Path) -> Result<()> {
    write_named_assets(target_dir, &plugin_asset_contents())?;
    Ok(())
}

pub fn runtime_doc_asset_contents() -> [(&'static str, &'static str); 4] {
    [
        ("README.md", RUNTIME_README_MD),
        ("docs/troubleshooting.md", RUNTIME_TROUBLESHOOTING_MD),
        (".env.example", RUNTIME_ENV_EXAMPLE),
        ("moon.toml.example", RUNTIME_MOON_TOML_EXAMPLE),
    ]
}

pub fn runtime_skill_asset_contents() -> [(&'static str, &'static str); 2] {
    [
        ("moon-admin/SKILL.md", ADMIN_SKILL_MD),
        ("moon-subagent/SKILL.md", SUBAGENT_SKILL_MD),
    ]
}

pub fn write_runtime_docs(target_dir: &Path) -> Result<()> {
    write_named_assets(target_dir, &runtime_doc_asset_contents())
}

pub fn write_runtime_skills(target_dir: &Path) -> Result<()> {
    write_named_assets(target_dir, &runtime_skill_asset_contents())
}

fn write_named_assets(target_dir: &Path, assets: &[(&str, &str)]) -> Result<()> {
    fs::create_dir_all(target_dir)?;
    for (name, content) in assets {
        let path = target_dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
    }
    Ok(())
}
