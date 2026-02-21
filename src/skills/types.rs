//! Skills type definitions.

use serde::{Deserialize, Serialize};

/// Loaded skill model.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name.
    pub name: String,
    /// Short description.
    pub description: String,
    /// Absolute path to `SKILL.md`.
    pub path: String,
    /// Source type: `workspace` or `builtin`.
    pub source: String,
    /// Parsed frontmatter metadata.
    pub metadata: SkillMetadata,
    /// Markdown body content.
    pub content: String,
}

/// Skill listing entry.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    /// Skill name.
    pub name: String,
    /// Skill file path.
    pub path: String,
    /// Source type: `workspace` or `builtin`.
    pub source: String,
}

fn default_required() -> bool {
    true
}

/// A required/optional environment variable declared by a skill.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EnvSpec {
    /// Environment variable name (e.g. "WHATSAPP_PHONE_NUMBER_ID").
    pub name: String,
    /// Human-readable description shown in `skills show`.
    pub description: String,
    /// Whether this env var is required (default: true).
    #[serde(default = "default_required")]
    pub required: bool,
}

/// Parsed frontmatter metadata.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillMetadata {
    /// Skill name.
    pub name: String,
    /// Skill description.
    pub description: String,
    /// Optional version (semver).
    pub version: Option<String>,
    /// Optional homepage URL.
    pub homepage: Option<String>,
    /// Skill author name.
    pub author: Option<String>,
    /// Skill license identifier (e.g. "MIT").
    pub license: Option<String>,
    /// Categorization tags (e.g. ["messaging", "sea"]).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Skill names that must be present for this skill to be available.
    #[serde(default)]
    pub depends: Vec<String>,
    /// Skill names that conflict with this skill.
    #[serde(default)]
    pub conflicts: Vec<String>,
    /// Environment variables needed by this skill (informational + gating).
    #[serde(default)]
    pub env_needed: Vec<EnvSpec>,
    /// ZeptoClaw metadata payload.
    pub metadata: Option<serde_json::Value>,
}

/// ZeptoClaw metadata extension.
/// Compatible with both `metadata.zeptoclaw` and `metadata.openclaw` namespaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ZeptoMetadata {
    /// Optional emoji for UI and summaries.
    pub emoji: Option<String>,
    /// Runtime requirements.
    pub requires: SkillRequirements,
    /// Suggested install options.
    pub install: Vec<InstallOption>,
    /// Whether to always inject this skill into context.
    pub always: bool,
    /// Platform filter (e.g. `["darwin", "linux", "win32"]`). Empty means all platforms.
    pub os: Vec<String>,
}

/// Requirement model for a skill.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillRequirements {
    /// Required binaries in `PATH` (all must be present).
    pub bins: Vec<String>,
    /// At least one of these binaries must be in `PATH` (OpenClaw compat).
    #[serde(default, alias = "anyBins")]
    pub any_bins: Vec<String>,
    /// Required environment variables.
    pub env: Vec<String>,
}

/// Install option metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallOption {
    /// Option identifier.
    pub id: String,
    /// Install kind (`brew`, `apt`, `cargo`, ...).
    pub kind: String,
    /// Optional formula.
    pub formula: Option<String>,
    /// Optional package name.
    pub package: Option<String>,
    /// Binaries expected after install.
    #[serde(default)]
    pub bins: Vec<String>,
    /// User-facing label.
    pub label: String,
}
