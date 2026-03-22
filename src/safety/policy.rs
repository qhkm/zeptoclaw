//! Content policy engine for ZeptoClaw.
//!
//! Evaluates input text against a set of compiled rules to detect security
//! threats, injection attempts, and sensitive data exposure. Each rule has an
//! associated severity and recommended action so callers can decide how to
//! respond to violations.
//!
//! The engine is designed to be constructed once and reused across many
//! invocations -- all regex patterns are compiled at construction time.

use regex::{Regex, RegexSet};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// How severe a policy violation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySeverity {
    /// Must be addressed immediately -- processing should stop.
    Critical,
    /// Serious concern that likely warrants blocking or sanitization.
    High,
    /// Notable but not necessarily blocking.
    Medium,
    /// Informational -- log and move on.
    Low,
}

/// What the caller should do about a violation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAction {
    /// Stop processing and return an error.
    Block,
    /// Remove or replace the offending content before continuing.
    Sanitize,
    /// Log a warning but allow processing to continue.
    Warn,
}

/// A single policy violation detected by the engine.
#[derive(Debug, Clone)]
pub struct PolicyViolation {
    /// Machine-readable name of the rule that matched.
    pub rule_name: String,
    /// Severity of the violation.
    pub severity: PolicySeverity,
    /// Recommended action for the caller.
    pub action: PolicyAction,
    /// Human-readable description of why this is a violation.
    pub description: String,
    /// The text fragment that matched, when available.
    pub matched_text: Option<String>,
}

// ---------------------------------------------------------------------------
// Internal rule definition
// ---------------------------------------------------------------------------

/// A compiled policy rule. Constructed once inside `PolicyEngine::new()`.
struct CompiledRule {
    name: &'static str,
    severity: PolicySeverity,
    action: PolicyAction,
    description: &'static str,
    /// Individual compiled regex used to extract the matched text.
    pattern: Regex,
}

// ---------------------------------------------------------------------------
// Rule definitions (pattern source strings)
// ---------------------------------------------------------------------------

/// `(name, severity, action, description, regex_pattern)`
///
/// All patterns are compiled with case-insensitive mode (`(?i)`) so that
/// trivial case-variation bypasses are ineffective.
const RULE_DEFS: &[(&str, PolicySeverity, PolicyAction, &str, &str)] = &[
    // 1. System file access
    (
        "system_file_access",
        PolicySeverity::Critical,
        PolicyAction::Block,
        "Attempt to access sensitive system files",
        r"(?i)(/etc/passwd|/etc/shadow|\.ssh/|\.aws/credentials|\.gnupg/|\.bashrc|\.profile|\.zshrc)",
    ),
    // 2. Crypto / private key paths
    (
        "crypto_key_patterns",
        PolicySeverity::High,
        PolicyAction::Block,
        "Reference to private key material",
        r"(?i)(id_rsa|id_ed25519|id_ecdsa|id_dsa|\.pem\b|private[_-]?key|-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY)",
    ),
    // 3. SQL injection
    (
        "sql_injection",
        PolicySeverity::High,
        PolicyAction::Sanitize,
        "Potential SQL injection payload",
        r"(?i)(DROP\s+TABLE|DELETE\s+FROM|UNION\s+SELECT|OR\s+1\s*=\s*1|';\s*--)",
    ),
    // 4. Shell injection
    (
        "shell_injection",
        PolicySeverity::Critical,
        PolicyAction::Block,
        "Potential shell injection payload",
        r"(?i)(;\s*rm\s+-rf|&&\s*rm\s|curl\s+.*\|\s*sh|wget\s+.*\|\s*sh|\$\(|`[^`]+`)",
    ),
    // 5. Encoded / indirect exploits
    (
        "encoded_exploits",
        PolicySeverity::Medium,
        PolicyAction::Warn,
        "Encoded or indirect code execution attempt",
        r"(?i)(base64_decode|eval\s*\(|exec\s*\(|__import__)",
    ),
    // 6. Path traversal
    (
        "path_traversal",
        PolicySeverity::High,
        PolicyAction::Sanitize,
        "Path traversal attempt",
        r"(\.\./|\.\.\\|%2[eE]%2[eE])",
    ),
    // 7. Sensitive environment variable references
    (
        "sensitive_env",
        PolicySeverity::Medium,
        PolicyAction::Warn,
        "Reference to sensitive environment variable",
        r"(?i)(DATABASE_URL|SECRET_KEY|PRIVATE_KEY)",
    ),
];

