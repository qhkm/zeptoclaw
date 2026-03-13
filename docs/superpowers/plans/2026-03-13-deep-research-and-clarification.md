# Deep Research Skill + Ask Clarification Tool Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a deep-research skill (SKILL.md) and an ask_clarification tool that pauses the agent loop for user input.

**Architecture:** Two independent deliverables. PR 1 is a markdown file only. PR 2 adds a new Rust tool, extends `ToolOutput` with a `pause_for_input` flag, modifies the agent loop's tool result tuple from `(String, String)` to `(String, String, bool)` in both non-streaming and streaming paths, adds `is_batch` to `ToolContext`, and updates the system prompt.

**Tech Stack:** Rust, async-trait, serde_json, tokio

**Spec:** `docs/superpowers/specs/2026-03-13-deep-research-skill-and-clarification-tool-design.md`

---

## Chunk 1: Deep Research Skill (PR 1)

### Task 1: Create deep-research SKILL.md

**Files:**
- Create: `skills/deep-research/SKILL.md`

- [ ] **Step 1: Create the skill file**

```markdown
---
name: deep-research
description: Systematic multi-phase web research producing thorough, cited reports
metadata: {"zeptoclaw":{"tags":["research","web","analysis"],"requires":{}}}
---

# Deep Research Skill

Use this skill when the user asks you to research a topic in depth, produce a report, or investigate something thoroughly. A single search query is NEVER enough for real research.

## Methodology

### Phase 1: Broad Exploration (3-5 searches)

Start wide. Use `web_search` with varied phrasings to map the territory:
- Search the main topic from different angles
- Identify subtopics, key players, and perspectives
- Note which areas have the most information vs gaps

Example: For "state of AI agents in 2026":
- `web_search("AI agent frameworks 2026")`
- `web_search("autonomous AI agents market landscape")`
- `web_search("AI agent orchestration tools comparison")`

### Phase 2: Deep Dive (2-3 searches + fetches per subtopic)

For each important subtopic from Phase 1:
- Use targeted, precise search queries
- Try multiple phrasings for the same concept
- Use `web_fetch` to read full articles (not just search snippets)
- Follow references and links mentioned in sources

Example:
- `web_search("LangGraph vs CrewAI vs AutoGen benchmark 2026")`
- `web_fetch("https://example.com/detailed-comparison-article")`

### Phase 3: Diversity & Validation

Ensure you have coverage across these 6 types of information:
1. **Facts & data** — statistics, market size, benchmarks, numbers
2. **Real-world examples** — case studies, actual implementations, user stories
3. **Expert opinions** — interviews, commentary, analysis from known figures
4. **Trends & predictions** — current year developments, future directions
5. **Comparisons** — alternatives, trade-offs, vs analyses
6. **Challenges & criticisms** — problems, limitations, balanced critique

Search specifically for any category you're missing.

### Phase 4: Synthesis Check

Before writing your final answer, verify:
- [ ] Did I cover 3-5 different angles?
- [ ] Did I read important sources in full (not just snippets)?
- [ ] Do I have concrete data AND real examples AND expert perspectives?
- [ ] Did I include both positives and challenges?
- [ ] Are my sources current and authoritative?

If any check fails, go back and search for what's missing.

## Temporal Awareness

Always use the actual current date in search queries for current events:
- Good: `"AI agent news March 2026"`
- Bad: `"AI agent news"` (may return outdated results)
- Try multiple date formats: `"March 2026"`, `"2026-03"`, `"2026"`

## Memory Integration

After completing research, save key findings using `longterm_memory`:
- Action: `set`
- Category: `research`
- Tags: relevant topic tags
- Save: main conclusions, key data points, important sources

This allows you to recall findings in future conversations without re-researching.

## Anti-Patterns (Do NOT Do These)

- **One-and-done**: Doing 1-2 searches and calling it "research"
- **Snippet reliance**: Using only search result snippets without reading full articles
- **One-sided**: Only searching for positives OR negatives, not both
- **Ignoring contradictions**: When sources disagree, investigate why — don't cherry-pick
- **Stale data**: Using old information without noting the date
- **Premature writing**: Starting to write the answer before research is complete
```

