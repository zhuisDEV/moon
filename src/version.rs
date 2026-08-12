use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const BUNDLE_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VersionInfo {
    pub ok: bool,
    pub name: String,
    pub version: String,
    pub git_commit: String,
    pub git_dirty: Option<bool>,
    pub build_target: String,
    pub build_profile: String,
    pub executable: String,
    pub canonical_executable: String,
    pub canonical: bool,
    pub bundle_format: u32,
}

impl VersionInfo {
    pub fn current() -> Result<Self> {
        let home = env::var_os("MOON_HOME").map(PathBuf::from);
        Self::current_for_optional_home(home.as_deref())
    }

    pub fn current_for_home(home: &Path) -> Result<Self> {
        Self::current_for_optional_home(Some(home))
    }

    fn current_for_optional_home(home: Option<&Path>) -> Result<Self> {
        let executable = normalized_current_executable()?;
        let canonical_executable = canonical_executable(home)?;
        let canonical = same_file(&executable, &canonical_executable);

        Ok(Self {
            ok: true,
            name: env!("CARGO_PKG_NAME").to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            git_commit: env!("MOON_GIT_COMMIT").to_owned(),
            git_dirty: git_dirty(),
            build_target: env!("MOON_BUILD_TARGET").to_owned(),
            build_profile: env!("MOON_BUILD_PROFILE").to_owned(),
            executable: executable.to_string_lossy().into_owned(),
            canonical_executable: canonical_executable.to_string_lossy().into_owned(),
            canonical,
            bundle_format: BUNDLE_FORMAT,
        })
    }
}

fn git_dirty() -> Option<bool> {
    match env!("MOON_GIT_DIRTY") {
        "true" => Some(true),
        "false" => Some(false),
        "unknown" => None,
        _ => unreachable!("build.rs validates MOON_GIT_DIRTY"),
    }
}

fn normalized_current_executable() -> Result<PathBuf> {
    let executable = env::current_exe().context("current executable could not be resolved")?;
    Ok(fs::canonicalize(&executable).unwrap_or(executable))
}

fn canonical_executable(explicit_home: Option<&Path>) -> Result<PathBuf> {
    let home = explicit_home
        .map(Path::to_path_buf)
        .or_else(dirs::home_dir)
        .context("home directory could not be resolved")?;
    Ok(home
        .join("bin")
        .join(format!("moon{}", env::consts::EXE_SUFFIX)))
}

fn same_file(left: &Path, right: &Path) -> bool {
    let Ok(left) = fs::canonicalize(left) else {
        return false;
    };
    let Ok(right) = fs::canonicalize(right) else {
        return false;
    };
    left == right
}

#[cfg(test)]
mod tests {
    use super::same_file;
    use std::fs;

    #[test]
    fn missing_paths_are_never_the_same_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let missing = temp.path().join("missing");
        assert!(!same_file(&missing, &missing));
    }

    #[test]
    fn normalized_paths_to_the_same_file_match() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("moon");
        fs::write(&file, "binary").expect("write fixture");
        assert!(same_file(&file, &temp.path().join("./moon")));
    }
}
