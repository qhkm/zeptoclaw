---
title: Agent Templates
description: Use and create agent templates for specialized roles
---

Agent templates let you configure specialized agent behaviors with custom system prompts, model selection, and parameter overrides.

## Built-in templates

ZeptoClaw ships with 4 built-in templates:

| Template | Description | System prompt focus |
|----------|-------------|-------------------|
| `coder` | Code assistant | Write clean, tested code |
| `researcher` | Research assistant | Search, analyze, summarize |
| `writer` | Writing assistant | Clear, concise prose |
| `analyst` | Data analyst | Analyze data, find patterns |

## Using templates

### CLI

```bash
# Use a template
zeptoclaw agent --template researcher -m "Research Rust async patterns"

# List available templates
zeptoclaw template list

# Show template details
zeptoclaw template show coder
```

### Batch mode

```bash
zeptoclaw batch --input prompts.txt --template coder
```

## Template overrides

Templates can override:

- **System prompt** — Custom instructions for the agent role
- **Model** — Use a different LLM model
- **Max tokens** — Adjust response length
- **Temperature** — Control response creativity

## Custom templates

Create a JSON file in `~/.zeptoclaw/templates/`:

```json
{
  "name": "devops",
  "description": "DevOps automation specialist",
  "system_prompt": "You are a DevOps engineer. Focus on infrastructure automation, CI/CD pipelines, and deployment best practices. Always consider security implications.",
  "model": "claude-sonnet-4-5-20250929",
  "max_tokens": 4096,
  "temperature": 0.3
}
```

Then use it:

```bash
zeptoclaw agent --template devops -m "Set up a GitHub Actions pipeline"
```

## Template + tool whitelists

Combine templates with the delegate tool's tool whitelist for controlled sub-agents:

```json
{
  "name": "safe_researcher",
  "description": "Research-only agent (no shell or file writes)",
  "system_prompt": "You are a research assistant. Search the web and analyze information.",
  "tools": ["web_search", "web_fetch", "memory", "longterm_memory"]
}
```

The agent using this template can only access the whitelisted tools.

## How templates are applied

1. Template system prompt replaces the default system prompt
2. Model override replaces the config default (if specified)
3. Token and temperature overrides replace defaults (if specified)
4. Tool whitelist restricts available tools (if specified)
5. The rest of the config (providers, channels, etc.) remains unchanged
