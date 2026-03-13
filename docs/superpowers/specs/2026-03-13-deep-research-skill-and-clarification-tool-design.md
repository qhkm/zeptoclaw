# Deep Research Skill + Ask Clarification Tool

**Date:** 2026-03-13
**Status:** Reviewed
**Scope:** Two independent features shipped as separate PRs

## Context

Analysis of DeerFlow revealed two features worth adopting that align with ZeptoClaw's goals (minimal core, great UX, zero binary bloat for skills):

1. **Deep research skill** — A SKILL.md teaching the agent systematic research methodology. Zero binary cost, dramatically better research output.
2. **Ask clarification tool** — A lightweight Rust tool that pauses execution to ask the user a question instead of guessing. Small binary impact, big UX improvement.

## PR 1: Deep Research Skill

### Deliverable

New file: `skills/deep-research/SKILL.md`

### Frontmatter

```yaml
name: deep-research
description: Systematic multi-phase web research producing thorough, cited reports
metadata:
  zeptoclaw:
    tags: [research, web, analysis]
    requires: {}
```

### Methodology (4 phases)

**Phase 1: Broad Exploration**
- 3-5 initial `web_search` calls with varied phrasings
- Map the territory: identify subtopics, perspectives, key players
- Goal: understand the landscape before going deep

**Phase 2: Deep Dive**
- For each subtopic identified in Phase 1:
  - Targeted searches with precise keywords
  - Multiple phrasings per subtopic
  - `web_fetch` on best sources to read full content (not just snippets)
  - Follow references found in sources
- A single search query is NEVER enough

**Phase 3: Diversity & Validation**
- Ensure coverage across 6 information types:
  - Facts & data (statistics, market size, benchmarks)
  - Real-world examples (case studies, implementations)
  - Expert opinions (interviews, commentary, analysis)
  - Trends & predictions (current year and forward)
  - Comparisons (alternatives, trade-offs)
  - Challenges & criticisms (balanced view)
- Search specifically for any missing category

**Phase 4: Synthesis Check**
Before writing the final answer, verify:
- [ ] 3-5 different angles covered?
- [ ] Important sources read in full via `web_fetch`?
- [ ] Concrete data + examples + expert perspectives?
- [ ] Both positives and challenges included?
- [ ] Sources are current and authoritative?

### ZeptoClaw-Specific Additions

**Temporal awareness:**
- Use actual current date in search queries for current events
- "tech news March 2026" not just "tech news"
- Try multiple date formats: numeric, written, relative

**Memory integration:**
- Save key findings to `longterm_memory` for future recall
- Use category "research" with relevant tags

**Anti-patterns to avoid:**
- Stopping after 1-2 searches
- Relying on search snippets without fetching full content
- Single-aspect research (only positives, only negatives)
- Ignoring contradictions between sources
- Using outdated information without noting the date
- Starting to write the answer before research is complete

### Wiring

No code changes. `SkillsLoader` auto-discovers `skills/deep-research/SKILL.md` from the builtin skills directory.

**Limitation:** For installed binaries (`cargo install`, Homebrew), the `skills/` directory is not bundled alongside the binary. Users of installed binaries would need to install the skill via `zeptoclaw skills install`. This is consistent with how community skills work. A future follow-up could embed bundled skills via `include_str!` at compile time.

### Tests

None required (no Rust code).

---

## PR 2: Ask Clarification Tool

### Deliverable

New Rust tool + ToolOutput enhancement + agent loop change + system prompt update + tests.

### New File: `src/tools/clarification.rs`

**Tool struct:** `AskClarificationTool`

**Tool trait implementation:**
- `name()` → `"ask_clarification"`
- `description()` → "Ask the user for clarification before proceeding with an ambiguous or risky action"
- `compact_description()` → "Ask user for clarification"
- `category()` → `ToolCategory::Memory` (read-only interaction tool, no destructive side effects; allows use in all agent modes including Observer)
- `parameters()`:
  ```json
  {
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
  }
  ```

**`execute()` logic:**
1. Parse `question`, `clarification_type`, `options`, `context` from args
2. Format `for_user` message:
   - If `context` provided: `"{context}\n\n{question}"`
   - If `options` provided: append numbered list `"\n1. {opt1}\n2. {opt2}\n..."`
   - Otherwise: just the question
3. Return `ToolOutput` with:
   - `for_llm`: `"Clarification requested. Waiting for user response before proceeding."`
   - `for_user`: formatted question
   - `pause_for_input`: `true`

### ToolOutput Change

**File:** `src/tools/mod.rs` (or `src/tools/types.rs`, wherever `ToolOutput` is defined)

Add field to the **existing** `ToolOutput` struct (which already has `is_error`, `is_async`, etc.):
```rust
// Add this field to the existing struct — do NOT replace the struct.
pub pause_for_input: bool,  // NEW — default false
```

All existing constructors (`new`, `llm_only`, `error`, `async_task`, `split`) must initialize `pause_for_input: false`.

Add builder method:
```rust
impl ToolOutput {
    pub fn with_pause(mut self) -> Self {
        self.pause_for_input = true;
        self
    }
}
```

