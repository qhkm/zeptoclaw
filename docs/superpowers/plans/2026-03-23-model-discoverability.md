# Model Discoverability & Provider Auto-Selection — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make it easy for users to discover, select, and switch AI models — from onboarding through daily use.

**Architecture:** Update hardcoded model catalog, add live model fetching from provider APIs, enhance onboarding with model selection step, improve `/model list` with usage hints, add `/model fetch` for live discovery, and warn on model-provider mismatches at startup.

**Tech Stack:** Rust, reqwest (already a dependency), serde_json for response parsing.

**Spec:** `docs/superpowers/specs/2026-03-23-model-discoverability-design.md`

---

## Prerequisites

**`provider_name_for_model()` must already exist** in `src/providers/registry.rs` and be exported from `src/providers/mod.rs`. This function was written earlier in this session (matches model strings against provider `model_keywords`). If not yet committed, commit it first before starting this plan. The function signature is:

```rust
pub fn provider_name_for_model(model: &str) -> Option<&'static str>
```

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `src/channels/model_switch.rs` | Modify | Update `KNOWN_MODELS`, add `Fetch` to `ModelCommand`, update `parse_model_command()`, add hints to `format_model_list()` |
| `src/config/types.rs` | Modify | Update `COMPILE_TIME_DEFAULT_MODEL` |
| `src/cli/common.rs` | Modify | Add `fetch_provider_models()`, add startup mismatch warning helper |
| `src/cli/onboard.rs` | Modify | Add `configure_model()` step |
| `src/cli/agent.rs` | Modify | Handle `ModelCommand::Fetch`, add startup warning call |
| `src/cli/slash.rs` | Modify | Add `model fetch` entry |
| `src/channels/telegram.rs` | Modify | Add `ModelCommand::Fetch` arm (no-op with hint) |

---

### Task 1: Update `KNOWN_MODELS` and compile-time default

These must ship together so the default model always appears in `/model list`.

**Files:**
- Modify: `src/channels/model_switch.rs:27-126` — replace `KNOWN_MODELS` entries
- Modify: `src/config/types.rs:771-774` — update `COMPILE_TIME_DEFAULT_MODEL`

- [ ] **Step 1: Write test for updated KNOWN_MODELS**

In `src/channels/model_switch.rs`, add at the bottom of the existing `#[cfg(test)]` block:

```rust
#[test]
fn test_known_models_no_empty_fields() {
    for km in KNOWN_MODELS {
        assert!(!km.provider.is_empty(), "provider is empty for {:?}", km.model);
        assert!(!km.model.is_empty(), "model is empty for {:?}", km.provider);
        assert!(!km.label.is_empty(), "label is empty for {:?}", km.model);
    }
}

#[test]
fn test_known_models_no_duplicates() {
    let mut seen = std::collections::HashSet::new();
    for km in KNOWN_MODELS {
        let key = format!("{}:{}", km.provider, km.model);
        assert!(seen.insert(key.clone()), "duplicate model entry: {}", key);
    }
}

#[test]
fn test_known_models_includes_default_model() {
    let default_model = crate::config::AgentDefaults::default().model;
    assert!(
        KNOWN_MODELS.iter().any(|km| km.model == default_model),
        "Default model '{}' must appear in KNOWN_MODELS",
        default_model
    );
}
```

- [ ] **Step 2: Run tests — expect `test_known_models_includes_default_model` to fail**

