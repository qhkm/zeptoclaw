use std::path::PathBuf;

use serde_json::json;
use zeptoclaw::config::{Config, SpillConfig};
use zeptoclaw::spill::SpillStore;
use zeptoclaw::tools::SpillSettings;
use zeptoclaw::tools::{Tool, ToolContext};

/// Remove a session's spill directory and the session directory above it.
fn cleanup(session_key: &str) {
    let root = SpillStore::session_root(session_key);
    let _ = std::fs::remove_dir_all(&root);
    if let Some(session_dir) = root.parent() {
        let _ = std::fs::remove_dir(session_dir);
    }
}

/// Build the tool context the agent loop builds (see `src/agent/loop.rs`):
/// workspace from config, spill root from [`SpillStore::session_root`].
///
/// These are genuinely different directories in production —
/// `~/.zeptoclaw/workspace` versus `~/.zeptoclaw/sessions/<key>/spill` — and
/// reproducing that is the whole point. An earlier version of this file set
/// `workspace = tempdir` and `root = tempdir/spill`, putting spill *inside*
/// the workspace, which production never produces. That is why the only
/// coverage of the retrieval claim passed while both retrieval tools in fact
/// rejected the path with a `SecurityViolation`.
fn production_shaped_ctx(session_key: &str, spill: SpillConfig) -> (ToolContext, PathBuf) {
    let workspace = Config::default().workspace_path();
    std::fs::create_dir_all(&workspace).expect("default workspace must be creatable");

    let root = SpillStore::session_root(session_key);
    assert!(
        !root.starts_with(&workspace),
        "spill root {} must be outside the workspace {}",
        root.display(),
        workspace.display()
    );

    let ctx = ToolContext {
        workspace: Some(workspace.display().to_string()),
        spill: Some(SpillSettings {
            session_id: session_key.to_string(),
            root: root.clone(),
            config: spill,
        }),
        ..Default::default()
    };
    (ctx, root)
}

#[tokio::test]
async fn oversized_output_is_retrievable_by_read_file_and_grep() {
    let session_key = format!("cli:spill-it-{}", std::process::id());
    cleanup(&session_key);
    let (ctx, root) = production_shaped_ctx(
        &session_key,
        SpillConfig {
            max_inline_lines: 10,
            ..Default::default()
        },
    );

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
    assert!(
        PathBuf::from(&path).starts_with(&root),
        "locator {path} should live under {}",
        root.display()
    );

    let slice = zeptoclaw::tools::filesystem::ReadFileTool
        .execute(json!({ "path": path, "offset": 2500, "limit": 1 }), &ctx)
        .await
        .unwrap();
    assert_eq!(slice.for_llm.trim(), "2500");

    let hit = zeptoclaw::tools::grep::GrepTool
        .execute(json!({ "pattern": "^2999$", "path": path }), &ctx)
        .await
        .unwrap();
    assert!(hit.for_llm.contains("2999"));

    cleanup(&session_key);
}

/// Another session's spill directory stays out of reach, even though this
/// session is allowed to read its own.
#[tokio::test]
async fn another_sessions_spill_directory_is_still_rejected() {
    let mine = format!("cli:spill-mine-{}", std::process::id());
    let theirs = format!("cli:spill-theirs-{}", std::process::id());
    cleanup(&mine);
    cleanup(&theirs);

    let (ctx, _root) = production_shaped_ctx(&mine, SpillConfig::default());

    let their_root = SpillStore::session_root(&theirs);
    std::fs::create_dir_all(&their_root).unwrap();
    let their_file = their_root.join("001-shell.txt");
    std::fs::write(&their_file, "someone else's output\n").unwrap();

    let err = zeptoclaw::tools::filesystem::ReadFileTool
        .execute(json!({ "path": their_file.display().to_string() }), &ctx)
        .await
        .expect_err("cross-session spill reads must stay rejected");
    assert!(
        err.to_string().contains("escapes workspace"),
        "expected a security violation, got: {err}"
    );

    cleanup(&mine);
    cleanup(&theirs);
}

#[tokio::test]
async fn spill_absent_matches_previous_behavior() {
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
