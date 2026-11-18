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
    let Redacted {
        mut value,
        mut count,
    } = redact_quoted_assignments(input);

    for (pattern, replacement) in [
        (private_key_pattern(), "<redacted-private-key>"),
        (database_url_pattern(), "<redacted-database-url>"),
        (cookie_pattern(), "${label}: <redacted>"),
        (prose_credential_pattern(), "<redacted-credential>"),
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

fn redact_quoted_assignments(input: &str) -> Redacted<String> {
    let mut value = String::with_capacity(input.len());
    let mut copied_until = 0;
    let mut count = 0;
    for captures in quoted_assignment_pattern().captures_iter(input) {
        let assignment = captures.get(0).expect("matched assignment");
        if assignment.start() < copied_until {
            continue;
        }
        let raw_key = captures.name("key").expect("matched key").as_str();
        let key = if raw_key.starts_with('"') {
            serde_json::from_str::<String>(raw_key).unwrap_or_default()
        } else {
            raw_key[1..raw_key.len() - 1]
                .replace("\\'", "'")
                .replace("\\\\", "\\")
        };
        if !is_sensitive_key(&key) {
            continue;
        }

        let tail = &input[assignment.end()..];
        let value_bytes = assignment_value_bytes(tail);
        if value_bytes == 0 {
            continue;
        }
        value.push_str(&input[copied_until..assignment.end()]);
        if tail.starts_with('\'') {
            value.push_str("'<redacted>'");
        } else {
            value.push_str("\"<redacted>\"");
        }
        copied_until = assignment.end() + value_bytes;
        count += 1;
    }
    value.push_str(&input[copied_until..]);
    Redacted { value, count }
}

fn assignment_value_bytes(input: &str) -> usize {
    // Parse a complete JSON value so escapes and credential objects cannot
    // leave a suffix of the secret behind in a transcript or code fence.
    let mut values = serde_json::Deserializer::from_str(input).into_iter::<Value>();
    if matches!(values.next(), Some(Ok(_))) {
        return values.byte_offset();
    }
    if let Some(quote @ ('\'' | '"')) = input.chars().next() {
        let mut escaped = false;
        for (offset, character) in input.char_indices().skip(1) {
            if matches!(character, '\r' | '\n') {
                return offset;
            }
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == quote {
                return offset + character.len_utf8();
            }
        }
        return input.len();
    }
    input
        .find(|character: char| {
            character.is_whitespace() || matches!(character, ',' | ';' | '}' | ']')
        })
        .unwrap_or(input.len())
}

fn quoted_assignment_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r#"(?P<key>"(?:\\[^\r\n]|[^"\\\r\n])*"|'(?:\\[^\r\n]|[^'\\\r\n])*')\s*[:=]\s*"#)
            .expect("valid quoted assignment regex")
    })
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

// A conversational credential can contain spaces. Redact the remainder of
// that line instead of preserving part of a passphrase or a recovery-code list.
fn prose_credential_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| Regex::new(
        r"(?im)(?:\b(?:password|passphrase|recovery codes?|backup codes?|one[- ]time (?:code|password)|(?:api|private) key|access token)\s+(?:is|are)\s+|\b(?:passphrase|recovery codes?|backup codes?|one[- ]time code)\s*[:=]\s*|(?:密码|密碼|恢复码|恢復碼|验证码|驗證碼)\s*(?:是|为|為|[:：=])\s*)[^\r\n]+"
    ).expect("valid conversational credential regex"))
}

fn assignment_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?im)(?P<label>\b(?:api[_-]?key|access[_-]?token|refresh[_-]?token|client[_-]?secret|aws[_-]?(?:access[_-]?key[_-]?id|secret[_-]?access[_-]?key)|slack[_-]?bot[_-]?token|database[_-]?url|connection[_-]?string|session[_-]?(?:cookie|id)|webhook[_-]?url|secret|password|private[_-]?key)\b)\s*[:=]\s*(?:"(?:\\[^\r\n]|[^"\\\r\n])*"|'(?:\\[^\r\n]|[^'\\\r\n])*'|[^\s,;]+)"#,
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
    fn redacts_quoted_credentials_inside_prose_and_code_fences() {
        let input = r#"The configuration was:
```json
{"password":"synthetic-prefix\"synthetic-suffix\\synthetic-end", "API-Key":"synthetic-api", "note":"keep this note"}
```
The next example has {"accessToken":"synthetic-access", "pass\u0077ord":"synthetic-unicode"}.
{'client_secret': 'synthetic-single\'synthetic-tail\\synthetic-last', 'enabled': true}
{"credentials":{"first":"synthetic-nested", "second":["synthetic-array"]},"ok":true}
password="synthetic-unquoted\"synthetic-remainder"
"#;
        let redacted = redact_text(input);
        assert_eq!(redacted.count, 7);
        assert!(!redacted.value.contains("synthetic-"));
        assert!(redacted.value.contains(r#""note":"keep this note""#));
        assert!(redacted.value.contains("'enabled': true"));
        assert!(redacted.value.contains(r#""ok":true"#));
        assert!(redacted.value.contains("```json"));
        assert_eq!(redact_text(&redacted.value).value, redacted.value);
    }

    #[test]
    fn quoted_credentials_use_the_metadata_sensitive_key_policy() {
        for key in [
            "api_key",
            "access-token",
            "refreshToken",
            "token",
            "secret",
            "Password",
            "credential",
            "credentials",
            "Authorization",
            "Cookie",
            "Set-Cookie",
            "session_cookie",
            "sessionId",
            "clientSecret",
            "private_key",
            "databaseUrl",
            "connection_string",
            "aws_secret_access_key",
            "AWS_ACCESS_KEY_ID",
            "slack_bot_token",
            "webhook-url",
        ] {
            let input = serde_json::json!({key: "synthetic-credential"}).to_string();
            let redacted = redact_text(&input);
            assert_eq!(redacted.count, 1, "key {key}");
            assert!(
                !redacted.value.contains("synthetic-credential"),
                "key {key}"
            );
            assert!(serde_json::from_str::<serde_json::Value>(&redacted.value).is_ok());
        }
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