// ---------------------------------------------------------------------------
// PolicyEngine
// ---------------------------------------------------------------------------

/// A compiled content policy engine.
///
/// Construct once via [`PolicyEngine::new()`] and reuse. The `RegexSet` is
/// used for a fast first-pass check; individual regexes are only consulted
/// for rules that the set reports as matching, keeping the common (clean)
/// path fast.
pub struct PolicyEngine {
    /// Fast first-pass set -- indices correspond to `rules`.
    set: RegexSet,
    /// Individual compiled rules for match extraction.
    rules: Vec<CompiledRule>,
}

impl PolicyEngine {
    /// Create a new `PolicyEngine` with the default rule set.
    ///
    /// All regex patterns are compiled eagerly. If any pattern fails to
    /// compile (which would be a bug in the static definitions) it is
    /// silently skipped -- this mirrors the approach used by the existing
    /// `ShellSecurityConfig`.
    pub fn new() -> Self {
        let patterns: Vec<&str> = RULE_DEFS.iter().map(|(_, _, _, _, pat)| *pat).collect();

        let set = RegexSet::new(&patterns).expect("static policy patterns must compile");

        let rules: Vec<CompiledRule> = RULE_DEFS
            .iter()
            .filter_map(|(name, sev, act, desc, pat)| {
                Regex::new(pat).ok().map(|regex| CompiledRule {
                    name,
                    severity: sev.clone(),
                    action: act.clone(),
                    description: desc,
                    pattern: regex,
                })
            })
            .collect();

        Self { set, rules }
    }

    /// Check `input` against all policy rules.
    ///
    /// Returns a (possibly empty) list of violations. Multiple rules can
    /// match the same input.
    pub fn check(&self, input: &str) -> Vec<PolicyViolation> {
        self.check_with_ignored_rules(input, &[])
    }

