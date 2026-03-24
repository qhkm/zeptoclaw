# Model Discoverability & Provider Auto-Selection

**Date:** 2026-03-23
**Status:** Approved
**Area:** config, providers, cli, channels

## Problem

Users cannot easily discover, select, or switch AI models in ZeptoClaw:

1. **Default always Claude** — compile-time default is `claude-sonnet-4-5-20250929` and provider resolution picks Anthropic first regardless of configured model name
2. **No model step in onboarding** — `zeptoclaw onboard` asks for provider + API key but never asks which model
3. **Hardcoded model list is stale** — `KNOWN_MODELS` lists GPT-5.1 and Gemini 2.5; current models are GPT-5.4 and Gemini 3.1
4. **`/model list` shows no usage hints** — users see models but don't know how to switch
5. **No live model discovery** — no way to see what models your API key actually has access to
6. **Silent mismatch** — setting `model: "gpt-4o"` with an Anthropic key silently sends the wrong model string to the wrong provider

## Prior Art (already shipped)

- **Provider auto-selection by model name** — `provider_name_for_model()` in `registry.rs` + reordering in `build_runtime_provider_chain()` (shipped same session, pre-spec)
- **`/model` slash command** — show, list, set, reset at runtime
- **`zeptoclaw onboard`** — express + full wizard with provider API key setup
- **`zeptoclaw provider status`** — shows resolved providers with redacted keys
- **`validate_api_key()`** — hits `/v1/models` for Anthropic/OpenAI key validation (discards response body); OpenRouter uses `/key` endpoint instead
- **`provider_name_for_model()`** in `registry.rs` — matches model string against provider `model_keywords` (added same session, committed with this spec's implementation)

## Design

### 1. Update `KNOWN_MODELS` to current models

**Note:** Old models (e.g. `claude-sonnet-4-5-20250929`, `gpt-4.1`, `gpt-4.1-mini`) are intentionally removed. They still work at runtime if referenced in config or long-term memory overrides — they just won't appear in `/model list`. The list shows current recommendations, not an exhaustive history.

Replace the stale hardcoded list in `src/channels/model_switch.rs`. Current models as of 2026-03-23:

**Anthropic:**
- `claude-opus-4-6` — Claude Opus 4.6
- `claude-sonnet-4-6` — Claude Sonnet 4.6
- `claude-haiku-4-5-20251001` — Claude Haiku 4.5

**OpenAI:**
- `gpt-5.4` — GPT-5.4
- `gpt-5.4-mini` — GPT-5.4 Mini
- `gpt-5.4-nano` — GPT-5.4 Nano
- `gpt-5.3-codex` — GPT-5.3 Codex

**OpenRouter:**
- `anthropic/claude-sonnet-4-6` — Claude Sonnet 4.6 (OR)
- `google/gemini-3.1-pro` — Gemini 3.1 Pro (OR)

**Groq:**
- `llama-4-scout-17b-16e-instruct` — Llama 4 Scout
- `llama-4-maverick-17b-128e-instruct` — Llama 4 Maverick

**Gemini:**
- `gemini-3.1-pro` — Gemini 3.1 Pro
- `gemini-3.1-flash-lite` — Gemini 3.1 Flash Lite

**Ollama:**
- `llama3.3` — Llama 3.3 (local)
- `mistral` — Mistral (local)

**DeepSeek:**
- `deepseek-chat` — DeepSeek V3
- `deepseek-reasoner` — DeepSeek R1

**Kimi:**
- `moonshot-v1-128k` — Kimi 128K
- `moonshot-v1-32k` — Kimi 32K

### 2. Update compile-time default model

Change `COMPILE_TIME_DEFAULT_MODEL` in `src/config/types.rs` from `claude-sonnet-4-5-20250929` to `claude-sonnet-4-6`.

**Important:** This change and the `KNOWN_MODELS` update (Section 1) must be applied in the same commit to avoid an intermediate state where the default model doesn't appear in `/model list`.

### 3. Add `fetch_provider_models()` for live model discovery

New async function in `src/cli/common.rs` (co-located with existing `validate_api_key()` which already uses `reqwest`; `registry.rs` is sync-only and should stay that way):

```rust
pub async fn fetch_provider_models(
    provider: &str,
    api_key: &str,
    api_base: Option<&str>,
) -> Result<Vec<String>>
```

**Behavior:**
- Calls the provider's models endpoint and parses the response to extract model IDs
- Returns sorted `Vec<String>` of model IDs
- On failure (network, rate limit, parse error), returns `Err` — callers fall back to `KNOWN_MODELS`

**API endpoints per provider:**

| Provider | Models Endpoint | Key Validation Endpoint | Response format |
|----------|----------------|------------------------|----------------|
| Anthropic | `GET {base}/v1/models` | Same | `{"data": [{"id": "..."}]}` |
| OpenAI | `GET {base}/models` | Same | `{"data": [{"id": "..."}]}` |
| OpenRouter | `GET {base}/models` | **Different:** `GET {base}/key` | `{"data": [{"id": "..."}]}` |
| Groq | `GET {base}/models` | Same | `{"data": [{"id": "..."}]}` |
| Gemini | `GET {base}/models` | Same | OpenAI-compat format via `v1beta/openai` base URL: `{"data": [{"id": "..."}]}` |
| Ollama | `GET {base}/../api/tags` | N/A (keyless) | `{"models": [{"name": "..."}]}` — needs special parsing |
| DeepSeek, Kimi, etc. | `GET {base}/models` | Same | `{"data": [{"id": "..."}]}` |

**Note on Gemini:** The registry configures Gemini's `default_base_url` as the OpenAI-compat shim (`v1beta/openai`), so `{base}/models` returns OpenAI-format JSON. No special Gemini parser needed.

**Note on Ollama:** Ollama's base URL in the registry is `http://localhost:11434/v1` (OpenAI-compat), but its native model list is at `/api/tags`. To construct the URL: strip the trailing `/v1` from the base URL (e.g., `base.trim_end_matches("/v1")`) and append `/api/tags`, yielding `http://localhost:11434/api/tags`. Needs its own response parser (`{"models": [{"name": "..."}]}`).

**Timeout:** 10 seconds (same as `validate_api_key()`).

### 4. Onboarding model selection step

Add a new `configure_model()` function called **after** `configure_providers()` returns. This applies to **both** express and full onboarding paths.

**Control flow:**

1. Collect all providers that were just configured (have API keys)
2. If zero providers configured → skip model selection entirely
3. If one provider configured → fetch its models and show selection menu
4. If multiple providers → first ask which is the primary provider, then show that provider's model menu

**Step 3/4 — primary provider selection (only when multiple):**
```
Multiple providers configured. Which should be your default?
  1. anthropic
  2. openai
  3. openrouter

Choice [1]:
```

**Model selection menu (for the chosen primary provider):**

1. Call `fetch_provider_models()` to get live models
2. If fetch fails, fall back to `KNOWN_MODELS` for that provider
3. Show numbered menu of top models (max 15, sorted by relevance — put known popular models first)
4. User picks a number, enters a custom model ID, or skips
5. Selected model is written to `config.agents.defaults.model`

```
Model Selection
===============
Fetching available models from OpenAI...

Which model would you like to use?
  1. gpt-5.4            GPT-5.4 (latest)
  2. gpt-5.4-mini       GPT-5.4 Mini (fast & cheap)
  3. gpt-5.4-nano       GPT-5.4 Nano (fastest)
  4. gpt-5.3-codex      GPT-5.3 Codex (coding)
  ... more models ...
  c. Custom (enter model ID)
  s. Skip (keep default)

Choice [1]:
```

### 5. Enhanced `/model list` with usage hints

Append usage hints to `format_model_list()` output:

```
Switch model:  /model <model-id>
With provider: /model openai:gpt-5.4
Reset:         /model reset
Config:        agents.defaults.model in ~/.zeptoclaw/config.json
```

Four lines added to the bottom of the existing output. No behavioral change to the list itself.

### 6. `/model fetch` — live model listing

New subcommand that fetches models from all configured providers in real-time.

**Implementation changes:**
- Add `Fetch` variant to `ModelCommand` enum in `src/channels/model_switch.rs`
- Add `"fetch"` arm to `parse_model_command()` pattern matching
- Add dispatch in `src/cli/agent.rs` `match mcmd { ModelCommand::Fetch => ... }`
- Add dispatch in `src/channels/telegram.rs` (or use `_ => {}` catch-all if Telegram shouldn't support it — Fetch requires async network calls which may not suit all channel contexts)
- Add `"model fetch"` entry to `builtin_commands()` in `src/cli/slash.rs`

```
/model fetch
```

Output:
```
Fetching models from configured providers...

anthropic (2 models):
  claude-opus-4-6
  claude-sonnet-4-6

openai (12 models):
  gpt-5.4
  gpt-5.4-mini
  gpt-5.4-nano
  ...

Switch: /model <model-id>
```

This is the async/network version. Default `/model list` stays fast (no network call).

### 7. Startup warning for model-provider mismatch

In `cli/common.rs` during agent setup, after provider chain is built, if `provider_name_for_model()` returns `None` and providers are configured:

```
⚠ Model "some-unknown-model" doesn't match any known provider.
  Configured providers: anthropic, openai
  Run /model list to see available models.
```

No network call. Just a `tracing::warn!()` that surfaces in CLI output. Not a hard error — the agent still starts.

## Files Changed

| File | Change |
|------|--------|
| `src/channels/model_switch.rs` | Update `KNOWN_MODELS`, add `Fetch` variant to `ModelCommand`, update `parse_model_command()` |
| `src/config/types.rs` | Update `COMPILE_TIME_DEFAULT_MODEL` to `claude-sonnet-4-6` |
| `src/providers/registry.rs` | Add `provider_name_for_model()` (already written, needs to ship with this work) |
| `src/providers/mod.rs` | Export `provider_name_for_model` |
| `src/cli/common.rs` | Add `fetch_provider_models()`, add startup model-provider mismatch warning |
| `src/cli/onboard.rs` | Add `configure_model()` step after provider setup (both express and full paths) |
| `src/cli/agent.rs` | Handle `ModelCommand::Fetch` dispatch |
| `src/cli/slash.rs` | Add `model fetch` to `builtin_commands()` |
| `src/channels/telegram.rs` | Handle `ModelCommand::Fetch` arm (or catch-all) |

## Testing

- Unit tests for `fetch_provider_models()` response parsing (mock JSON for OpenAI-format, Ollama-format)
- Unit tests for onboarding model menu formatting
- Unit test for updated `KNOWN_MODELS` (no empty fields, no duplicates)
- Unit test for `/model list` usage hints in output
- Unit test for startup warning logic (`provider_name_for_model()` returns `None`)
- Unit test for `parse_model_command("/model fetch")` returning `ModelCommand::Fetch`
- Unit test for `ModelCommand::Fetch` dispatch in agent.rs
- Existing `/model` tests continue to pass

## Out of Scope

- Model pricing display (future enhancement)
- Model capability comparison (context window, vision support, etc.)
- Auto-updating `KNOWN_MODELS` from a remote registry
- Model aliases (e.g. "claude" → "claude-sonnet-4-6")
