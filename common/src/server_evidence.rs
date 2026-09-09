use regex::Regex;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

use super::agent_runtime::{env_enabled, env_u64, sha256_value};

pub fn server_evidence(value: &Value) -> Value {
    let content_hash = sha256_value(value);
    if !env_enabled("TALLY_SERVER_EVIDENCE_ENABLED", true) {
        return json!({
            "schema_version": "tally-server-evidence.v1",
            "visibility": "private",
            "text": Value::Null,
            "content_hash": content_hash,
            "truncated": false,
            "redaction_count": 0,
            "risk_signals": [],
            "disabled": true,
        });
    }
    let max_chars = env_u64("TALLY_SERVER_EVIDENCE_MAX_CHARS", 8_192).clamp(256, 32_768) as usize;
    let mut projection_budget = max_chars.saturating_mul(4);
    let projected = bounded_projection(value, 0, &mut projection_budget);
    let mut redaction_count = 0_u64;
    let redacted = redact_sensitive_value(&projected, None, &mut redaction_count);
    let text = match &redacted {
        Value::String(value) => value.clone(),
        _ => serde_json::to_string(&redacted).unwrap_or_default(),
    };
    let (text, truncated) = truncate_chars(&text, max_chars);
    json!({
        "schema_version": "tally-server-evidence.v1",
        "visibility": "arbitrator",
        "risk_signals": detect_risk_signals(&text),
        "text": text,
        "content_hash": content_hash,
        "truncated": truncated,
        "redaction_count": redaction_count,
    })
}

fn bounded_projection(value: &Value, depth: usize, budget: &mut usize) -> Value {
    if *budget == 0 || depth > 32 {
        return Value::String("[TRUNCATED]".to_string());
    }
    *budget = budget.saturating_sub(1);
    match value {
        Value::String(value) => {
            let mut chars = value.chars();
            let projected = chars.by_ref().take(*budget).collect::<String>();
            let consumed = projected.chars().count();
            *budget = budget.saturating_sub(consumed);
            if chars.next().is_some() {
                Value::String(format!("{projected}[TRUNCATED]"))
            } else {
                Value::String(projected)
            }
        }
        Value::Array(items) => {
            let mut projected = Vec::new();
            for item in items {
                if *budget == 0 {
                    projected.push(Value::String("[TRUNCATED]".to_string()));
                    break;
                }
                projected.push(bounded_projection(item, depth + 1, budget));
            }
            Value::Array(projected)
        }
        Value::Object(map) => {
            let mut projected = serde_json::Map::new();
            for (key, item) in map {
                if *budget == 0 {
                    projected.insert("_tally_truncated".to_string(), Value::Bool(true));
                    break;
                }
                let key_chars = key.chars().take((*budget).saturating_add(1)).count();
                if key_chars > *budget {
                    projected.insert("_tally_truncated".to_string(), Value::Bool(true));
                    break;
                }
                *budget -= key_chars;
                projected.insert(key.clone(), bounded_projection(item, depth + 1, budget));
            }
            Value::Object(projected)
        }
        _ => value.clone(),
    }
}

pub fn evidence_summary(evidence: &Value, fallback: &str) -> String {
    evidence["text"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| value.chars().take(240).collect())
        .unwrap_or_else(|| fallback.to_string())
}

fn redact_sensitive_value(value: &Value, key: Option<&str>, redactions: &mut u64) -> Value {
    if key.is_some_and(sensitive_key) {
        *redactions += 1;
        return Value::String("[REDACTED]".to_string());
    }
    match value {
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        redact_sensitive_value(value, Some(key), redactions),
                    )
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|value| redact_sensitive_value(value, None, redactions))
                .collect(),
        ),
        Value::String(value) => Value::String(redact_inline_secrets(value, redactions)),
        _ => value.clone(),
    }
}

fn sensitive_key(key: &str) -> bool {
    let normalized = key.to_ascii_lowercase().replace(['-', ' '], "_");
    matches!(
        normalized.as_str(),
        "authorization"
            | "cookie"
            | "credential"
            | "credentials"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "api_key"
            | "apikey"
            | "access_key"
            | "private_key"
    ) || ["_password", "_secret", "_token", "_api_key", "_credential"]
        .iter()
        .any(|suffix| normalized.ends_with(suffix))
}

