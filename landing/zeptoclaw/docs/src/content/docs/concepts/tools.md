---
title: Tools
description: How agent tools work in ZeptoClaw
---

Tools are the actions your agent can take. Each tool implements the `Tool` trait with a name, description, parameter schema, and an async `execute` method.

## How tools work

When an LLM decides to use a tool, it returns a structured tool call with a name and JSON arguments. The agent loop:

1. Looks up the tool by name in the registry
2. Checks the approval gate (if configured)
3. Calls `execute()` with the arguments and a `ToolContext`
4. Sanitizes the result (strips large blobs, truncates)
5. Returns the result to the LLM

## Tool context

Every tool receives a `ToolContext` containing:

- **workspace** — Path to the agent's workspace directory
- **channel** — The originating channel name (e.g., "telegram")
- **chat_id** — The originating chat/conversation ID

## Built-in tools

ZeptoClaw ships with 32 built-in tools:

| Tool | Description |
|------|-------------|
| `shell` | Execute shell commands (with optional container isolation) |
| `read_file` | Read file contents from workspace |
| `write_file` | Write or create files in workspace |
| `list_files` | List directory contents |
| `edit_file` | Search-and-replace edits (string or unified diff mode) |
| `grep` | Search file contents by regex pattern across the workspace |
| `find` | Find files by glob pattern (e.g. `**/*.rs`) |
| `web_search` | Web search via Brave API |
| `web_fetch` | Fetch and parse web pages |
| `http_request` | General-purpose HTTP client for arbitrary API calls |
| `memory` | Search workspace memory (markdown files) |
| `longterm_memory` | Persistent key-value store with categories and tags |
| `message` | Send proactive messages to channels |
| `cron` | Schedule recurring tasks |
| `spawn` | Delegate background tasks |
| `delegate` | Create sub-agents (agent swarms) |
| `whatsapp` | Send WhatsApp messages via Cloud API |
| `gsheets` | Read and write Google Sheets |
| `r8r` | R8r workflow integration |
| `reminder` | Persistent reminders with cron delivery |
| `git` | Git operations (status, diff, log, commit) |
| `project` | Project scaffolding and management |
| `stripe` | Stripe API integration for payment operations |
| `pdf_read` | Extract text from PDF files (feature-gated) |
| `transcribe` | Audio transcription with provider abstraction |
| `screenshot` | Capture webpage screenshots (feature-gated) |
| `find_skills` | Search the skill registry |
| `install_skill` | Install skills from the registry |
| `android` | Android device control via ADB (feature-gated) |
| `hardware` | GPIO, serial, and USB peripheral operations (feature-gated) |

Some tools are feature-gated and require compile-time flags: `--features tool-pdf` for PDF, `--features screenshot` for screenshots, `--features android` for Android, `--features hardware` for hardware peripherals.

### Opt-in coding tools

`grep` and `find` are **disabled by default** to keep the core runtime portable for IoT and embedded environments where bash may not be available. Enable them by:

- Using the `coder` template: `zeptoclaw agent --template coder -m "..."`
- Setting `tools.coding_tools: true` in `~/.zeptoclaw/config.json`
- Setting `ZEPTOCLAW_TOOLS_CODING_TOOLS=true` env var

## Parallel execution

When the LLM returns multiple tool calls in one response, ZeptoClaw executes them concurrently using `futures::future::join_all`. This reduces latency when tools are independent.

## Result sanitization

Tool results are sanitized before being sent back to the LLM:

- Base64 data URIs are replaced with `[base64 data removed]`
- Hex blobs (>100 chars) are replaced with `[hex data removed]`
- Results exceeding 50KB are truncated

This prevents token waste from large binary outputs.

## Custom tools via plugins

You can add custom tools without modifying ZeptoClaw's source code using the [plugin system](/docs/guides/plugins/). Plugins are JSON manifests that define tool name, parameters, and a command template.
