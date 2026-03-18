use anyhow::Result;
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct MoonPaths {
    pub moon_home: PathBuf,
    pub raw_dir: PathBuf,
    pub mds_dir: PathBuf,
    pub mlib_dir: PathBuf,
    pub cleanse_dir: PathBuf,
    pub memory_dir: PathBuf,
    pub memory_file: PathBuf,
    pub logs_dir: PathBuf,
    pub context_engine_dir: PathBuf,
    pub openclaw_sessions_dir: PathBuf,
    pub qmd_bin: PathBuf,
    pub qmd_db: PathBuf,
    pub qmd_config_dir: PathBuf,
    pub moon_home_is_explicit: bool,
}

fn required_home_dir() -> Result<PathBuf> {
    if let Some(home) = dirs::home_dir() {
        return Ok(home);
    }
    Err(anyhow::anyhow!("HOME directory could not be resolved"))
}

fn env_or_default_path(var: &str, fallback: PathBuf) -> PathBuf {
    match env::var(var) {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => fallback,
    }
}

fn moon_home_from_inputs(home: PathBuf, moon_home_env: Option<&str>) -> (PathBuf, bool) {
    match moon_home_env {
        Some(v) if !v.trim().is_empty() => (PathBuf::from(v.trim()), true),
        _ => (home.join(".moon"), false),
    }
}

pub fn resolve_paths() -> Result<MoonPaths> {
    let home = required_home_dir()?;
    let moon_home_env = env::var("MOON_HOME").ok();
    let (moon_home, is_explicit) = moon_home_from_inputs(home.clone(), moon_home_env.as_deref());

    let raw_dir = env_or_default_path("MOON_RAW_DIR", moon_home.join("raw"));
    let mds_dir = env_or_default_path("MOON_MDS_DIR", moon_home.join("mds"));
    let mlib_dir = env_or_default_path("MOON_MLIB_DIR", moon_home.join("mlib"));
    let cleanse_dir = env_or_default_path("MOON_CLEANSE_DIR", moon_home.join("cleanse"));
    let memory_dir = env_or_default_path("MOON_MEMORY_DIR", moon_home.join("memory"));
    let memory_file = env_or_default_path("MOON_MEMORY_FILE", moon_home.join("MEMORY.md"));
    let logs_dir = env_or_default_path("MOON_LOGS_DIR", moon_home.join("logs"));
    let context_engine_dir = moon_home.join("mce");
    let openclaw_sessions_dir = env_or_default_path(
        "OPENCLAW_SESSIONS_DIR",
        home.join(".openclaw/agents/main/sessions"),
    );
    let qmd_bin = env_or_default_path("QMD_BIN", home.join(".bun/bin/qmd"));
    let qmd_runtime_dir = moon_home.join("qmd");
    let qmd_db = env_or_default_path("QMD_DB", qmd_runtime_dir.join("index.sqlite"));
    let qmd_config_dir = env_or_default_path("QMD_CONFIG_DIR", qmd_runtime_dir.join("config"));

    Ok(MoonPaths {
        moon_home,
        raw_dir,
        mds_dir,
        mlib_dir,
        cleanse_dir,
        memory_dir,
        memory_file,
        logs_dir,
        context_engine_dir,
        openclaw_sessions_dir,
        qmd_bin,
        qmd_db,
        qmd_config_dir,
        moon_home_is_explicit: is_explicit,
    })
}

#[cfg(test)]
mod tests {
    use super::moon_home_from_inputs;
    use std::path::PathBuf;

    #[test]
    fn default_moon_home_uses_dot_moon_when_unset() {
        let home = PathBuf::from("/home/alice");
        let (moon_home, is_explicit) = moon_home_from_inputs(home.clone(), None);
        assert_eq!(moon_home, home.join(".moon"));
        assert!(!is_explicit);
    }

    #[test]
    fn explicit_moon_home_is_preserved() {
        let (moon_home, is_explicit) =
            moon_home_from_inputs(PathBuf::from("/home/alice"), Some("/workspace"));
        assert_eq!(moon_home, PathBuf::from("/workspace"));
        assert!(is_explicit);
    }

    #[test]
    fn blank_moon_home_falls_back_to_dot_moon() {
        let home = PathBuf::from("/home/alice");
        let (moon_home, is_explicit) = moon_home_from_inputs(home.clone(), Some("   "));
        assert_eq!(moon_home, home.join(".moon"));
        assert!(!is_explicit);
    }
}