Ensure `Default` and existing constructors (`llm_only`, `new`, etc.) set `pause_for_input: false`.

### Agent Loop Change

**File:** `src/agent/loop.rs`

**Problem:** The agent loop reduces tool results to `(String, String)` tuples (tool_call_id, sanitized_result) inside the per-tool async closure. The `ToolOutput` object is consumed to extract `for_user` and send it via `MessageBus`, then only the `for_llm` string survives. We cannot check `pause_for_input` on the collected results as-is.

**Solution:** Change the tool future return type from `(String, String)` to `(String, String, bool)` where the third element is `pause_for_input`. This must be applied to **both** code paths:

1. **Non-streaming path** (~lines 1108-1395): The per-tool closure currently returns `(id, sanitized_result)`. Change to `(id, sanitized_result, pause_for_input)`.

2. **Streaming path** (~lines 1688-1952): Same change in the streaming tool execution closure.

In both paths, after results are collected:

```rust
// Destructure the new tuple
let should_pause = tool_results.iter().any(|(_, _, pause)| *pause);

// Add tool result messages to session as usual (using id and result)
for (id, result, _) in &tool_results {
    // ... existing message addition logic
}

if should_pause {
    // Break the tool-calling loop — don't call LLM again
    // The for_user message was already sent via MessageBus
    // Next user message will naturally resume the conversation
    break;
}
```

Inside each per-tool closure, extract the flag before consuming `ToolOutput`:
```rust
let pause = tool_output.pause_for_input;
// ... existing for_user handling, sanitization ...
(tool_call_id, sanitized_result, pause)
```

### System Prompt Addition

**File:** `src/agent/context.rs`

Append to `DEFAULT_SYSTEM_PROMPT`:

```
When facing ambiguity, use the ask_clarification tool instead of guessing:
- Missing information needed to proceed
- Multiple valid approaches to choose from
- Destructive or irreversible actions that need confirmation
- Ambiguous requirements that could be interpreted different ways
Do not over-use it for trivial decisions you can make yourself.
```

### Batch Mode Handling

**Prerequisite:** Add `pub is_batch: bool` to `ToolContext` in `src/tools/types.rs` (default `false`). Set to `true` in the batch command handler (`src/batch.rs`) when constructing the `ToolContext`.

In `execute()`, check `ctx.is_batch`. If true:
- Return `ToolOutput::llm_only("Unable to clarify in batch mode. Proceeding with best judgment based on available information.")`
- Do NOT set `pause_for_input`

### Registration

**File:** `src/tools/mod.rs` — Export `AskClarificationTool`
**File:** `src/kernel/registrar.rs` — Register in `register_all_tools()`

No feature gate. Always available.

### Behavior by Mode

| Mode | Behavior |
|------|----------|
| CLI interactive | Question printed to terminal. User types answer. Agent continues. |
| Gateway (Telegram/Discord/etc) | Question sent as normal message via MessageBus. User replies. Agent continues. |
| Batch mode | Returns fallback message. No pause. Agent proceeds with best guess. |

### Tests (~17)

**Unit tests in `src/tools/clarification.rs`:**
1. `test_name` — returns `"ask_clarification"`
2. `test_parameters_schema` — validates JSON schema structure
3. `test_execute_simple_question` — question only, correct for_user/for_llm
4. `test_execute_with_type` — clarification_type included in output
5. `test_execute_with_options` — numbered options formatted correctly
6. `test_execute_with_context` — context prepended to question
7. `test_execute_full` — all fields provided
8. `test_pause_flag_set` — `pause_for_input == true`
9. `test_missing_question` — error on missing required field
10. `test_invalid_type` — handles unknown clarification_type gracefully

**ToolOutput tests:**
11. `test_default_pause_false` — existing constructors don't set pause
12. `test_with_pause_builder` — builder sets flag correctly

**Agent loop tests (if feasible inline):**
13. `test_pause_breaks_loop` — tool result with pause_for_input stops iteration

**Integration-level:**
14. `test_batch_mode_fallback` — returns fallback message, no pause
15. `test_category_memory` — returns `ToolCategory::Memory`
16. `test_empty_options_array` — `options: []` behaves like no options
17. `test_compact_description` — returns expected short description

### Documentation Updates

**CLAUDE.md:**
- Add `clarification.rs` to tools list with description: `# Clarification — ask_clarification tool (pause for user input)`
- Add `deep-research` to core skills list: `Core skills (bundled): github, skill-creator, deep-research`
- Update tool count (32 → 33 built-in)
- Update test counts after implementation

---

## Decision Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Two PRs vs one | Two separate | Independent features, ship skill immediately |
| Clarification in gateway | Same as normal message | Simple, no special state, LLM handles multi-turn naturally |
| Implementation approach | Tool with pause flag | Reliable pause (~20 lines), reusable for future tools |
| System prompt guidance | Light rules | Prevents over-use and under-use |
| Skill methodology | DeerFlow + ZeptoClaw additions | Proven methodology, enhanced with memory integration |