Run: `cargo nextest run --lib -E 'test(known_models)'`
Expected: `test_known_models_includes_default_model` FAILS (current default `claude-sonnet-4-5-20250929` won't match after update)

- [ ] **Step 3: Update `COMPILE_TIME_DEFAULT_MODEL`**

In `src/config/types.rs`, change:
```rust
    None => "claude-sonnet-4-5-20250929",
```
to:
```rust
    None => "claude-sonnet-4-6",
```

- [ ] **Step 4: Replace `KNOWN_MODELS` with current models**

In `src/channels/model_switch.rs`, replace the entire `KNOWN_MODELS` array (lines 27–126) with:

```rust
pub const KNOWN_MODELS: &[KnownModel] = &[
    // Anthropic
    KnownModel {
        provider: "anthropic",
        model: "claude-opus-4-6",
        label: "Claude Opus 4.6",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-sonnet-4-6",
        label: "Claude Sonnet 4.6",
    },
    KnownModel {
        provider: "anthropic",
        model: "claude-haiku-4-5-20251001",
        label: "Claude Haiku 4.5",
    },
    // OpenAI
    KnownModel {
        provider: "openai",
        model: "gpt-5.4",
        label: "GPT-5.4",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.4-mini",
        label: "GPT-5.4 Mini",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.4-nano",
        label: "GPT-5.4 Nano",
    },
    KnownModel {
        provider: "openai",
        model: "gpt-5.3-codex",
        label: "GPT-5.3 Codex",
    },
    // OpenRouter
    KnownModel {
        provider: "openrouter",
        model: "anthropic/claude-sonnet-4-6",
        label: "Claude Sonnet 4.6 (OR)",
    },
    KnownModel {
        provider: "openrouter",
        model: "google/gemini-3.1-pro",
        label: "Gemini 3.1 Pro (OR)",
    },
    // Groq
    KnownModel {
        provider: "groq",
        model: "llama-4-scout-17b-16e-instruct",
        label: "Llama 4 Scout",
    },
    KnownModel {
        provider: "groq",
        model: "llama-4-maverick-17b-128e-instruct",
        label: "Llama 4 Maverick",
    },
    // Gemini
    KnownModel {
        provider: "gemini",
        model: "gemini-3.1-pro",
        label: "Gemini 3.1 Pro",
    },
    KnownModel {
        provider: "gemini",
        model: "gemini-3.1-flash-lite",
        label: "Gemini 3.1 Flash Lite",
    },
    // Ollama (local)
    KnownModel {
        provider: "ollama",
        model: "llama3.3",
        label: "Llama 3.3 (local)",
    },
    KnownModel {
        provider: "ollama",
        model: "mistral",
        label: "Mistral (local)",
    },
    // DeepSeek
    KnownModel {
        provider: "deepseek",
        model: "deepseek-chat",
        label: "DeepSeek V3",
    },
    KnownModel {
        provider: "deepseek",
        model: "deepseek-reasoner",
        label: "DeepSeek R1",
    },
    // Kimi (Moonshot AI)
    KnownModel {
        provider: "kimi",
        model: "moonshot-v1-128k",
        label: "Kimi 128K",
    },
    KnownModel {
        provider: "kimi",
        model: "moonshot-v1-32k",
        label: "Kimi 32K",
    },
];
```

- [ ] **Step 5: Fix pre-existing tests that reference removed models**

In `src/channels/model_switch.rs`, find `test_format_model_list_shows_current` (~line 462). Update the `ModelOverride` model field from `"claude-sonnet-4-5-20250929"` to `"claude-opus-4-6"` (a model that exists in the updated `KNOWN_MODELS`). Similarly update `test_format_model_list_does_not_duplicate_known_model` (~line 486) to use `"claude-opus-4-6"` instead of `"claude-sonnet-4-5-20250929"`.

- [ ] **Step 6: Run all tests to verify**

Run: `cargo nextest run --lib -E 'test(known_models) | test(model)'`
Expected: ALL PASS

- [ ] **Step 7: Commit**

```bash
git add src/channels/model_switch.rs src/config/types.rs
git commit -m "feat(config): update KNOWN_MODELS to current models and default to claude-sonnet-4-6"
```

---

### Task 2: Add usage hints to `/model list`

**Files:**
- Modify: `src/channels/model_switch.rs` — `format_model_list()` function (~line 205)

- [ ] **Step 1: Write test for usage hints**

Add to test module in `src/channels/model_switch.rs`:

```rust
#[test]
fn test_format_model_list_includes_usage_hints() {
    let configured = vec!["anthropic".to_string()];
    let output = format_model_list(&configured, None, &[]);
    assert!(output.contains("Switch model:"), "should include switch hint");
    assert!(output.contains("/model reset"), "should include reset hint");
    assert!(output.contains("agents.defaults.model"), "should include config hint");
}
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `cargo nextest run --lib -E 'test(usage_hints)'`
Expected: FAIL — current output has no hints

- [ ] **Step 3: Add hints to `format_model_list()`**

In `src/channels/model_switch.rs`, in `format_model_list()`, just before the final `output.trim_end().to_string()` line (around line 280), add:

```rust
    output.push_str("\n\n");
    output.push_str("Switch model:  /model <model-id>\n");
    output.push_str("With provider: /model <provider>:<model-id>\n");
    output.push_str("Reset:         /model reset\n");
    output.push_str("Config:        agents.defaults.model in ~/.zeptoclaw/config.json");
```

- [ ] **Step 4: Run test — expect PASS**

Run: `cargo nextest run --lib -E 'test(usage_hints) | test(format_model_list)'`
Expected: ALL PASS

- [ ] **Step 5: Commit**

```bash
git add src/channels/model_switch.rs
git commit -m "feat(cli): add usage hints to /model list output"
```

---

### Task 3: Add `ModelCommand::Fetch` variant and `/model fetch` parsing

**Files:**
- Modify: `src/channels/model_switch.rs` — `ModelCommand` enum, `parse_model_command()`
- Modify: `src/cli/slash.rs` — `builtin_commands()`

- [ ] **Step 1: Write test for `/model fetch` parsing**

Add to test module in `src/channels/model_switch.rs`:

```rust
#[test]
fn test_parse_model_command_fetch() {
    let cmd = parse_model_command("/model fetch");
    assert_eq!(cmd, Some(ModelCommand::Fetch));
}
```

- [ ] **Step 2: Run test — expect compile error**

Run: `cargo nextest run --lib -E 'test(parse_model_command_fetch)'`
Expected: FAIL — `ModelCommand::Fetch` doesn't exist

- [ ] **Step 3: Add `Fetch` variant to `ModelCommand`**

In `src/channels/model_switch.rs`, add to the `ModelCommand` enum:

```rust
pub enum ModelCommand {
    /// `/model` — show current model
    Show,
    /// `/model <provider:model>` — set model
    Set(ModelOverride),
    /// `/model reset` — clear override
    Reset,
    /// `/model list` — show available models
    List,
    /// `/model fetch` — fetch live models from provider APIs
    Fetch,
}
```

- [ ] **Step 4: Add `"fetch"` arm to `parse_model_command()`**

In `parse_model_command()`, update the match block:

```rust
    match rest {
        "reset" => Some(ModelCommand::Reset),
        "list" => Some(ModelCommand::List),
        "fetch" => Some(ModelCommand::Fetch),
        arg => {
```

- [ ] **Step 5: Add `Fetch` arm to all `match mcmd` dispatch sites**

In `src/cli/agent.rs` (~line 416), add inside the `match mcmd` block after the `ModelCommand::Reset` arm:

```rust
                                ModelCommand::Fetch => {
                                    println!("Fetching models from configured providers...");
                                    // TODO(model-fetch): wire fetch_provider_models() in Task 5
                                    println!("(not yet implemented — use /model list for now)");
                                }
```

In `src/channels/telegram.rs`, add inside its `match cmd` block after `ModelCommand::List`:

```rust
                                        ModelCommand::Fetch => {
                                            let req = bot.send_message(
                                                teloxide::types::ChatId(chat_id_num),
                                                "Use /model list to see available models.\n/model fetch is only available in CLI mode.",
                                            );
                                            let _ = apply_thread_id(req, &thread_id).await;
                                        }
```

- [ ] **Step 6: Add `model fetch` to slash commands**

In `src/cli/slash.rs`, add after the `"model list"` entry in `builtin_commands()`:

```rust
        SlashCommand {
            name: "model fetch",
            description: "Fetch live models from providers",
        },
```

- [ ] **Step 7: Run tests**

Run: `cargo nextest run --lib -E 'test(parse_model_command) | test(completer)'`
Expected: ALL PASS

- [ ] **Step 8: Commit**

```bash
git add src/channels/model_switch.rs src/cli/agent.rs src/channels/telegram.rs src/cli/slash.rs
git commit -m "feat(cli): add /model fetch command variant"
```

---

### Task 4: Add `fetch_provider_models()` with response parsing

**Files:**
- Modify: `src/cli/common.rs` — add `fetch_provider_models()` function and tests

- [ ] **Step 1: Write tests for response parsing**

Add to the `#[cfg(test)]` module in `src/cli/common.rs`:

```rust
#[test]
fn test_parse_openai_models_response() {
    let json = serde_json::json!({
        "data": [
            {"id": "gpt-5.4", "object": "model"},
            {"id": "gpt-5.4-mini", "object": "model"},
            {"id": "gpt-5.4-nano", "object": "model"},
        ]
    });
    let models = parse_models_openai_format(&json);
    assert_eq!(models, vec!["gpt-5.4", "gpt-5.4-mini", "gpt-5.4-nano"]);
}

#[test]
fn test_parse_openai_models_empty_data() {
    let json = serde_json::json!({"data": []});
    let models = parse_models_openai_format(&json);
    assert!(models.is_empty());
}

#[test]
fn test_parse_openai_models_missing_data() {
    let json = serde_json::json!({"error": "unauthorized"});
    let models = parse_models_openai_format(&json);
    assert!(models.is_empty());
}

#[test]
fn test_parse_ollama_models_response() {
    let json = serde_json::json!({
        "models": [
            {"name": "llama3.3:latest", "size": 123456},
            {"name": "mistral:latest", "size": 654321},
        ]
    });
    let models = parse_models_ollama_format(&json);
    assert_eq!(models, vec!["llama3.3:latest", "mistral:latest"]);
}

#[test]
fn test_parse_ollama_models_empty() {
    let json = serde_json::json!({"models": []});
    let models = parse_models_ollama_format(&json);
    assert!(models.is_empty());
}
```

- [ ] **Step 2: Run tests — expect compile error**

Run: `cargo nextest run --lib -E 'test(parse_openai_models) | test(parse_ollama_models)'`
Expected: FAIL — functions don't exist

- [ ] **Step 3: Implement response parsers**

Add to `src/cli/common.rs` (above the test module):

```rust
/// Parse model IDs from an OpenAI-format `/models` response.
/// Works for: Anthropic, OpenAI, OpenRouter, Groq, Gemini (via compat shim), DeepSeek, Kimi.
pub(crate) fn parse_models_openai_format(json: &serde_json::Value) -> Vec<String> {
    json.get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// Parse model names from an Ollama `/api/tags` response.
pub(crate) fn parse_models_ollama_format(json: &serde_json::Value) -> Vec<String> {
    json.get("models")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}
```

- [ ] **Step 4: Run parser tests — expect PASS**

Run: `cargo nextest run --lib -E 'test(parse_openai_models) | test(parse_ollama_models)'`
Expected: ALL PASS

- [ ] **Step 5: Implement `fetch_provider_models()`**

Add to `src/cli/common.rs`:

```rust
/// Fetch available models from a provider's API.
///
/// Uses the standard `/models` endpoint for most providers (OpenAI format).
/// Ollama uses `/api/tags` with a different response format.
/// Returns sorted model IDs on success; Err on network/parse failure.
pub(crate) async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
    api_base: Option<&str>,
) -> anyhow::Result<Vec<String>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;

    let (url, is_ollama) = match provider {
        "anthropic" => {
            let base = api_base.unwrap_or("https://api.anthropic.com");
            (format!("{}/v1/models", base), false)
        }
        "openai" => {
            let base = api_base.unwrap_or("https://api.openai.com/v1");
            (format!("{}/models", base), false)
        }
        "ollama" => {
            let base = api_base.unwrap_or("http://localhost:11434/v1");
            let base_stripped = base
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .trim_end_matches('/');
            (format!("{}/api/tags", base_stripped), true)
        }
        _ => {
            // All other OpenAI-compatible providers (openrouter, groq, gemini, deepseek, kimi, etc.)
            let base = api_base.unwrap_or("https://api.openai.com/v1");
            (format!("{}/models", base), false)
        }
    };

    let mut req = client.get(&url);
    if provider == "anthropic" {
        req = req
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01");
    } else if !api_key.is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key));
    }

    let resp = req.send().await?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "Failed to fetch models from {} (HTTP {})",
            provider,
            resp.status().as_u16()
        );
    }

    let body: serde_json::Value = resp.json().await?;
    let mut models = if is_ollama {
        parse_models_ollama_format(&body)
    } else {
        parse_models_openai_format(&body)
    };
    models.sort();
    Ok(models)
}
```

- [ ] **Step 6: Run full build**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 7: Commit**

```bash
git add src/cli/common.rs
git commit -m "feat(providers): add fetch_provider_models() for live model discovery"
```

---

### Task 5: Wire `/model fetch` to `fetch_provider_models()`

**Files:**
- Modify: `src/cli/agent.rs` — replace TODO in `ModelCommand::Fetch` arm

- [ ] **Step 1: Replace the TODO placeholder in agent.rs**

In `src/cli/agent.rs`, replace the `ModelCommand::Fetch` arm (the TODO placeholder from Task 3) with:

```rust
                                ModelCommand::Fetch => {
                                    println!("Fetching models from configured providers...\n");
                                    let selections =
                                        zeptoclaw::providers::resolve_runtime_providers(&config);
                                    if selections.is_empty() {
                                        println!("No providers configured. Run 'zeptoclaw onboard' to set up.");
                                    } else {
                                        for s in &selections {
                                            let api_base = s.api_base.as_deref();
                                            match super::common::fetch_provider_models(
                                                s.name, &s.api_key, api_base,
                                            )
                                            .await
                                            {
                                                Ok(models) => {
                                                    println!(
                                                        "{} ({} models):",
                                                        s.name,
                                                        models.len()
                                                    );
                                                    for m in &models {
                                                        println!("  {}", m);
                                                    }
                                                    println!();
                                                }
                                                Err(e) => {
                                                    println!("{}: failed to fetch ({})", s.name, e);
                                                    println!();
                                                }
                                            }
                                        }
                                        println!("Switch: /model <model-id>");
                                    }
                                }
```

- [ ] **Step 2: Build to verify compilation**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 3: Commit**

```bash
git add src/cli/agent.rs
git commit -m "feat(cli): wire /model fetch to live provider model listing"
```

---

### Task 6: Add startup model-provider mismatch warning

**Files:**
- Modify: `src/cli/common.rs` — add `warn_model_provider_mismatch()` helper
- Modify: `src/cli/agent.rs` — call warning after agent setup

- [ ] **Step 1: Write test for warning logic**

Add to `src/cli/common.rs` test module:

```rust
#[test]
fn test_model_mismatch_warning_returns_none_when_matched() {
    assert!(model_provider_mismatch_warning("gpt-5.4", &["openai", "anthropic"]).is_none());
    assert!(model_provider_mismatch_warning("claude-sonnet-4-6", &["anthropic"]).is_none());
}

#[test]
fn test_model_mismatch_warning_returns_message_when_unmatched() {
    let msg = model_provider_mismatch_warning("some-unknown-model", &["anthropic", "openai"]);
    assert!(msg.is_some());
    let msg = msg.unwrap();
    assert!(msg.contains("some-unknown-model"));
    assert!(msg.contains("anthropic"));
    assert!(msg.contains("/model list"));
}

#[test]
fn test_model_mismatch_warning_returns_none_when_no_providers() {
    assert!(model_provider_mismatch_warning("gpt-5.4", &[]).is_none());
}
```

- [ ] **Step 2: Run tests — expect compile error**

Run: `cargo nextest run --lib -E 'test(model_mismatch)'`
Expected: FAIL — function doesn't exist

- [ ] **Step 3: Implement warning helper**

Add to `src/cli/common.rs`:

```rust
/// Returns a warning message if the configured model doesn't match any known provider.
/// Returns `None` if the model matches a provider or no providers are configured.
pub(crate) fn model_provider_mismatch_warning(
    model: &str,
    configured_providers: &[&str],
) -> Option<String> {
    if configured_providers.is_empty() {
        return None;
    }
    if zeptoclaw::providers::provider_name_for_model(model).is_some() {
        return None;
    }
    Some(format!(
        "Model \"{}\" doesn't match any known provider.\n  \
         Configured providers: {}\n  \
         Run /model list to see available models.",
        model,
        configured_providers.join(", "),
    ))
}
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run --lib -E 'test(model_mismatch)'`
Expected: ALL PASS

- [ ] **Step 5: Call warning from agent setup**

In `src/cli/agent.rs`, find the section after `setup_agent()` returns (where the agent is ready to start the interactive loop). Add:

```rust
    // Warn if configured model doesn't match any known provider.
    {
        let provider_names = zeptoclaw::providers::configured_provider_names(&config);
        let provider_refs: Vec<&str> = provider_names.iter().map(|s| *s).collect();
        if let Some(warning) = super::common::model_provider_mismatch_warning(
            &config.agents.defaults.model,
            &provider_refs,
        ) {
            tracing::warn!("{}", warning);
        }
    }
```

- [ ] **Step 6: Build to verify compilation**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 7: Commit**

```bash
git add src/cli/common.rs src/cli/agent.rs
git commit -m "feat(cli): warn at startup when model doesn't match any configured provider"
```

---

### Task 7: Add model selection step to onboarding

**Files:**
- Modify: `src/cli/onboard.rs` — add `configure_model()` function, call from both paths

- [ ] **Step 1: Write test for model menu formatting**

Add to `src/cli/onboard.rs` test module (or create one if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_model_menu_with_known_models() {
        let models = vec![
            "gpt-5.4".to_string(),
            "gpt-5.4-mini".to_string(),
            "gpt-5.4-nano".to_string(),
        ];
        let menu = format_model_menu(&models, 10);
        assert!(menu.contains("1."));
        assert!(menu.contains("gpt-5.4"));
        assert!(menu.contains("c."));
        assert!(menu.contains("s."));
    }

    #[test]
    fn test_format_model_menu_truncates_at_max() {
        let models: Vec<String> = (0..20).map(|i| format!("model-{}", i)).collect();
        let menu = format_model_menu(&models, 5);
        // Should show 5 numbered entries + custom + skip
        assert!(menu.contains("5."));
        assert!(!menu.contains("6."));
    }
}
```

- [ ] **Step 2: Run tests — expect compile error**

Run: `cargo nextest run --lib -E 'test(format_model_menu)'`
Expected: FAIL — function doesn't exist

- [ ] **Step 3: Implement `format_model_menu()` helper**

Add to `src/cli/onboard.rs`:

```rust
/// Format a numbered model selection menu.
fn format_model_menu(models: &[String], max_display: usize) -> String {
    let mut output = String::new();
    for (i, model) in models.iter().take(max_display).enumerate() {
        output.push_str(&format!("  {}. {}\n", i + 1, model));
    }
    if models.len() > max_display {
        output.push_str(&format!("  ... ({} more)\n", models.len() - max_display));
    }
    output.push_str("  c. Custom (enter model ID)\n");
    output.push_str("  s. Skip (keep default)\n");
    output
}
```

- [ ] **Step 4: Run tests — expect PASS**

Run: `cargo nextest run --lib -E 'test(format_model_menu)'`
Expected: ALL PASS

- [ ] **Step 5: Implement `configure_model()`**

Add to `src/cli/onboard.rs`:

```rust
use zeptoclaw::channels::model_switch::KNOWN_MODELS;
use zeptoclaw::providers::configured_provider_names;

/// Model selection step for onboarding.
///
/// Shows available models for the configured provider(s) and lets the user pick one.
/// Tries live fetch first, falls back to KNOWN_MODELS.
async fn configure_model(config: &mut Config) -> Result<()> {
    let providers = configured_provider_names(config);
    if providers.is_empty() {
        return Ok(());
    }

    println!();
    println!("Model Selection");
    println!("===============");

    // If multiple providers, ask which is primary
    let primary = if providers.len() > 1 {
        println!("Multiple providers configured. Which should be your default?");
        for (i, p) in providers.iter().enumerate() {
            println!("  {}. {}", i + 1, p);
        }
        println!();
        print!("Choice [1]: ");
        io::stdout().flush()?;
        let input = read_line()?;
        let idx = input.trim().parse::<usize>().unwrap_or(1).saturating_sub(1);
        providers.get(idx).copied().unwrap_or(providers[0])
    } else {
        providers[0]
    };

    // Try live fetch, fall back to KNOWN_MODELS
    println!();
    println!("Fetching available models from {}...", primary);

    let selections = zeptoclaw::providers::resolve_runtime_providers(config);
    let selection = selections.iter().find(|s| s.name == primary);

    let models: Vec<String> = if let Some(s) = selection {
        match super::common::fetch_provider_models(s.name, &s.api_key, s.api_base.as_deref()).await
        {
            Ok(m) if !m.is_empty() => m,
            _ => {
                println!("  Could not fetch live models, showing known models.");
                KNOWN_MODELS
                    .iter()
                    .filter(|km| km.provider == primary)
                    .map(|km| km.model.to_string())
                    .collect()
            }
        }
    } else {
        KNOWN_MODELS
            .iter()
            .filter(|km| km.provider == primary)
            .map(|km| km.model.to_string())
            .collect()
    };

    if models.is_empty() {
        println!("  No models found for {}. Keeping default.", primary);
        return Ok(());
    }

    println!();
    println!("Which model would you like to use?");
    print!("{}", format_model_menu(&models, 15));
    println!();
    print!("Choice [1]: ");
    io::stdout().flush()?;

    let input = read_line()?;
    let input = input.trim();

    match input {
        "s" | "S" | "" => {
            println!("  Keeping default model: {}", config.agents.defaults.model);
        }
        "c" | "C" => {
            print!("Enter model ID: ");
            io::stdout().flush()?;
            let custom = read_line()?;
            let custom = custom.trim();
            if !custom.is_empty() {
                config.agents.defaults.model = custom.to_string();
                println!("  Set model to: {}", custom);
            }
        }
        choice => {
            if let Ok(idx) = choice.parse::<usize>() {
                if idx >= 1 && idx <= models.len() {
                    config.agents.defaults.model = models[idx - 1].clone();
                    println!("  Set model to: {}", models[idx - 1]);
                } else {
                    println!("  Invalid choice. Keeping default.");
                }
            } else {
                // Treat as a direct model ID
                config.agents.defaults.model = choice.to_string();
                println!("  Set model to: {}", choice);
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 6: Wire `configure_model()` into both onboarding paths**

In `cmd_onboard()`, for the **full** path (~line 250, after `configure_providers()`):

```rust
        configure_providers(&mut config).await?;
        configure_model(&mut config).await?;   // ADD THIS LINE
        configure_soul(&config)?;
```

For the **express** path (~line 311, after `configure_providers()` and before `configure_soul()`):

```rust
        } else {
            configure_providers(&mut config).await?;
        }

        configure_model(&mut config).await?;   // ADD THIS LINE

        configure_soul(&config)?;
```

- [ ] **Step 7: Build to verify compilation**

Run: `cargo build`
Expected: Compiles successfully

- [ ] **Step 8: Commit**

```bash
git add src/cli/onboard.rs
git commit -m "feat(onboard): add model selection step after provider setup"
```

---

### Task 8: Final checks — fmt, clippy, full test suite

**Files:** All modified files

- [ ] **Step 1: Run formatter**

Run: `cargo fmt`

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 3: Run full test suite**

Run: `cargo nextest run --lib`
Expected: ALL PASS

- [ ] **Step 4: Run doc tests**

Run: `cargo test --doc`
Expected: ALL PASS

- [ ] **Step 5: Final commit if fmt changed anything**

```bash
git add src/ && git diff --cached --quiet || git commit -m "chore: fmt"
```

- [ ] **Step 6: Verify format check**

Run: `cargo fmt -- --check`
Expected: No changes needed
