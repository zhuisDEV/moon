use crate::commands::CommandReport;
use crate::moon::openai_codex_auth::{self, LoginCallbackMode};
use anyhow::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoonLoginProvider {
    OpenAiCodex,
}

#[derive(Debug, Clone)]
pub struct MoonLoginOptions {
    pub provider: MoonLoginProvider,
    pub headless: bool,
}

pub fn run(opts: &MoonLoginOptions) -> Result<CommandReport> {
    let mut report = CommandReport::new("login");
    match opts.provider {
        MoonLoginProvider::OpenAiCodex => {
            let result = openai_codex_auth::login(opts.headless)?;
            report.detail("login.provider=openai-codex");
            report.detail(format!(
                "login.callback_mode={}",
                match result.callback_mode {
                    LoginCallbackMode::BrowserCallback => "browser",
                    LoginCallbackMode::ManualCode => "manual",
                }
            ));
            report.detail(format!("login.browser_opened={}", result.browser_opened));
            report.detail(format!(
                "login.auth_store={}",
                result.auth_store_path.display()
            ));
            report.detail(format!(
                "login.expires_at_epoch_ms={}",
                result.expires_at_epoch_ms
            ));
            if let Some(email) = result.email {
                report.detail(format!("login.email={email}"));
            }
            if let Some(account_id) = result.account_id {
                report.detail(format!("login.account_id={account_id}"));
            }
        }
    }
    Ok(report)
}
