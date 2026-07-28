use anyhow::{Context, Result};
use regex::Regex;
use serde_json::Value;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redacted<T> {
    pub value: T,
    pub count: usize,
}

pub fn redact_text(input: &str) -> Redacted<String> {
    let mut value = input.to_string();
    let mut count = 0;

    for (pattern, replacement) in [
        (private_key_pattern(), "<redacted-private-key>"),
        (database_url_pattern(), "<redacted-database-url>"),
        (cookie_pattern(), "${label}: <redacted>"),
        (assignment_pattern(), "${label}=<redacted>"),
        (bearer_pattern(), "Bearer <redacted>"),
        (openai_key_pattern(), "<redacted>"),
        (aws_access_key_pattern(), "<redacted>"),
        (github_token_pattern(), "<redacted>"),
        (slack_token_pattern(), "<redacted>"),
    ] {
        count += pattern.find_iter(&value).count();
        value = pattern.replace_all(&value, replacement).into_owned();
    }

    Redacted { value, count }
}

pub fn redact_json(raw: &str) -> Result<Redacted<String>> {
    let mut value: Value = serde_json::from_str(raw).context("metadata_json must be valid JSON")?;
    let count = redact_json_value(&mut value);
    Ok(Redacted {
        value: serde_json::to_string(&value)?,
        count,
    })
}

fn redact_json_value(value: &mut Value) -> usize {
    match value {
        Value::String(text) => {
            let redacted = redact_text(text);
            *text = redacted.value;
            redacted.count
        }
        Value::Array(items) => items.iter_mut().map(redact_json_value).sum(),
        Value::Object(entries) => entries
            .iter_mut()
            .map(|(key, value)| {
                if is_sensitive_key(key) {
                    let changed = usize::from(value.as_str() != Some("<redacted>"));
                    *value = Value::String("<redacted>".to_string());
                    changed
                } else {
                    redact_json_value(value)
                }
            })
            .sum(),
        _ => 0,
    }
}

fn is_sensitive_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "apikey"
            | "accesstoken"
            | "refreshtoken"
            | "token"
            | "secret"
            | "password"
            | "credential"
            | "credentials"
            | "authorization"
            | "cookie"
            | "setcookie"
            | "sessioncookie"
            | "sessionid"
            | "clientsecret"
            | "privatekey"
            | "databaseurl"
            | "connectionstring"
            | "awssecretaccesskey"
            | "awsaccesskeyid"
            | "slackbottoken"
            | "webhookurl"
    )
}

fn assignment_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?im)(?P<label>\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|aws[_-]?(?:access[_-]?key[_-]?id|secret[_-]?access[_-]?key)|slack[_-]?bot[_-]?token|database[_-]?url|connection[_-]?string|session[_-]?(?:cookie|id)|webhook[_-]?url|secret|password|private[_-]?key)\b)\s*[:=]\s*(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;]+)"#,
        )
        .expect("valid assignment regex")
    })
}

fn bearer_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\bBearer\s+[A-Za-z0-9._~+/=-]{12,}").expect("valid bearer regex")
    })
}

fn openai_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{12,}").expect("valid key regex"))
}

fn aws_access_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"\b(?:AKIA|ASIA)[A-Z0-9]{16}\b").expect("valid AWS access key regex")
    })
}

fn github_token_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,})\b")
            .expect("valid GitHub token regex")
    })
}

fn slack_token_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b").expect("valid Slack token regex")
    })
}

fn database_url_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?i)\b(?:postgres(?:ql)?|mysql|mongodb(?:\+srv)?|redis)://[^\s:/]+:[^@\s]+@[^\s]+",
        )
        .expect("valid database URL regex")
    })
}

fn cookie_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?im)(?P<label>\b(?:cookie|set-cookie|session[_-]?cookie)\b)\s*[:=]\s*[^\r\n]+",
        )
        .expect("valid cookie regex")
    })
}

fn private_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----",
        )
        .expect("valid private key regex")
    })
}

#[cfg(test)]
mod tests {
    use super::{redact_json, redact_text};

    #[test]
    fn redacts_common_secret_shapes_without_hiding_normal_context() {
        let input = "Decision: keep SQLite.\nAPI_KEY=very-secret-value\nAuthorization: Bearer abcdefghijklmnop\nkey sk-example123456789";
        let redacted = redact_text(input);
        assert_eq!(redacted.count, 3);
        assert!(redacted.value.contains("Decision: keep SQLite."));
        assert!(!redacted.value.contains("very-secret-value"));
        assert!(!redacted.value.contains("abcdefghijklmnop"));
        assert!(!redacted.value.contains("sk-example"));
    }

    #[test]
    fn redacts_sensitive_json_fields_recursively() {
        let redacted = redact_json(
            r#"{"nested":{"token":"secret-token","note":"Bearer abcdefghijklmnop"},"ok":true}"#,
        )
        .expect("redact");
        assert_eq!(redacted.count, 2);
        assert!(redacted.value.contains(r#""token":"<redacted>""#));
        assert!(!redacted.value.contains("secret-token"));
    }

    #[test]
    fn redacts_cloud_tokens_database_urls_cookies_and_private_keys() {
        let slack_fixture = [
            "xox",
            "b-111111111111-222222222222-",
            "abcdefghijklmnopqrstuvwx",
        ]
        .concat();
        let input = format!(
            concat!(
                "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE\n",
                "SLACK_BOT_TOKEN={slack_fixture}\n",
                "DATABASE_URL=postgres://audit_user:audit_password@localhost/audit\n",
                "Cookie: session=abcdefghijklmnopqrstuvwxyz123456\n",
                "-----BEGIN PRIVATE KEY-----\nprivate-material\n-----END PRIVATE KEY-----\n",
            ),
            slack_fixture = slack_fixture,
        );
        let redacted = redact_text(&input);
        assert!(redacted.count >= 5);
        for secret in [
            "AKIAIOSFODNN7EXAMPLE",
            "xoxb-",
            "postgres://",
            "abcdefghijklmnopqrstuvwxyz123456",
            "private-material",
        ] {
            assert!(!redacted.value.contains(secret));
        }

        let metadata = redact_json(
            r#"{"aws_secret_access_key":"secret","database_url":"postgres://user:pass@host/db","private_key":"private"}"#,
        )
        .expect("metadata");
        assert_eq!(metadata.count, 3);
        assert!(!metadata.value.contains("postgres://"));
        assert!(!metadata.value.contains(r#""private""#));
    }
}