fn redact_inline_secrets(value: &str, redactions: &mut u64) -> String {
    static SECRET_ASSIGNMENT: OnceLock<Regex> = OnceLock::new();
    static LABELED_SECRET: OnceLock<Regex> = OnceLock::new();
    static AUTHORIZATION_VALUE: OnceLock<Regex> = OnceLock::new();
    static KNOWN_SECRET: OnceLock<Regex> = OnceLock::new();
    static PRIVATE_KEY: OnceLock<Regex> = OnceLock::new();
    let assignment = SECRET_ASSIGNMENT.get_or_init(|| {
        Regex::new(
            r#"(?i)\b([A-Z0-9_]*(?:API[_-]?KEY|ACCESS[_-]?KEY|SECRET|TOKEN|PASSWORD|PASSWD|CREDENTIAL)[A-Z0-9_]*)\b\s*=\s*(?:\"[^\"\r\n]*\"|'[^'\r\n]*'|[^\s;&|]+)"#,
        )
        .expect("valid secret-assignment pattern")
    });
    let labeled = LABELED_SECRET.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(api[ _-]?key|access[ _-]?key|secret|token|password|authorization|cookie|credential)\b\s*(?::|=|\bis\b)?\s*[\"']?([A-Za-z0-9_./+=-]{8,})"#,
        )
        .expect("valid labeled-secret pattern")
    });
    let known = KNOWN_SECRET.get_or_init(|| {
        Regex::new(
            r#"\b(?:sk-[A-Za-z0-9_-]{12,}|gh[pousr]_[A-Za-z0-9_]{16,}|github_pat_[A-Za-z0-9_]{16,}|(?:AKIA|ASIA)[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})\b"#,
        )
        .expect("valid known-secret pattern")
    });
    let authorization = AUTHORIZATION_VALUE.get_or_init(|| {
        Regex::new(r#"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{8,}"#)
            .expect("valid authorization pattern")
    });
    let private_key = PRIVATE_KEY.get_or_init(|| {
        Regex::new(
            r#"(?s)-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?-----END [A-Z0-9 ]*PRIVATE KEY-----"#,
        )
        .expect("valid private-key pattern")
    });
    let assigned = assignment
        .replace_all(value, |captures: &regex::Captures<'_>| {
            *redactions += 1;
            format!("{}=[REDACTED]", &captures[1])
        })
        .into_owned();
    let labeled = labeled
        .replace_all(&assigned, |captures: &regex::Captures<'_>| {
            *redactions += 1;
            format!("{}=[REDACTED]", &captures[1])
        })
        .into_owned();
    let authorized = authorization
        .replace_all(&labeled, |captures: &regex::Captures<'_>| {
            *redactions += 1;
            format!("{} [REDACTED]", &captures[1])
        })
        .into_owned();
    let known = known
        .replace_all(&authorized, |_: &regex::Captures<'_>| {
            *redactions += 1;
            "[REDACTED]".to_string()
        })
        .into_owned();
    private_key
        .replace_all(&known, |_: &regex::Captures<'_>| {
            *redactions += 1;
            "[REDACTED PRIVATE KEY]".to_string()
        })
        .into_owned()
}

fn detect_risk_signals(text: &str) -> Vec<String> {
    let text = text.to_ascii_lowercase();
    let rules: &[(&str, &[&str])] = &[
        (
            "destructive_change",
            &[
                "rm -rf",
                "git reset --hard",
                "git clean -fd",
                "git push --force",
                "drop table",
                "truncate table",
                "format c:",
                "del /f",
            ],
        ),
        (
            "credential_access",
            &[
                ".aws/credentials",
                ".ssh/",
                "private_key",
                "keychain",
                "api_key",
                "password",
                ".env",
            ],
        ),
        (
            "privilege_escalation",
            &["sudo ", "runas ", "chmod 777", "setfacl ", "takeown "],
        ),
        (
            "external_transfer",
            &[
                "curl ",
                "wget ",
                "scp ",
                "rsync ",
                "nc ",
                "invoke-webrequest",
            ],
        ),
        (
            "persistence_change",
            &[
                "crontab",
                "launchctl",
                "systemctl enable",
                "schtasks",
                "currentversion\\run",
            ],
        ),
        (
            "dynamic_execution",
            &[
                "bash -c",
                "sh -c",
                "powershell -enc",
                "invoke-expression",
                "eval(",
            ],
        ),
    ];
    let mut signals = BTreeSet::new();
    for (signal, patterns) in rules {
        if patterns.iter().any(|pattern| text.contains(pattern)) {
            signals.insert((*signal).to_string());
        }
    }
    signals.into_iter().collect()
}

fn truncate_chars(value: &str, max_chars: usize) -> (String, bool) {
    let mut chars = value.chars();
    let text = chars.by_ref().take(max_chars).collect::<String>();
    (text, chars.next().is_some())
}

#[cfg(test)]
mod tests {
    use super::server_evidence;
    use serde_json::json;

    #[test]
    fn evidence_is_bounded_redacted_and_actionable() {
        let evidence = server_evidence(&json!({
            "command": "sudo rm -rf /tmp/example && curl https://example.test; AWS_SECRET_ACCESS_KEY='not-for-the-server'; Authorization: Bearer abcdefghijklmnop",
            "api_key": "THIS_MUST_NOT_LEAVE_THE_DEVICE",
            "nested": {"token": "ghp_abcdefghijklmnopqrstuvwxyz"},
            "key": "-----BEGIN PRIVATE KEY-----\nsecret material\n-----END PRIVATE KEY-----",
        }));
        let text = evidence["text"].as_str().unwrap();
        assert!(text.contains("rm -rf"));
        assert!(!text.contains("THIS_MUST_NOT_LEAVE_THE_DEVICE"));
        assert!(!text.contains("ghp_abcdefghijklmnopqrstuvwxyz"));
        assert!(!text.contains("not-for-the-server"));
        assert!(!text.contains("abcdefghijklmnop"));
        assert!(!text.contains("secret material"));
        assert!(evidence["redaction_count"].as_u64().unwrap() >= 5);
        let signals = evidence["risk_signals"].as_array().unwrap();
        assert!(signals.contains(&json!("destructive_change")));
        assert!(signals.contains(&json!("external_transfer")));
        assert!(signals.contains(&json!("privilege_escalation")));
    }
}
