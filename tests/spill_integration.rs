//! End-to-end: oversized shell output survives as a readable file.

use serde_json::json;
use zeptoclaw::config::SpillConfig;
use zeptoclaw::tools::SpillSettings;
use zeptoclaw::tools::{Tool, ToolContext};

#[tokio::test]
async fn oversized_output_is_retrievable_by_read_file_and_grep() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        workspace: Some(dir.path().display().to_string()),
        spill: Some(SpillSettings {
            session_id: "it-session".to_string(),
            root: dir.path().join("spill"),
            config: SpillConfig {
                max_inline_lines: 10,
                ..Default::default()
            },
        }),
        ..Default::default()
    };

    let out = zeptoclaw::tools::shell::ShellTool::new()
        .execute(json!({ "command": "seq 1 3000" }), &ctx)
        .await
        .unwrap();

    let path = out
        .for_llm
        .split("saved to ")
        .nth(1)
        .and_then(|s| s.lines().next())
        .expect("locator present")
        .trim()
        .to_string();

    // read_file reaches a line the old truncation destroyed
    let slice = zeptoclaw::tools::filesystem::ReadFileTool
        .execute(json!({ "path": path, "offset": 2500, "limit": 1 }), &ctx)
        .await
        .unwrap();
    assert_eq!(slice.for_llm.trim(), "2500");

    // grep finds content past the old limit
    let hit = zeptoclaw::tools::grep::GrepTool
        .execute(json!({ "pattern": "^2999$", "path": path }), &ctx)
        .await
        .unwrap();
    assert!(hit.for_llm.contains("2999"));
}

#[tokio::test]
async fn spill_disabled_matches_previous_behavior() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = ToolContext {
        workspace: Some(dir.path().display().to_string()),
        ..Default::default()
    };

    let out = zeptoclaw::tools::shell::ShellTool::new()
        .execute(json!({ "command": "seq 1 3000" }), &ctx)
        .await
        .unwrap();

    assert!(out.for_llm.contains("[output truncated"));
    assert!(!out.for_llm.contains("saved to"));
}
