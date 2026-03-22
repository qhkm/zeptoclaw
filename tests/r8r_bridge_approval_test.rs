//! Tests for approval response parsing and channel message formatting.

use zeptoclaw::r8r_bridge::approval::{
    format_approval_message, format_help_message, format_timeout_message, parse_approval_response,
};

// ---------------------------------------------------------------------------
// parse_approval_response — approve keywords
// ---------------------------------------------------------------------------

#[test]
fn test_approve_keywords() {
    for keyword in &["approve", "approved", "yes", "y", "lgtm", "ok"] {
        let result = parse_approval_response(keyword)
            .unwrap_or_else(|| panic!("expected Some for keyword {:?}", keyword));
        assert_eq!(
            result.decision, "approved",
            "keyword {:?} should map to 'approved'",
            keyword
        );
        assert_eq!(
            result.reason, "",
            "no trailing text means empty reason for keyword {:?}",
            keyword
        );
    }
}

// ---------------------------------------------------------------------------
// parse_approval_response — reject keywords
// ---------------------------------------------------------------------------

#[test]
fn test_reject_keywords() {
    for keyword in &["reject", "rejected", "no", "n", "deny", "denied"] {
        let result = parse_approval_response(keyword)
            .unwrap_or_else(|| panic!("expected Some for keyword {:?}", keyword));
        assert_eq!(
            result.decision, "rejected",
            "keyword {:?} should map to 'rejected'",
            keyword
        );
        assert_eq!(
            result.reason, "",
            "no trailing text means empty reason for keyword {:?}",
            keyword
        );
    }
}

// ---------------------------------------------------------------------------
// parse_approval_response — reason extraction
// ---------------------------------------------------------------------------

#[test]
fn test_reject_with_reason() {
    let result = parse_approval_response("reject bad build").unwrap();
    assert_eq!(result.decision, "rejected");
    assert_eq!(result.reason, "bad build");
}

#[test]
fn test_approve_with_reason() {
    let result = parse_approval_response("approve looks good").unwrap();
    assert_eq!(result.decision, "approved");
    assert_eq!(result.reason, "looks good");
}

// ---------------------------------------------------------------------------
// parse_approval_response — case insensitivity
// ---------------------------------------------------------------------------

#[test]
fn test_case_insensitive() {
    let result = parse_approval_response("APPROVE").unwrap();
    assert_eq!(result.decision, "approved");
    assert_eq!(result.reason, "");

    let result = parse_approval_response("LGTM please merge").unwrap();
    assert_eq!(result.decision, "approved");
    assert_eq!(result.reason, "please merge");

    let result = parse_approval_response("REJECT too risky").unwrap();
    assert_eq!(result.decision, "rejected");
    assert_eq!(result.reason, "too risky");
}

// ---------------------------------------------------------------------------
// parse_approval_response — unrecognized input returns None
// ---------------------------------------------------------------------------

#[test]
fn test_unrecognized_returns_none() {
    assert!(
        parse_approval_response("deploy staging").is_none(),
        "unrecognized first word should return None"
    );
    assert!(
        parse_approval_response("hello").is_none(),
        "unrecognized word should return None"
    );
    assert!(
        parse_approval_response("").is_none(),
        "empty string should return None"
    );
}

// ---------------------------------------------------------------------------
// format_approval_message
// ---------------------------------------------------------------------------

#[test]
fn test_format_approval_message() {
    let msg = format_approval_message(
        "deploy-prod",
        "exec_1234567890abcdef",
        "Deploy to production?",
    );

    assert!(msg.contains("[r8r]"), "should contain [r8r] prefix");
    assert!(msg.contains("deploy-prod"), "should contain workflow name");
    assert!(
        msg.contains("exec_123"),
        "should contain start of execution id"
    );
    // short_id is first 8 chars of execution_id
    assert!(msg.contains("exec_123"), "short_id should be first 8 chars");
    assert!(
        msg.contains("Deploy to production?"),
        "should contain the message body"
    );
    assert!(
        msg.contains("approve"),
        "should contain approve instruction"
    );
    assert!(msg.contains("reject"), "should contain reject instruction");
}

#[test]
fn test_format_approval_message_short_id_length() {
    let execution_id = "abcdefgh12345678";
    let msg = format_approval_message("wf", execution_id, "ok?");
    // The short_id should be exactly the first 8 characters
    assert!(
        msg.contains("abcdefgh"),
        "short_id should be first 8 chars of execution_id"
    );
    assert!(
        !msg.contains("abcdefgh1"),
        "short_id must not be longer than 8 chars"
    );
}

// ---------------------------------------------------------------------------
// format_timeout_message
// ---------------------------------------------------------------------------

#[test]
fn test_format_timeout_message() {
    // seconds only
    let msg = format_timeout_message("deploy-prod", "exec_1234567890abcdef", 45);
    assert!(msg.contains("[r8r]"), "should contain [r8r] prefix");
    assert!(msg.contains("deploy-prod"), "should contain workflow name");
    assert!(
        msg.contains("expired") || msg.contains("Expired"),
        "should mention expiry"
    );
    assert!(msg.contains("45s"), "45 seconds should format as '45s'");

    // minutes
    let msg = format_timeout_message("wf", "exec_abc", 90);
    assert!(msg.contains("1m"), "90 seconds should format as '1m'");

    // hours + minutes
    let msg = format_timeout_message("wf", "exec_abc", 3661);
    assert!(
        msg.contains("1h"),
        "3661 seconds should mention hours component"
    );
    assert!(
        msg.contains("1m"),
        "3661 seconds should mention minutes component"
    );

    // exact hour (0 minutes omitted or shown)
    let msg_hour = format_timeout_message("wf", "exec_abc", 3600);
    assert!(
        msg_hour.contains("1h"),
        "3600 seconds should format as '1h ...'"
    );
}

// ---------------------------------------------------------------------------
// format_help_message
// ---------------------------------------------------------------------------

#[test]
fn test_format_help_message() {
    let msg = format_help_message();
    assert!(
        msg.contains("approve"),
        "help message should mention approve"
    );
    assert!(msg.contains("reject"), "help message should mention reject");
}