    /// Check `input` against all policy rules except those explicitly ignored.
    pub fn check_with_ignored_rules(
        &self,
        input: &str,
        ignored_rules: &[&str],
    ) -> Vec<PolicyViolation> {
        // Fast path: if no patterns match, return immediately.
        let matches: Vec<usize> = self.set.matches(input).into_iter().collect();
        if matches.is_empty() {
            return Vec::new();
        }

        let mut violations = Vec::with_capacity(matches.len());

        for idx in matches {
            let rule = &self.rules[idx];
            if ignored_rules.contains(&rule.name) {
                continue;
            }
            let matched_text = rule.pattern.find(input).map(|m| m.as_str().to_string());

            violations.push(PolicyViolation {
                rule_name: rule.name.to_string(),
                severity: rule.severity.clone(),
                action: rule.action.clone(),
                description: rule.description.to_string(),
                matched_text,
            });
        }

        violations
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PolicyEngine {
        PolicyEngine::new()
    }

    // -- System file access ------------------------------------------------

    #[test]
    fn test_system_file_access_etc_passwd() {
        let v = engine().check("read /etc/passwd");
        assert!(!v.is_empty());
        let hit = v.iter().find(|v| v.rule_name == "system_file_access");
        assert!(hit.is_some(), "expected system_file_access violation");
        let hit = hit.unwrap();
        assert_eq!(hit.severity, PolicySeverity::Critical);
        assert_eq!(hit.action, PolicyAction::Block);
    }

    #[test]
    fn test_system_file_access_ssh() {
        let v = engine().check("copy .ssh/id_rsa to /tmp");
        let names: Vec<&str> = v.iter().map(|v| v.rule_name.as_str()).collect();
        assert!(
            names.contains(&"system_file_access"),
            "expected system_file_access, got {:?}",
            names
        );
    }

    #[test]
    fn test_system_file_access_aws() {
        let v = engine().check("cat .aws/credentials");
        assert!(v.iter().any(|v| v.rule_name == "system_file_access"));
    }

    #[test]
    fn test_check_with_ignored_rules_skips_named_rule_only() {
        let v = engine()
            .check_with_ignored_rules("echo $(whoami) > .aws/credentials", &["shell_injection"]);
        assert!(
            !v.iter().any(|v| v.rule_name == "shell_injection"),
            "shell_injection should be ignored"
        );
        assert!(
            v.iter().any(|v| v.rule_name == "system_file_access"),
            "other matching rules should still be reported"
        );
    }

    // -- Crypto key patterns -----------------------------------------------

    #[test]
    fn test_crypto_key_pem_reference() {
        let v = engine().check("load server.pem for TLS");
        assert!(v.iter().any(|v| v.rule_name == "crypto_key_patterns"));
    }

    #[test]
    fn test_crypto_key_begin_private() {
        let v = engine().check("-----BEGIN PRIVATE KEY-----");
        assert!(v.iter().any(|v| v.rule_name == "crypto_key_patterns"));
    }

    // -- SQL injection -----------------------------------------------------

    #[test]
    fn test_sql_injection_drop_table() {
        let v = engine().check("DROP TABLE users;");
        let hit = v.iter().find(|v| v.rule_name == "sql_injection").unwrap();
        assert_eq!(hit.severity, PolicySeverity::High);
        assert_eq!(hit.action, PolicyAction::Sanitize);
    }

    #[test]
    fn test_sql_injection_union_select() {
        let v = engine().check("1 UNION SELECT * FROM credentials");
        assert!(v.iter().any(|v| v.rule_name == "sql_injection"));
    }

    // -- Shell injection ---------------------------------------------------

    #[test]
    fn test_shell_injection_rm_rf() {
        let v = engine().check("do something; rm -rf /");
        let hit = v.iter().find(|v| v.rule_name == "shell_injection").unwrap();
        assert_eq!(hit.severity, PolicySeverity::Critical);
        assert_eq!(hit.action, PolicyAction::Block);
    }

    #[test]
    fn test_shell_injection_command_substitution() {
        let v = engine().check("result=$(cat /etc/shadow)");
        assert!(v.iter().any(|v| v.rule_name == "shell_injection"));
    }

    // -- Encoded exploits --------------------------------------------------

    #[test]
    fn test_encoded_exploits_eval() {
        let v = engine().check("eval('payload')");
        let hit = v
            .iter()
            .find(|v| v.rule_name == "encoded_exploits")
            .unwrap();
        assert_eq!(hit.severity, PolicySeverity::Medium);
        assert_eq!(hit.action, PolicyAction::Warn);
    }

    // -- Path traversal ----------------------------------------------------

    #[test]
    fn test_path_traversal_dotdot() {
        let v = engine().check("open ../../etc/hosts");
        assert!(v.iter().any(|v| v.rule_name == "path_traversal"));
    }

    #[test]
    fn test_path_traversal_encoded() {
        let v = engine().check("GET /files/%2e%2e/secret");
        assert!(v.iter().any(|v| v.rule_name == "path_traversal"));
    }

    // -- Sensitive env vars ------------------------------------------------

    #[test]
    fn test_sensitive_env_database_url() {
        let v = engine().check("export DATABASE_URL=postgres://...");
        assert!(v.iter().any(|v| v.rule_name == "sensitive_env"));
    }

    // -- Clean input -------------------------------------------------------

    #[test]
    fn test_clean_input_no_violations() {
        let v = engine().check("Hello, how are you today?");
        assert!(v.is_empty(), "expected no violations, got {:?}", v.len());
    }

    // -- Multiple violations -----------------------------------------------

    #[test]
    fn test_multiple_violations_in_one_input() {
        // This input triggers both system_file_access and shell_injection
        let v = engine().check("$(cat /etc/passwd)");
        assert!(
            v.len() >= 2,
            "expected at least 2 violations, got {}",
            v.len()
        );
    }

    // -- Case insensitivity ------------------------------------------------

    #[test]
    fn test_case_insensitive_sql() {
        let v = engine().check("drop table users");
        assert!(
            v.iter().any(|v| v.rule_name == "sql_injection"),
            "case-insensitive SQL check failed"
        );
    }
}
