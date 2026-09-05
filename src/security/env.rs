//! Subprocess environment scrubbing for agent-controlled child processes.
//!
//! Agent-controlled subprocesses must never inherit the full parent
//! environment, which contains secrets (API keys, tokens, database URLs,
//! and so on). These helpers replace the inherited environment with a
//! scrubbed snapshot: only non-sensitive inherited variables plus any
//! explicitly configured values are passed to the child.

use std::collections::HashMap;
use std::ffi::OsStr;

/// Environment variable names that are never inherited by agent-controlled
/// subprocesses.
const SENSITIVE_SEGMENTS: &[&str] = &[
    "AUTH",
    "COOKIE",
    "CREDENTIAL",
    "CREDENTIALS",
    "KEY",
    "PASS",
    "PASSWD",
    "PASSWORD",
    "SECRET",
    "SESSION",
    "TOKEN",
];

/// Replace the inherited environment of `command` with a scrubbed snapshot,
/// then apply explicitly configured values. Names in `passthrough` opt back
/// into inheritance, including names that match the sensitive-name filter.
///
/// Explicit command-scoped values always win over inherited values.
pub(crate) fn configure_environment(
    command: &mut tokio::process::Command,
    explicit: Option<&HashMap<String, String>>,
    passthrough: &[String],
) {
    command.env_clear();

    for (name, value) in std::env::vars_os() {
        if should_inherit(&name, passthrough) {
            command.env(name, value);
        }
    }

    if let Some(explicit) = explicit {
        for (name, value) in explicit {
            command.env(name, value);
        }
    }
}

fn should_inherit(name: &OsStr, passthrough: &[String]) -> bool {
    let name = name.to_string_lossy();
    passthrough
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&name))
        || !is_sensitive_name(&name)
}

/// Returns true when `name` looks like a secret-bearing environment variable
/// that must never be inherited by agent-controlled subprocesses.
pub(crate) fn is_sensitive_name(name: &str) -> bool {
    const SENSITIVE_SUFFIXES: &[&str] = &[
        "_API_BASE",
        "_API_KEY",
        "_BASE_URL",
        "_DSN",
        "_PASSWORD",
        "_SECRET",
        "_TOKEN",
        "DATABASE_URL",
        "DB_URL",
    ];

    let upper = name.to_ascii_uppercase();
    if SENSITIVE_SUFFIXES
        .iter()
        .any(|suffix| upper.ends_with(suffix))
        || upper.contains("SERVICE_ACCOUNT")
    {
        return true;
    }

    // Single-segment catch-all: e.g. MYSECRET, MYTOKEN, SESSIONID — names
    // with no separator that still end in a sensitive segment.
    if SENSITIVE_SEGMENTS
        .iter()
        .any(|seg| upper.len() > seg.len() && upper.ends_with(seg))
    {
        return true;
    }

    upper
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            SENSITIVE_SEGMENTS
                .iter()
                .any(|sensitive| segment.eq_ignore_ascii_case(sensitive))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- is_sensitive_name ----

    #[test]
    fn sensitive_names_are_detected() {
        let sensitive = [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "GITHUB_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "MY_PASSWORD",
            "PASSWD",
            "DATABASE_URL",
            "POSTGRES_DSN",
            "STRIPE_SECRET",
            "SESSION_COOKIE",
            "AUTH_TOKEN",
            "CREDENTIALS",
            "SERVICE_ACCOUNT_JSON",
            "MYAPP_API_BASE",
            "FOO_BASE_URL",
            "API_KEY",
            "TOKEN",
            "SECRET",
            "PASSWORD",
            "DB_URL",
            "MYSECRET",
            "my_lowercase_token",
        ];
        for name in sensitive {
            assert!(
                is_sensitive_name(name),
                "expected {name:?} to be flagged sensitive"
            );
        }
    }

    #[test]
    fn non_sensitive_names_are_allowed() {
        let allowed = [
            "PATH", "HOME", "LANG", "LC_ALL", "TERM", "SHELL", "USER", "LOGNAME", "TMPDIR",
            "EDITOR", "PAGER", "DISPLAY", "PWD", "OLDPWD", "SHLVL", "ZC_DEBUG", "FOO_BAR",
            "NODE_ENV",
        ];
        for name in allowed {
            assert!(
                !is_sensitive_name(name),
                "expected {name:?} to be non-sensitive"
            );
        }
    }

    #[test]
    fn empty_and_short_names_are_non_sensitive() {
        assert!(!is_sensitive_name(""));
        assert!(!is_sensitive_name("K"));
        assert!(!is_sensitive_name("_"));
    }

    // ---- configure_environment ----

    #[tokio::test]
    async fn scrub_strips_secrets_keeps_benign_and_applies_explicit() {
        // Set a secret in the real parent env so we can prove it is not
        // inherited by the child.
        let secret_name = "ZC_SCRUB_TEST_TOKEN";
        std::env::set_var(secret_name, "super-secret-value");
        std::env::set_var("ZC_SCRUB_TEST_BENIGN", "keep-me");

        let mut cmd = tokio::process::Command::new("env");
        let mut explicit = HashMap::new();
        explicit.insert("ZC_SCRUB_TEST_EXPLICIT".to_string(), "set-me".to_string());
        explicit.insert(secret_name.to_string(), "explicit-wins".to_string());
        configure_environment(&mut cmd, Some(&explicit), &[]);

        let output = cmd.output().await.expect("spawn env failed");
        let stdout = String::from_utf8_lossy(&output.stdout);

        // The inherited secret must not leak through.
        assert!(
            !stdout.contains("super-secret-value"),
            "child inherited a secret: {stdout}"
        );
        // Benign inherited vars survive.
        assert!(stdout.contains("ZC_SCRUB_TEST_BENIGN=keep-me"));
        // Explicit values always win, even for sensitive names.
        assert!(stdout.contains("ZC_SCRUB_TEST_EXPLICIT=set-me"));
        assert!(stdout.contains("ZC_SCRUB_TEST_TOKEN=explicit-wins"));

        std::env::remove_var(secret_name);
        std::env::remove_var("ZC_SCRUB_TEST_BENIGN");
    }

    #[tokio::test]
    async fn passthrough_opts_back_into_secret_inheritance() {
        let secret_name = "ZC_SCRUB_TEST_PASSTHROUGH_TOKEN";
        std::env::set_var(secret_name, "super-secret-value");

        let mut cmd = tokio::process::Command::new("env");
        let passthrough = vec![secret_name.to_string()];
        configure_environment(&mut cmd, None, &passthrough);

        let output = cmd.output().await.expect("spawn env failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("ZC_SCRUB_TEST_PASSTHROUGH_TOKEN=super-secret-value"));

        std::env::remove_var(secret_name);
    }
}
