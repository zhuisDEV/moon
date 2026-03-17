use anyhow::{Context, Result, anyhow};
use std::env;
use std::path::PathBuf;

fn moon_env_path(moon_home: Option<String>) -> Result<PathBuf> {
    let Some(raw) = moon_home else {
        return Err(anyhow!(
            "MOON_HOME is not set; moon only loads environment from $MOON_HOME/.env"
        ));
    };

    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(anyhow!(
            "MOON_HOME is empty; moon only loads environment from $MOON_HOME/.env"
        ));
    }

    Ok(PathBuf::from(trimmed).join(".env"))
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

    #[test]
    fn moon_env_path_rejects_unset_moon_home() {
        let err = moon_env_path(None).expect_err("missing moon home should fail");
        assert!(
            err.to_string().contains("MOON_HOME is not set"),
            "unexpected error: {err:#}"
        );
    }

    #[test]
    fn moon_env_path_rejects_empty_moon_home() {
        let err = moon_env_path(Some("   ".to_string())).expect_err("empty moon home should fail");
        assert!(
            err.to_string().contains("MOON_HOME is empty"),
            "unexpected error: {err:#}"
        );
    }
}
