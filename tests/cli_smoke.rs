//! CLI smoke tests — verify all commands that work without API keys.
//!
//! These tests run the compiled binary and verify exit codes and output.
//! No external API keys or network access required.

use std::process::Command;

/// Helper: run zeptoclaw with given args and return (exit_code, stdout, stderr).
fn run_cli(args: &[&str]) -> (i32, String, String) {
    let bin = env!("CARGO_BIN_EXE_zeptoclaw");
    let output = Command::new(bin)
        .args(args)
        .env("RUST_LOG", "") // suppress tracing noise
        .output()
        .expect("failed to execute zeptoclaw binary");
    let code = output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

// ============================================================================
// Help & Version
// ============================================================================

#[test]
fn cli_no_args_shows_help() {
    let (code, stdout, _stderr) = run_cli(&[]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("zeptoclaw"));
}

#[test]
fn cli_help_flag() {
    let (code, stdout, _stderr) = run_cli(&["--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Usage:"));
    assert!(stdout.contains("Commands:"));
}

#[test]
fn cli_version_command() {
    let (code, stdout, _stderr) = run_cli(&["version"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("zeptoclaw"));
    // Should contain a semver-like version string
    assert!(stdout.contains('.'));
}

// ============================================================================
// Config
// ============================================================================

#[test]
fn cli_config_check() {
    let (code, stdout, _stderr) = run_cli(&["config", "check"]);
    // Exits 0 if config is valid (or has only warnings), 1 if errors
    assert!(code == 0 || code == 1);
    // Should always mention the config file path
    assert!(
        stdout.contains("config") || stdout.contains("Config"),
        "Expected config-related output, got: {}",
        stdout
    );
}

#[test]
fn cli_config_check_help() {
    let (code, stdout, _stderr) = run_cli(&["config", "check", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Check"));
}

// ============================================================================
// Auth
// ============================================================================

#[test]
fn cli_auth_status() {
    let (code, stdout, _stderr) = run_cli(&["auth", "status"]);
    assert_eq!(code, 0);
    // Should mention provider status
    assert!(
        stdout.contains("provider")
            || stdout.contains("Provider")
            || stdout.contains("auth")
            || stdout.contains("Auth")
            || stdout.contains("status")
            || stdout.contains("Status"),
        "Expected auth status output, got: {}",
        stdout
    );
}

#[test]
fn cli_auth_login() {
    // Test with unsupported provider to get the error path (no browser/timeout)
    let (code, _stdout, stderr) = run_cli(&["auth", "login", "openai"]);
    // Should exit non-zero since OpenAI doesn't support OAuth
    assert_ne!(code, 0);
    assert!(
        stderr.contains("does not support OAuth")
            || stderr.contains("not support OAuth")
            || _stdout.contains("does not support OAuth"),
        "Expected OAuth unsupported error"
    );
}

#[test]
fn cli_auth_logout() {
    let (code, _stdout, _stderr) = run_cli(&["auth", "logout"]);
    assert_eq!(code, 0);
}

// ============================================================================
// Skills
// ============================================================================

#[test]
fn cli_skills_list() {
    let (code, _stdout, _stderr) = run_cli(&["skills", "list"]);
    assert_eq!(code, 0);
}

#[test]
fn cli_skills_list_all() {
    let (code, _stdout, _stderr) = run_cli(&["skills", "list", "--all"]);
    assert_eq!(code, 0);
}

#[test]
fn cli_skills_show_nonexistent() {
    let (code, _stdout, stderr) = run_cli(&["skills", "show", "nonexistent-skill-xyz"]);
    // Should fail gracefully (exit 1 or print error)
    assert!(
        code != 0 || stderr.contains("not found") || stderr.contains("Error"),
        "Expected error for nonexistent skill"
    );
}

// ============================================================================
// History
// ============================================================================

#[test]
fn cli_history_list() {
    let (code, _stdout, _stderr) = run_cli(&["history", "list"]);
    assert_eq!(code, 0);
}

#[test]
fn cli_history_list_with_limit() {
    let (code, _stdout, _stderr) = run_cli(&["history", "list", "--limit", "5"]);
    assert_eq!(code, 0);
}

#[test]
fn cli_history_show_nonexistent() {
    let (code, _stdout, _stderr) = run_cli(&["history", "show", "nonexistent-session-xyz"]);
    // Should fail gracefully — no crash
    assert!(code == 0 || code == 1);
}

// ============================================================================
// Templates
// ============================================================================

#[test]
fn cli_template_list() {
    let (code, stdout, _stderr) = run_cli(&["template", "list"]);
    assert_eq!(code, 0);
    // Should list at least the built-in templates
    assert!(
        stdout.contains("coder")
            || stdout.contains("researcher")
            || stdout.contains("template")
            || stdout.contains("Template")
            || stdout.is_empty(), // empty is OK if no templates configured
        "Expected template listing, got: {}",
        stdout
    );
}

#[test]
fn cli_template_show_nonexistent() {
    let (code, _stdout, _stderr) = run_cli(&["template", "show", "nonexistent-template-xyz"]);
    // Should fail gracefully
    assert!(code == 0 || code == 1);
}

// ============================================================================
// Channel
// ============================================================================

#[test]
fn cli_channel_list() {
    let (code, _stdout, _stderr) = run_cli(&["channel", "list"]);
    assert_eq!(code, 0);
}

// ============================================================================
// Status
// ============================================================================

#[test]
fn cli_status() {
    let (code, stdout, _stderr) = run_cli(&["status"]);
    assert_eq!(code, 0);
    // Status should include version info
    assert!(
        stdout.contains("zeptoclaw")
            || stdout.contains("ZeptoClaw")
            || stdout.contains("version")
            || stdout.contains("Version"),
        "Expected status output with version info, got: {}",
        stdout
    );
}

// ============================================================================
// Heartbeat
// ============================================================================

#[test]
fn cli_heartbeat_show() {
    let (code, _stdout, _stderr) = run_cli(&["heartbeat", "--show"]);
    // Should work (shows heartbeat.md content or "not found" message)
    assert_eq!(code, 0);
}

// ============================================================================
// Invalid commands & edge cases
// ============================================================================

#[test]
fn cli_invalid_command() {
    let (code, _stdout, stderr) = run_cli(&["nonexistent-command"]);
    assert_ne!(code, 0);
    assert!(
        stderr.contains("error") || stderr.contains("unrecognized"),
        "Expected error message for invalid command, got stderr: {}",
        stderr
    );
}

#[test]
fn cli_agent_help() {
    // `agent --help` should work without API keys
    let (code, stdout, _stderr) = run_cli(&["agent", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("agent") || stdout.contains("Agent"));
}

#[test]
fn cli_batch_help() {
    let (code, stdout, _stderr) = run_cli(&["batch", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("input") || stdout.contains("Input"));
}

#[test]
fn cli_gateway_help() {
    let (code, stdout, _stderr) = run_cli(&["gateway", "--help"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("gateway") || stdout.contains("Gateway"));
}
