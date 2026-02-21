//! Secret and credential leak detection.
//!
//! Scans text for common secret patterns (API keys, tokens, PEM keys, etc.)
//! and provides configurable actions: block, redact, or warn.
//!
//! # Example
//!
//! ```
//! use zeptoclaw::safety::leak_detector::{LeakDetector, LeakAction};
//!
//! let detector = LeakDetector::new();
//! let detections = detector.scan("my key is sk-abc12345678901234567890");
//! assert_eq!(detections.len(), 1);
//! assert_eq!(detections[0].action, LeakAction::Redact);
//!
//! let (redacted, _) = detector.redact("my key is sk-abc12345678901234567890");
//! assert!(redacted.contains("sk-a***7890"));
//! ```

use regex::Regex;

/// Action to take when a secret pattern is detected.
#[derive(Debug, Clone, PartialEq)]
pub enum LeakAction {
    /// Return error, don't pass content through.
    Block,
    /// Mask middle characters (keep first 4 + last 4).
    Redact,
    /// Log warning but pass content through unchanged.
    Warn,
}

/// A single detection result from scanning content.
#[derive(Debug, Clone)]
pub struct LeakDetection {
    /// Human-readable name of the pattern that matched.
    pub pattern_name: String,
    /// The literal text that was matched.
    pub matched_text: String,
    /// The action configured for this pattern type.
    pub action: LeakAction,
}

/// A compiled secret pattern with its metadata.
struct SecretPattern {
    name: &'static str,
    regex: Regex,
    action: LeakAction,
}

/// Scans text for leaked secrets and credentials.
///
/// All regex patterns are compiled once at construction time. The detector is
/// `Send + Sync` safe because `regex::Regex` is thread-safe.
pub struct LeakDetector {
    patterns: Vec<SecretPattern>,
}

impl LeakDetector {
    /// Create a new detector with all built-in secret patterns compiled.
    ///
    /// Patterns are ordered so that more specific patterns (e.g. `sk-ant-api`)
    /// are checked before broader ones (e.g. `sk-`), preventing the broader
    /// pattern from shadowing the specific one during redaction.
    #[must_use]
    pub fn new() -> Self {
        let pattern_defs: Vec<(&str, &str, LeakAction)> = vec![
            // --- API keys (Redact) ---
            // Anthropic must come before OpenAI since both start with `sk-`
            (
                "anthropic_api_key",
                r"sk-ant-api[a-zA-Z0-9\-]{20,}",
                LeakAction::Redact,
            ),
            ("openai_api_key", r"sk-[a-zA-Z0-9]{20,}", LeakAction::Redact),
            ("aws_access_key", r"AKIA[A-Z0-9]{16}", LeakAction::Redact),
            (
                "github_pat",
                r"github_pat_[a-zA-Z0-9_]{22,}",
                LeakAction::Redact,
            ),
            ("github_token", r"ghp_[a-zA-Z0-9]{36}", LeakAction::Redact),
            (
                "stripe_live_key",
                r"sk_live_[a-zA-Z0-9]{24,}",
                LeakAction::Redact,
            ),
            (
                "stripe_test_key",
                r"sk_test_[a-zA-Z0-9]{24,}",
                LeakAction::Redact,
            ),
            (
                "google_api_key",
                r"AIza[a-zA-Z0-9_\-]{35}",
                LeakAction::Redact,
            ),
            (
                "slack_bot_token",
                r"xoxb-[a-zA-Z0-9\-]+",
                LeakAction::Redact,
            ),
            (
                "slack_user_token",
                r"xoxp-[a-zA-Z0-9\-]+",
                LeakAction::Redact,
            ),
            (
                "bearer_token",
                r"Bearer [a-zA-Z0-9._\-]{20,}",
                LeakAction::Redact,
            ),
            // --- Block ---
            (
                "pem_private_key",
                r"-----BEGIN (RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----",
                LeakAction::Block,
            ),
            // --- Warn ---
            (
                "authorization_header",
                r"Authorization:\s*[a-zA-Z0-9._\-]{20,}",
                LeakAction::Warn,
            ),
            ("high_entropy_hex", r"[0-9a-fA-F]{64,}", LeakAction::Warn),
            (
                "generic_jwt",
                r"eyJ[a-zA-Z0-9_\-]{10,}\.[a-zA-Z0-9_\-]{10,}\.[a-zA-Z0-9_\-]{10,}",
                LeakAction::Warn,
            ),
        ];

        let patterns = pattern_defs
            .into_iter()
            .map(|(name, pattern, action)| SecretPattern {
                name,
                regex: Regex::new(pattern).unwrap_or_else(|e| {
                    panic!("BUG: invalid built-in regex pattern '{name}': {e}")
                }),
                action,
            })
            .collect();

        Self { patterns }
    }

