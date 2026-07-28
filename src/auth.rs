use anyhow::{Context, Result};
use serde::Serialize;
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_MODEL_PROMPT_BYTES: usize = 128 * 1024;
const MAX_MODEL_OUTPUT_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthLevel {
    OpenClaw,
    Moon,
    Codex,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthCheck {
    pub level: AuthLevel,
    pub available: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthStatusReport {
    pub checks: Vec<AuthCheck>,
    pub selected: Option<AuthLevel>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelOutcome {
    pub auth_level: AuthLevel,
    pub model: String,
    pub reasoning: String,
    pub output: String,
}

#[derive(Debug, Clone)]
pub struct AuthResolver {
    codex_path: PathBuf,
}

impl Default for AuthResolver {
    fn default() -> Self {
        Self {
            codex_path: env::var_os("MOON_CODEX_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("codex")),
        }
    }
}

impl AuthResolver {
    pub fn status(&self, moon_home: &Path, openclaw_available: bool) -> AuthStatusReport {
        let mut checks = vec![AuthCheck {
            level: AuthLevel::OpenClaw,
            available: openclaw_available,
            detail: if openclaw_available {
                "OpenClaw reported an authenticated Codex model runtime".to_string()
            } else {
                "not running inside an authenticated OpenClaw model runtime".to_string()
            },
        }];
        checks.push(self.codex_status(AuthLevel::Moon, Some(&moon_codex_home(moon_home))));
        checks.push(self.codex_status(AuthLevel::Codex, None));
        let selected = checks
            .iter()
            .find(|check| check.available)
            .map(|check| check.level);
        AuthStatusReport { checks, selected }
    }

    pub fn login(&self, moon_home: &Path, device_auth: bool) -> Result<AuthStatusReport> {
        ensure_private_dir(moon_home)?;
        let codex_home = moon_codex_home(moon_home);
        ensure_private_dir(&codex_home)?;
        let mut command = Command::new(&self.codex_path);
        command.arg("login").env("CODEX_HOME", &codex_home);
        if device_auth {
            command.arg("--device-auth");
        }
        let status = command
            .status()
            .with_context(|| format!("failed to launch {}", self.codex_path.display()))?;
        if !status.success() {
            anyhow::bail!("Moon Codex login did not complete successfully");
        }
        Ok(self.status(moon_home, false))
    }

    pub fn execute(
        &self,
        moon_home: &Path,
        prompt: &str,
        model: &str,
        reasoning: &str,
    ) -> Result<ModelOutcome> {
        validate_model_request(prompt, model, reasoning)?;
        let moon_codex_home = moon_codex_home(moon_home);
        let moon_status = self.codex_status(AuthLevel::Moon, Some(&moon_codex_home));
        if moon_status.available {
            match self.execute_at_level(
                AuthLevel::Moon,
                Some(&moon_codex_home),
                prompt,
                model,
                reasoning,
            ) {
                Ok(outcome) => return Ok(outcome),
                Err(ModelFailure::AuthUnavailable) => {}
                Err(ModelFailure::Other(message)) => anyhow::bail!("{message}"),
            }
        }

        let local_status = self.codex_status(AuthLevel::Codex, None);
        if !local_status.available {
            anyhow::bail!(
                "no usable Moon or local Codex login; run `moon auth login` or `codex login`"
            );
        }
        match self.execute_at_level(AuthLevel::Codex, None, prompt, model, reasoning) {
            Ok(outcome) => Ok(outcome),
            Err(ModelFailure::AuthUnavailable) => {
                anyhow::bail!("local Codex authentication became unavailable")
            }
            Err(ModelFailure::Other(message)) => anyhow::bail!("{message}"),
        }
    }

    fn codex_status(&self, level: AuthLevel, codex_home: Option<&Path>) -> AuthCheck {
        let mut command = Command::new(&self.codex_path);
        command.args(["login", "status"]);
        apply_codex_home(&mut command, codex_home);
        let result = command.output();
        match result {
            Ok(output) if output.status.success() => AuthCheck {
                level,
                available: true,
                detail: login_method(&output.stdout, &output.stderr),
            },
            Ok(_) => AuthCheck {
                level,
                available: false,
                detail: "not logged in".to_string(),
            },
            Err(_) => AuthCheck {
                level,
                available: false,
                detail: "Codex runtime unavailable".to_string(),
            },
        }
    }

    fn execute_at_level(
        &self,
        level: AuthLevel,
        codex_home: Option<&Path>,
        prompt: &str,
        model: &str,
        reasoning: &str,
    ) -> std::result::Result<ModelOutcome, ModelFailure> {
        let temp = tempfile::Builder::new()
            .prefix("moon-codex-")
            .tempdir()
            .map_err(|_| ModelFailure::Other("failed to create private model workspace".into()))?;
        let output_path = temp.path().join("last-message.txt");
        let mut command = Command::new(&self.codex_path);
        command
            .args([
                "exec",
                "--ephemeral",
                "--ignore-user-config",
                "--ignore-rules",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "-C",
            ])
            .arg(temp.path())
            .args(["--model", model, "--config"])
            .arg(format!("model_reasoning_effort=\"{reasoning}\""))
            .arg("--output-last-message")
            .arg(&output_path)
            .arg("-")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        apply_codex_home(&mut command, codex_home);
        let mut child = command
            .spawn()
            .map_err(|_| ModelFailure::Other("failed to launch Codex runtime".into()))?;
        child
            .stdin
            .take()
            .ok_or_else(|| ModelFailure::Other("Codex runtime stdin unavailable".into()))?
            .write_all(prompt.as_bytes())
            .map_err(|_| ModelFailure::Other("failed to send model request".into()))?;
        let output = child
            .wait_with_output()
            .map_err(|_| ModelFailure::Other("failed to wait for Codex runtime".into()))?;
        if !output.status.success() {
            let diagnostic = String::from_utf8_lossy(&output.stderr);
            return if is_auth_unavailable(&diagnostic) {
                Err(ModelFailure::AuthUnavailable)
            } else {
                Err(ModelFailure::Other(format!(
                    "Codex model request failed at {} authentication level",
                    auth_level_name(level)
                )))
            };
        }
        let metadata = fs::metadata(&output_path)
            .map_err(|_| ModelFailure::Other("Codex returned no final response".into()))?;
        if metadata.len() > MAX_MODEL_OUTPUT_BYTES {
            return Err(ModelFailure::Other(
                "Codex final response exceeded the output limit".into(),
            ));
        }
        let output = fs::read_to_string(&output_path)
            .map_err(|_| ModelFailure::Other("failed to read Codex final response".into()))?;
        if output.trim().is_empty() {
            return Err(ModelFailure::Other(
                "Codex returned an empty final response".into(),
            ));
        }
        Ok(ModelOutcome {
            auth_level: level,
            model: model.to_string(),
            reasoning: reasoning.to_string(),
            output: output.trim().to_string(),
        })
    }
}

#[derive(Debug)]
enum ModelFailure {
    AuthUnavailable,
    Other(String),
}

fn moon_codex_home(moon_home: &Path) -> PathBuf {
    moon_home.join("auth/codex")
}

fn apply_codex_home(command: &mut Command, codex_home: Option<&Path>) {
    if let Some(codex_home) = codex_home {
        command.env("CODEX_HOME", codex_home);
    }
}

fn login_method(stdout: &[u8], stderr: &[u8]) -> String {
    let mut text = String::from_utf8_lossy(stdout).to_ascii_lowercase();
    text.push_str(&String::from_utf8_lossy(stderr).to_ascii_lowercase());
    if text.contains("chatgpt") {
        "logged in using ChatGPT".to_string()
    } else if text.contains("api key") {
        "logged in using an API key".to_string()
    } else if text.contains("access token") {
        "logged in using an access token".to_string()
    } else {
        "logged in".to_string()
    }
}

fn is_auth_unavailable(diagnostic: &str) -> bool {
    let lower = diagnostic.to_ascii_lowercase();
    [
        "not logged in",
        "authentication required",
        "oauth expired",
        "unauthorized",
        "no auth profile",
        "missing authentication",
        "status 401",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn auth_level_name(level: AuthLevel) -> &'static str {
    match level {
        AuthLevel::OpenClaw => "OpenClaw",
        AuthLevel::Moon => "Moon",
        AuthLevel::Codex => "local Codex",
    }
}

fn validate_model_request(prompt: &str, model: &str, reasoning: &str) -> Result<()> {
    if prompt.trim().is_empty() {
        anyhow::bail!("model prompt must not be empty");
    }
    if prompt.len() > MAX_MODEL_PROMPT_BYTES {
        anyhow::bail!("model prompt exceeds {MAX_MODEL_PROMPT_BYTES} bytes");
    }
    if model.is_empty()
        || model.len() > 128
        || !model
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || ".:_/-".contains(character))
    {
        anyhow::bail!("model name contains unsupported characters");
    }
    if !["low", "medium", "high", "xhigh"].contains(&reasoning) {
        anyhow::bail!("reasoning must be one of low, medium, high, or xhigh");
    }
    Ok(())
}

fn ensure_private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        if let Some(parent) = path.parent() {
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AuthLevel, is_auth_unavailable, validate_model_request};

    #[test]
    fn only_auth_failures_are_eligible_for_fallback() {
        assert!(is_auth_unavailable("OAuth expired"));
        assert!(is_auth_unavailable("status 401"));
        assert!(!is_auth_unavailable("rate limit status 429"));
        assert!(!is_auth_unavailable("network timeout"));
    }

    #[test]
    fn model_requests_are_bounded_and_model_names_are_safe() {
        validate_model_request("Return READY.", "gpt-5.6-sol", "high").expect("valid");
        assert!(validate_model_request("", "gpt-5.6-sol", "high").is_err());
        assert!(validate_model_request("test", "model; touch /tmp/x", "high").is_err());
        assert!(validate_model_request("test", "gpt-5.6-sol", "turbo").is_err());
        assert_eq!(AuthLevel::Codex, AuthLevel::Codex);
    }
}