- [ ] **Step 2: Verify skill loads correctly**

Run: `ls skills/deep-research/SKILL.md`
Expected: File exists

- [ ] **Step 3: Commit**

```bash
git add skills/deep-research/SKILL.md
git commit -m "feat: add deep-research skill with 4-phase methodology

Teaches the agent systematic research using web_search and web_fetch:
- Phase 1: Broad exploration (3-5 searches)
- Phase 2: Deep dive (targeted + full content fetch)
- Phase 3: Diversity validation (6 information types)
- Phase 4: Synthesis check

Inspired by DeerFlow's research methodology, adapted for ZeptoClaw
with memory integration and temporal awareness.

Closes #N"
```

---

## Chunk 2: ToolOutput + ToolContext Changes (PR 2, foundation)

### Task 2: Add `pause_for_input` to ToolOutput

**Files:**
- Modify: `src/tools/types.rs`

- [ ] **Step 1: Write failing tests for pause_for_input**

Add these tests to the existing `mod tests` block in `src/tools/types.rs`:

```rust
#[test]
fn test_tool_output_default_pause_false() {
    let out = ToolOutput::llm_only("test");
    assert!(!out.pause_for_input);

    let out2 = ToolOutput::user_visible("test");
    assert!(!out2.pause_for_input);

    let out3 = ToolOutput::error("test");
    assert!(!out3.pause_for_input);

    let out4 = ToolOutput::split("a", "b");
    assert!(!out4.pause_for_input);

    let out5 = ToolOutput::async_task("test");
    assert!(!out5.pause_for_input);
}

#[test]
fn test_tool_output_with_pause() {
    let out = ToolOutput::llm_only("test").with_pause();
    assert!(out.pause_for_input);
    assert_eq!(out.for_llm, "test");
}

#[test]
fn test_tool_output_split_with_pause() {
    let out = ToolOutput::split("llm", "user").with_pause();
    assert!(out.pause_for_input);
    assert_eq!(out.for_user.as_deref(), Some("user"));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --lib test_tool_output_default_pause_false test_tool_output_with_pause test_tool_output_split_with_pause`
Expected: FAIL — `pause_for_input` field does not exist

- [ ] **Step 3: Add `pause_for_input` field and builder method**

In `src/tools/types.rs`, add the field to `ToolOutput`:

```rust
pub struct ToolOutput {
    pub for_llm: String,
    pub for_user: Option<String>,
    pub is_error: bool,
    pub is_async: bool,
    /// When true, the agent loop should break after this tool result
    /// and wait for the next user message before continuing.
    pub pause_for_input: bool,
}
```

Update ALL existing constructors to set `pause_for_input: false`:
- `llm_only()`
- `user_visible()`
- `error()`
- `async_task()`
- `split()`

Add builder method:

```rust
/// Mark this output as requiring a pause for user input.
///
/// When set, the agent loop will stop the tool-calling cycle after
/// this result and wait for the next user message.
pub fn with_pause(mut self) -> Self {
    self.pause_for_input = true;
    self
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo nextest run --lib test_tool_output_default_pause_false test_tool_output_with_pause test_tool_output_split_with_pause`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/tools/types.rs
git commit -m "feat: add pause_for_input flag to ToolOutput

Enables tools to signal the agent loop to stop and wait for user
input. All existing constructors default to false. Builder method
with_pause() sets the flag."
```

### Task 3: Add `is_batch` to ToolContext

**Files:**
- Modify: `src/tools/types.rs`
- Modify: `src/cli/batch.rs`

- [ ] **Step 1: Write failing test**

Add to `mod tests` in `src/tools/types.rs`:

```rust
#[test]
fn test_tool_context_is_batch_default() {
    let ctx = ToolContext::new();
    assert!(!ctx.is_batch);
}