    /// Scan `input` for all matching secret patterns.
    ///
    /// Returns a `Vec<LeakDetection>` with one entry per match. A single input
    /// can produce multiple detections if it contains different secret types.
    #[must_use]
    pub fn scan(&self, input: &str) -> Vec<LeakDetection> {
        let mut detections = Vec::new();
        for pattern in &self.patterns {
            for mat in pattern.regex.find_iter(input) {
                detections.push(LeakDetection {
                    pattern_name: pattern.name.to_string(),
                    matched_text: mat.as_str().to_string(),
                    action: pattern.action.clone(),
                });
            }
        }
        detections
    }

    /// Redact all detected secrets in `input` and return the sanitized string
    /// along with the list of detections.
    ///
    /// For each match with `LeakAction::Redact`, the middle characters are
    /// replaced with `***` while preserving the first 4 and last 4 characters.
    /// If the matched text is 8 characters or shorter, it is replaced entirely
    /// with `***`.
    ///
    /// Matches with `LeakAction::Block` or `LeakAction::Warn` are recorded in
    /// the detections list but the text is **not** modified (callers should
    /// inspect the action to decide how to handle those).
    #[must_use]
    pub fn redact(&self, input: &str) -> (String, Vec<LeakDetection>) {
        let mut result = input.to_string();
        let mut detections = Vec::new();

        for pattern in &self.patterns {
            // We must re-find matches on the evolving `result` string because
            // earlier redactions may shift byte offsets. Collect matches first
            // to avoid borrowing conflicts, then replace in reverse order so
            // byte offsets remain valid for earlier matches.
            let matches: Vec<(usize, usize, String)> = pattern
                .regex
                .find_iter(&result)
                .map(|m| (m.start(), m.end(), m.as_str().to_string()))
                .collect();

            for (start, end, matched) in matches.iter().rev() {
                detections.push(LeakDetection {
                    pattern_name: pattern.name.to_string(),
                    matched_text: matched.clone(),
                    action: pattern.action.clone(),
                });

                if pattern.action == LeakAction::Redact {
                    let redacted = redact_string(matched);
                    result.replace_range(start..end, &redacted);
                }
            }
        }

        (result, detections)
    }
}

