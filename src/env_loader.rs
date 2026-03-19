use anyhow::{Context, Result, anyhow};
use std::env;
use std::path::PathBuf;

fn moon_env_path(moon_home: Option<String>) -> Result<PathBuf> {
    match moon_home {
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return default_moon_env_path();
            }
            Ok(PathBuf::from(trimmed).join(".env"))
        }
        None => default_moon_env_path(),
    }
}

fn default_moon_env_path() -> Result<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Err(anyhow!(
            "HOME directory could not be resolved; moon could not locate ~/.moon/.env"
        ));
    };

    Ok(home.join(".moon").join(".env"))
}

pub fn load_dotenv() -> Result<PathBuf> {
    let path = moon_env_path(env::var("MOON_HOME").ok())?;
    if !path.is_file() {
        return Err(anyhow!(
            "required env file missing: {} (moon only loads environment from $MOON_HOME/.env)",
            path.display()
        ));
    }

    dotenvy::from_path(&path)
        .with_context(|| format!("failed to load env file {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::moon_env_path;
    use std::path::PathBuf;

    #[test]
    fn moon_env_path_uses_moon_home_dot_env() {
        let got = moon_env_path(Some("/workspace".to_string())).expect("moon env path");
        assert_eq!(got, PathBuf::from("/workspace/.env"));
    }
}