#[test]
fn test_tool_context_with_batch() {
    let ctx = ToolContext::new().with_batch(true);
    assert!(ctx.is_batch);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo nextest run --lib test_tool_context_is_batch`
Expected: FAIL — `is_batch` field does not exist

- [ ] **Step 3: Add `is_batch` field to ToolContext**

In `src/tools/types.rs`, add field:

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub channel: Option<String>,
    pub chat_id: Option<String>,
    pub workspace: Option<String>,
    /// Whether the tool is running in batch mode (no interactive user).
    pub is_batch: bool,
}
```

Add builder method:

```rust
pub fn with_batch(mut self, is_batch: bool) -> Self {
    self.is_batch = is_batch;
    self
}
```

- [ ] **Step 4: Propagate `is_batch` through InboundMessage metadata**

`ToolContext` is NOT constructed in `src/batch.rs` — it's built inside the agent loop at `src/agent/loop.rs:1094` and `:1712`. Use `InboundMessage.metadata` to carry the flag.

In `src/cli/batch.rs`, where `InboundMessage` is created for each batch prompt, add:
```rust
inbound.metadata.insert("is_batch".into(), "true".into());
```

In `src/agent/loop.rs`, at line ~1094 (non-streaming) and ~1712 (streaming), where `ToolContext` is constructed, update the builder chain:
```rust
let tool_ctx = ToolContext::new()
    .with_channel(&msg.channel, &msg.chat_id)
    .with_workspace(&workspace_str)
    .with_batch(msg.metadata.get("is_batch").map_or(false, |v| v == "true"));
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo nextest run --lib test_tool_context_is_batch`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add src/tools/types.rs src/batch.rs
git commit -m "feat: add is_batch flag to ToolContext

Allows tools to detect batch mode and adjust behavior accordingly.
Set to true when running via batch command."
```

---

## Chunk 3: AskClarificationTool Implementation (PR 2, core tool)

### Task 4: Create clarification tool with tests

**Files:**
- Create: `src/tools/clarification.rs`
- Modify: `src/tools/mod.rs` (add `pub mod clarification;`)

- [ ] **Step 1: Create the tool file with tests first**

Create `src/tools/clarification.rs` with the full implementation + tests:

```rust
//! Ask clarification tool — pauses agent execution to ask the user a question.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::Result;
use crate::tools::{Tool, ToolCategory, ToolContext, ToolOutput};

/// Tool that pauses agent execution to ask the user for clarification.
pub struct AskClarificationTool;

#[async_trait]
impl Tool for AskClarificationTool {
    fn name(&self) -> &str {
        "ask_clarification"
    }

    fn description(&self) -> &str {
        "Ask the user for clarification before proceeding with an ambiguous or risky action"
    }

    fn compact_description(&self) -> &str {
        "Ask user for clarification"
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Memory
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user"
                },
                "clarification_type": {
                    "type": "string",
                    "enum": ["missing_info", "ambiguous_requirement", "approach_choice", "risk_confirmation", "suggestion"],
                    "description": "The type of clarification needed"
                },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Numbered options for the user to choose from"
                },
                "context": {
                    "type": "string",
                    "description": "Brief context for why clarification is needed"
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> Result<ToolOutput> {
        // Batch mode fallback — no interactive user
        if ctx.is_batch {
            return Ok(ToolOutput::llm_only(
                "Unable to clarify in batch mode. Proceeding with best judgment based on available information."
            ));
        }

        let question = args
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| crate::error::ZeptoError::Tool("Missing required field: question".into()))?;

        let context = args.get("context").and_then(|v| v.as_str());
        let options: Vec<&str> = args
            .get("options")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        // Build formatted user-facing message
        let mut user_msg = String::new();

        if let Some(ctx_text) = context {
            user_msg.push_str(ctx_text);
            user_msg.push_str("\n\n");
        }

        user_msg.push_str(question);

        if !options.is_empty() {
            user_msg.push('\n');
            for (i, opt) in options.iter().enumerate() {
                user_msg.push_str(&format!("\n{}. {}", i + 1, opt));
            }
        }

        Ok(ToolOutput::split(
            "Clarification requested. Waiting for user response before proceeding.",
            user_msg,
        )
        .with_pause())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ToolContext {
        ToolContext::new()
    }

    fn batch_ctx() -> ToolContext {
        ToolContext::new().with_batch(true)
    }

    #[test]
    fn test_name() {
        assert_eq!(AskClarificationTool.name(), "ask_clarification");
    }

    #[test]
    fn test_compact_description() {
        assert_eq!(AskClarificationTool.compact_description(), "Ask user for clarification");
    }

    #[test]
    fn test_category_memory() {
        assert_eq!(AskClarificationTool.category(), ToolCategory::Memory);
    }

    #[test]
    fn test_parameters_schema() {
        let params = AskClarificationTool.parameters();
        let props = params.get("properties").unwrap();
        assert!(props.get("question").is_some());
        assert!(props.get("clarification_type").is_some());
        assert!(props.get("options").is_some());
        assert!(props.get("context").is_some());

        let required = params.get("required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 1);
        assert_eq!(required[0], "question");
    }

    #[tokio::test]
    async fn test_execute_simple_question() {
        let args = json!({"question": "What format do you want?"});
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        assert_eq!(out.for_llm, "Clarification requested. Waiting for user response before proceeding.");
        assert_eq!(out.for_user.as_deref(), Some("What format do you want?"));
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_execute_with_context() {
        let args = json!({
            "question": "Which approach?",
            "context": "There are two ways to implement this."
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.starts_with("There are two ways to implement this."));
        assert!(user.contains("Which approach?"));
    }

    #[tokio::test]
    async fn test_execute_with_options() {
        let args = json!({
            "question": "Which database?",
            "options": ["PostgreSQL", "SQLite", "MongoDB"]
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.contains("1. PostgreSQL"));
        assert!(user.contains("2. SQLite"));
        assert!(user.contains("3. MongoDB"));
    }

    #[tokio::test]
    async fn test_execute_with_type() {
        let args = json!({
            "question": "Should I delete the file?",
            "clarification_type": "risk_confirmation"
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
        assert_eq!(out.for_user.as_deref(), Some("Should I delete the file?"));
    }

    #[tokio::test]
    async fn test_execute_full() {
        let args = json!({
            "question": "How should I proceed?",
            "clarification_type": "approach_choice",
            "context": "The module can be refactored two ways.",
            "options": ["Full rewrite", "Incremental patch"]
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();

        let user = out.for_user.unwrap();
        assert!(user.starts_with("The module can be refactored two ways."));
        assert!(user.contains("How should I proceed?"));
        assert!(user.contains("1. Full rewrite"));
        assert!(user.contains("2. Incremental patch"));
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_pause_flag_set() {
        let args = json!({"question": "test"});
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_missing_question() {
        let args = json!({"context": "some context"});
        let result = AskClarificationTool.execute(args, &ctx()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_invalid_type_graceful() {
        let args = json!({
            "question": "What?",
            "clarification_type": "nonexistent_type"
        });
        // Should not error — clarification_type is informational, not validated
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        assert!(out.pause_for_input);
    }

    #[tokio::test]
    async fn test_empty_options_array() {
        let args = json!({
            "question": "What?",
            "options": []
        });
        let out = AskClarificationTool.execute(args, &ctx()).await.unwrap();
        let user = out.for_user.unwrap();
        // Empty options should not produce numbered list
        assert!(!user.contains("1."));
        assert_eq!(user, "What?");
    }

    #[tokio::test]
    async fn test_batch_mode_fallback() {
        let args = json!({"question": "What format?"});
        let out = AskClarificationTool.execute(args, &batch_ctx()).await.unwrap();

        assert!(!out.pause_for_input);
        assert!(out.for_llm.contains("batch mode"));
        assert!(out.for_user.is_none());
    }
}
```

- [ ] **Step 2: Add module declaration and re-export**

In `src/tools/mod.rs`:
- Add `pub mod clarification;` in the module list (alphabetical order, after `composed`)
- Add `pub use clarification::AskClarificationTool;` in the re-export section (alphabetical order, after `composed` re-exports)

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo nextest run --lib clarification`
Expected: All 13 tests PASS

- [ ] **Step 4: Commit**

```bash
git add src/tools/clarification.rs src/tools/mod.rs
git commit -m "feat: add ask_clarification tool

Pauses agent execution to ask the user a question. Supports
optional clarification_type, numbered options, and context.
Returns ToolOutput with pause_for_input=true.
Falls back gracefully in batch mode."
```

---

## Chunk 4: Agent Loop + System Prompt + Registration (PR 2, wiring)

### Task 5: Wire pause_for_input into agent loop

**Files:**
- Modify: `src/agent/loop.rs` (two locations: non-streaming ~line 1338, streaming ~line 1922)

- [ ] **Step 1: Modify non-streaming tool result tuple**

In `src/agent/loop.rs`, find the non-streaming tool closure return at line ~1338:

Change `(id, sanitized)` to:

```rust
let pause = tool_output.as_ref().map_or(false, |o| o.pause_for_input);
(id, sanitized, pause)
```

Note: `tool_output` is the `Option<ToolOutput>` from the match block at line ~1252. Extract `pause` BEFORE the `for_user` handling consumes `tool_output` — specifically right after the `let (result, success, tool_output) = match ...` block at line ~1252, add:

```rust
let pause = tool_output.as_ref().map_or(false, |o| o.pause_for_input);
```

Then change the final return from `(id, sanitized)` to `(id, sanitized, pause)`.

- [ ] **Step 2: Update non-streaming result collection**

At line ~1361, change:

```rust
let results: Vec<(String, String)> = results;
for (id, result) in &results {
    session.add_message(Message::tool_result(id, result));
}
```

To:

```rust
let results: Vec<(String, String, bool)> = results;
let should_pause = results.iter().any(|(_, _, pause)| *pause);
for (id, result, _) in &results {
    session.add_message(Message::tool_result(id, result));
}
```

Then after the chain_tracker.record() call and before the tool_call_limit check, add:

```rust
if should_pause {
    break;
}
```

- [ ] **Step 3: Apply same changes to streaming path**

**IMPORTANT:** In the streaming path, `tool_output` is MOVED (not borrowed) at line ~1866:
```rust
if let Some(output) = tool_output {  // moves tool_output
```
Extract `pause` BEFORE this block:
```rust
let pause = tool_output.as_ref().map_or(false, |o| o.pause_for_input);
if let Some(output) = tool_output {
    // ... existing for_user handling
}
```

At line ~1922, change `(id, sanitized)` to `(id, sanitized, pause)`.

At line ~1944, change:

```rust
let results: Vec<(String, String)> = results;
for (id, result) in &results {
    session.add_message(Message::tool_result(id, result));
}
```

To:

```rust
let results: Vec<(String, String, bool)> = results;
let should_pause = results.iter().any(|(_, _, pause)| *pause);
for (id, result, _) in &results {
    session.add_message(Message::tool_result(id, result));
}
```

And add `if should_pause { break; }` before the tool_call_limit check.

- [ ] **Step 4: Update check_loop_guard_outcomes in BOTH paths**

The `check_loop_guard_outcomes` function (line ~117) expects `&[(String, String)]`. After changing results to 3-tuples, map them back for this call.

**Non-streaming path** (after `for (id, result, _) in &results` block, before `check_loop_guard_outcomes` call at ~line 1421):
```rust
let results_for_guard: Vec<(String, String)> = results.iter().map(|(id, r, _)| (id.clone(), r.clone())).collect();
```
Pass `&results_for_guard` to `check_loop_guard_outcomes`.

**Streaming path** (same change, at ~line 1975):
```rust
let results_for_guard: Vec<(String, String)> = results.iter().map(|(id, r, _)| (id.clone(), r.clone())).collect();
```
Pass `&results_for_guard` to `check_loop_guard_outcomes`.

- [ ] **Step 5: Run full test suite**

Run: `cargo nextest run --lib`
Expected: All tests PASS (no regressions)

- [ ] **Step 6: Commit**

```bash
git add src/agent/loop.rs
git commit -m "feat: wire pause_for_input into agent loop

Tool result tuples now carry a pause flag. When any tool sets
pause_for_input=true, the agent loop breaks after recording
results, allowing the next user message to resume naturally.
Applied to both non-streaming and streaming paths."
```

### Task 6: Update system prompt

**Files:**
- Modify: `src/agent/context.rs`

- [ ] **Step 1: Append clarification guidance to DEFAULT_SYSTEM_PROMPT**

Add before the closing `"#;` of `DEFAULT_SYSTEM_PROMPT` (after the heartbeat section):

```rust
\n\nYou have an ask_clarification tool. When facing ambiguity, use it instead of guessing:
- Missing information needed to proceed
- Multiple valid approaches to choose from
- Destructive or irreversible actions that need confirmation
- Ambiguous requirements that could be interpreted different ways
Do not over-use it for trivial decisions you can make yourself.
```

- [ ] **Step 2: Run tests to verify no regressions**

Run: `cargo nextest run --lib context`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add src/agent/context.rs
git commit -m "feat: add clarification guidance to system prompt

Instructs the agent when to use ask_clarification: ambiguous
requirements, destructive actions, multiple approaches.
Warns against over-use for trivial decisions."
```

### Task 7: Register tool in registrar

**Files:**
- Modify: `src/kernel/registrar.rs`

- [ ] **Step 1: Add registration**

Find the memory tools group (~line 437) or create a new group comment after it. Add:

```rust
// --- Group N: Interaction tools ---
if filter.is_enabled("ask_clarification") {
    registry.register(Box::new(crate::tools::clarification::AskClarificationTool));
}
```

- [ ] **Step 2: Run full test suite**

Run: `cargo nextest run --lib`
Expected: All tests PASS

- [ ] **Step 3: Commit**

```bash
git add src/kernel/registrar.rs
git commit -m "feat: register ask_clarification tool

Always available, no feature gate. Registered after memory tools
in the tool registrar."
```

---

## Chunk 5: Pre-push checks + documentation (PR 2, finalize)

### Task 8: Run pre-push checklist

- [ ] **Step 1: Format**

Run: `cargo fmt`

- [ ] **Step 2: Clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Full test suite**

Run: `cargo nextest run --lib`
Expected: All tests PASS

- [ ] **Step 4: Doc tests**

Run: `cargo test --doc`
Expected: PASS

- [ ] **Step 5: Verify clean format**

Run: `cargo fmt -- --check`
Expected: No diff

### Task 9: Update CLAUDE.md

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Add clarification tool to tools list**

In the `src/tools/` section of CLAUDE.md architecture, add:

```text
│   ├── clarification.rs # Ask clarification tool (pause for user input)
```

- [ ] **Step 2: Update core skills list**

Update the core skills line:

```text
**Core skills** (bundled in this repo — `skills/`): `github`, `skill-creator`, `deep-research`
```

- [ ] **Step 3: Update tool count**

Change "32 built-in" to "33 built-in" in the tools description.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md for ask_clarification tool and deep-research skill"
```