impl Default for LeakDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Redact a secret string by keeping the first 4 and last 4 characters and
/// replacing everything in between with `***`.
///
/// If the string is 8 characters or shorter, the entire string is replaced
/// with `***` since there are not enough characters to preserve meaningful
/// prefix/suffix context.
fn redact_string(s: &str) -> String {
    // Work on characters to avoid slicing inside multibyte UTF-8 characters.
    let char_count = s.chars().count();
    if char_count <= 8 {
        return "***".to_string();
    }
    let prefix: String = s.chars().take(4).collect();
    let suffix_rev: String = s.chars().rev().take(4).collect();
    let suffix: String = suffix_rev.chars().rev().collect();
    format!("{prefix}***{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detector() -> LeakDetector {
        LeakDetector::new()
    }

    // ---------------------------------------------------------------
    // Detection tests — each secret type is recognized
    // ---------------------------------------------------------------

    #[test]
    fn test_detect_openai_key() {
        let d = detector();
        let input = "key: sk-abcdefghijklmnopqrstuvwxyz";
        let hits = d.scan(input);
        assert!(!hits.is_empty(), "should detect OpenAI key");
        assert_eq!(hits[0].pattern_name, "openai_api_key");
        assert_eq!(hits[0].action, LeakAction::Redact);
    }

    #[test]
    fn test_detect_anthropic_key() {
        let d = detector();
        let input = "key: sk-ant-api03-abcdefghijklmnopqrstuvwxyz";
        let hits = d.scan(input);
        // Anthropic pattern is more specific and should match first
        let anthropic_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "anthropic_api_key")
            .collect();
        assert!(
            !anthropic_hits.is_empty(),
            "should detect Anthropic key specifically"
        );
        assert_eq!(anthropic_hits[0].action, LeakAction::Redact);
    }

    #[test]
    fn test_detect_aws_access_key() {
        let d = detector();
        let input = "aws_key=AKIAIOSFODNN7EXAMPLE";
        let hits = d.scan(input);
        assert!(!hits.is_empty(), "should detect AWS access key");
        assert_eq!(hits[0].pattern_name, "aws_access_key");
        assert_eq!(hits[0].action, LeakAction::Redact);
    }

    #[test]
    fn test_detect_github_token() {
        let d = detector();
        let input = "token: ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij";
        let hits = d.scan(input);
        assert!(!hits.is_empty(), "should detect GitHub token");
        assert_eq!(hits[0].pattern_name, "github_token");
    }

    #[test]
    fn test_detect_github_pat() {
        let d = detector();
        let input = "pat: github_pat_aBcDeFgHiJkLmNoPqRsT1234";
        let hits = d.scan(input);
        let pat_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "github_pat")
            .collect();
        assert!(!pat_hits.is_empty(), "should detect GitHub PAT");
    }

    #[test]
    fn test_detect_stripe_keys() {
        let d = detector();
        let live = ["sk", "live", "abcdefghijklmnopqrstuvwx"].join("_");
        let test = ["sk", "test", "abcdefghijklmnopqrstuvwx"].join("_");

        let live_hits = d.scan(&live);
        assert!(!live_hits.is_empty(), "should detect Stripe live key");
        assert!(live_hits
            .iter()
            .any(|h| h.pattern_name == "stripe_live_key"));

        let test_hits = d.scan(&test);
        assert!(!test_hits.is_empty(), "should detect Stripe test key");
        assert!(test_hits
            .iter()
            .any(|h| h.pattern_name == "stripe_test_key"));
    }

    #[test]
    fn test_detect_google_api_key() {
        let d = detector();
        let input = "key=AIzaSyA1234567890abcdefghijklmnopqrstuv";
        let hits = d.scan(input);
        assert!(!hits.is_empty(), "should detect Google API key");
        assert_eq!(hits[0].pattern_name, "google_api_key");
    }

    #[test]
    fn test_detect_slack_tokens() {
        let d = detector();
        let bot = format!("token: {}-123456789012-abcdefghijklmn", "xoxb");
        let user = format!("token: {}-123456789012-abcdefghijklmn", "xoxp");

        let bot_hits = d.scan(&bot);
        assert!(!bot_hits.is_empty(), "should detect Slack bot token");
        assert!(bot_hits.iter().any(|h| h.pattern_name == "slack_bot_token"));

        let user_hits = d.scan(&user);
        assert!(!user_hits.is_empty(), "should detect Slack user token");
        assert!(user_hits
            .iter()
            .any(|h| h.pattern_name == "slack_user_token"));
    }

    #[test]
    fn test_detect_bearer_token() {
        let d = detector();
        let input = "header: Bearer eyJhbGciOiJIUzI1NiJ9.payload.sig_value";
        let hits = d.scan(input);
        let bearer_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "bearer_token")
            .collect();
        assert!(!bearer_hits.is_empty(), "should detect Bearer token");
        assert_eq!(bearer_hits[0].action, LeakAction::Redact);
    }

    #[test]
    fn test_detect_pem_private_key() {
        let d = detector();
        let input = "-----BEGIN RSA PRIVATE KEY-----\nMIIEpAIBAAKCAQ...";
        let hits = d.scan(input);
        assert!(!hits.is_empty(), "should detect PEM private key");
        assert_eq!(hits[0].pattern_name, "pem_private_key");
        assert_eq!(hits[0].action, LeakAction::Block);
    }

    #[test]
    fn test_detect_high_entropy_hex() {
        let d = detector();
        // 64 hex chars
        let input = "hash: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hits = d.scan(input);
        let hex_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "high_entropy_hex")
            .collect();
        assert!(!hex_hits.is_empty(), "should detect high-entropy hex");
        assert_eq!(hex_hits[0].action, LeakAction::Warn);
    }

    #[test]
    fn test_detect_jwt() {
        let d = detector();
        let input = "token: eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U";
        let hits = d.scan(input);
        let jwt_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "generic_jwt")
            .collect();
        assert!(!jwt_hits.is_empty(), "should detect JWT");
        assert_eq!(jwt_hits[0].action, LeakAction::Warn);
    }

    // ---------------------------------------------------------------
    // Redaction tests
    // ---------------------------------------------------------------

    #[test]
    fn test_redaction_masks_correctly() {
        let d = detector();
        let key = "sk-abcdefghijklmnopqrstuvwxyz";
        let (redacted, detections) = d.redact(&format!("my key is {key}"));

        assert!(
            !detections.is_empty(),
            "should produce detections during redact"
        );
        // First 4 chars of match: "sk-a", last 4: "wxyz"
        assert!(
            redacted.contains("sk-a***wxyz"),
            "redacted output should keep first 4 and last 4 chars, got: {redacted}"
        );
        assert!(
            !redacted.contains(key),
            "original key must not appear in redacted output"
        );
    }

    #[test]
    fn test_redaction_short_string_replaced_entirely() {
        // Test the redact_string helper directly for short inputs
        assert_eq!(redact_string("abcdefgh"), "***"); // exactly 8 chars
        assert_eq!(redact_string("short"), "***"); // < 8 chars
        assert_eq!(redact_string("123456789"), "1234***6789"); // 9 chars, enough to preserve
    }

    #[test]
    fn test_block_action_for_pem_keys() {
        let d = detector();
        let input = "-----BEGIN PRIVATE KEY-----\nbase64data...\n-----END PRIVATE KEY-----";
        let hits = d.scan(input);
        assert!(!hits.is_empty());
        assert_eq!(hits[0].action, LeakAction::Block);

        // PEM keys should NOT be redacted by the redact method (Block action
        // leaves text untouched — caller must check the action and reject).
        let (output, _) = d.redact(input);
        assert!(
            output.contains("-----BEGIN PRIVATE KEY-----"),
            "Block action should not modify text; caller handles rejection"
        );
    }

    #[test]
    fn test_warn_action_for_hex() {
        let d = detector();
        let hex = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let input = format!("hash: {hex}");
        let (output, detections) = d.redact(&input);

        let hex_detections: Vec<_> = detections
            .iter()
            .filter(|d| d.pattern_name == "high_entropy_hex")
            .collect();
        assert!(!hex_detections.is_empty());
        assert_eq!(hex_detections[0].action, LeakAction::Warn);

        // Warn action should not modify the text
        assert!(
            output.contains(hex),
            "Warn action should not redact the text"
        );
    }

    #[test]
    fn test_clean_content_no_detections() {
        let d = detector();
        let input = "This is perfectly normal text with no secrets at all.";
        let hits = d.scan(input);
        assert!(
            hits.is_empty(),
            "clean content should produce no detections"
        );

        let (output, detections) = d.redact(input);
        assert_eq!(output, input, "clean content should pass through unchanged");
        assert!(
            detections.is_empty(),
            "clean content should produce no detections during redact"
        );
    }

    #[test]
    fn test_multiple_secrets_in_one_input() {
        let d = detector();
        let stripe_key = ["sk", "live", "abcdefghijklmnopqrstuvwx"].join("_");
        let input = format!(
            "openai: sk-abcdefghijklmnopqrstuvwxyz aws: AKIAIOSFODNN7EXAMPLE stripe: {}",
            stripe_key
        );
        let hits = d.scan(&input);

        // Should detect at least 3 distinct pattern types
        let pattern_names: Vec<_> = hits.iter().map(|h| h.pattern_name.as_str()).collect();
        assert!(
            pattern_names.contains(&"openai_api_key"),
            "should detect OpenAI key in multi-secret input"
        );
        assert!(
            pattern_names.contains(&"aws_access_key"),
            "should detect AWS key in multi-secret input"
        );
        assert!(
            pattern_names.contains(&"stripe_live_key")
                || pattern_names.contains(&"stripe_test_key"),
            "should detect Stripe key in multi-secret input"
        );

        // Redaction should mask all redactable secrets
        let (redacted, _) = d.redact(&input);
        assert!(
            !redacted.contains("sk-abcdefghijklmnopqrstuvwxyz"),
            "OpenAI key should be redacted"
        );
        assert!(
            !redacted.contains("AKIAIOSFODNN7EXAMPLE"),
            "AWS key should be redacted"
        );
    }

    #[test]
    fn test_detect_authorization_header() {
        let d = detector();
        let input = "Authorization: abc123def456ghi789jkl012mno";
        let hits = d.scan(input);
        let auth_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.pattern_name == "authorization_header")
            .collect();
        assert!(!auth_hits.is_empty(), "should detect Authorization header");
        assert_eq!(auth_hits[0].action, LeakAction::Warn);
    }

    #[test]
    fn test_detect_pem_variants() {
        let d = detector();
        let variants = [
            "-----BEGIN PRIVATE KEY-----",
            "-----BEGIN RSA PRIVATE KEY-----",
            "-----BEGIN EC PRIVATE KEY-----",
            "-----BEGIN DSA PRIVATE KEY-----",
            "-----BEGIN OPENSSH PRIVATE KEY-----",
        ];
        for variant in &variants {
            let hits = d.scan(variant);
            assert!(
                hits.iter().any(|h| h.pattern_name == "pem_private_key"),
                "should detect PEM variant: {variant}"
            );
        }
    }
}
