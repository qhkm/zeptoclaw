---
title: Tools Reference
description: Complete reference for all built-in agent tools
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---

ZeptoClaw ships with 32 built-in tools. Most tools are available by default; `grep` and `find` are opt-in coding tools (see below).

## shell

Execute shell commands with optional container isolation.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `command` | string | Yes | The shell command to execute |

**Security:** Commands are checked against a regex blocklist (dangerous patterns like `rm -rf /`, `curl | sh`, etc.) and can be isolated in Docker or Apple Container.

## read_file

Read file contents from the workspace.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Relative path within workspace |

## write_file

Write or create files in the workspace.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Relative path within workspace |
| `content` | string | Yes | File contents to write |

## list_files

List directory contents.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | No | Directory path (default: workspace root) |

## edit_file

Edit an existing file. Supports two modes — string replacement and unified diff.

**String replacement mode:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Relative path within workspace |
| `old_text` | string | Yes | Exact text to find and replace |
| `new_text` | string | Yes | Replacement text |

**Unified diff mode** (pass `diff` instead of `old_text`/`new_text`):

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Relative path within workspace |
| `diff` | string | Yes | Unified diff string (`@@ -old +new @@` format) |

The two modes are mutually exclusive. Diff mode is useful for LLMs that emit standard `diff` output.

## grep

Search file contents by regex pattern. **Opt-in coding tool** — requires `--template coder` or `tools.coding_tools: true`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | Yes | Regex pattern to search for |
| `path` | string | No | Directory or file to search (default: workspace root) |
| `glob` | string | No | Glob filter for file types (e.g. `*.rs`) |
| `ignore_case` | boolean | No | Case-insensitive search (default: false) |
| `limit` | integer | No | Max matches to return (default: 100) |

**Security:** Path argument is validated against the workspace boundary.

## find

Find files by glob pattern. **Opt-in coding tool** — requires `--template coder` or `tools.coding_tools: true`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `pattern` | string | Yes | Glob pattern (e.g. `**/*.rs`, `src/**/test_*`) |
| `path` | string | No | Root directory to search from (default: workspace root) |
| `limit` | integer | No | Max results to return (default: 200) |

**Security:** Path argument is validated against the workspace boundary.

## web_search

Search the web using the Brave Search API.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search query |

**Security:** SSRF protection blocks requests to private IP ranges, IPv6 loopback, and non-HTTP schemes. DNS pinning prevents rebinding attacks.

## web_fetch

Fetch and parse a web page.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | Yes | URL to fetch |

Returns cleaned text content (HTML stripped). Response body limited to prevent token waste.

## memory

Search workspace memory (markdown files).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search query |

Searches markdown files in the workspace, scoring by keyword relevance with chunked results.

## longterm_memory

Persistent key-value store with categories and tags.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: set, get, search, delete, list, categories |
| `key` | string | Varies | Memory key |
| `value` | string | Varies | Value to store |
| `category` | string | No | Category for organization |
| `tags` | array | No | Tags for filtering |

Stored at `~/.zeptoclaw/memory/longterm.json`. Persists across sessions with access tracking.

## message

Send proactive messages to channels.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `content` | string | Yes | Message text |
| `channel` | string | No | Target channel (telegram, slack, discord, webhook) |
| `chat_id` | string | No | Target chat ID |

Falls back to the current context's channel and chat_id if not specified.

## cron

Schedule recurring tasks.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: add, list, remove |
| `name` | string | Varies | Job name |
| `schedule` | string | Varies | Cron expression |
| `message` | string | Varies | Message to process |

## spawn

Delegate a background task.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `message` | string | Yes | Task description |
| `label` | string | No | Task label |

## delegate

Create a sub-agent (agent swarm).

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `role` | string | Yes | Sub-agent role/system prompt |
| `message` | string | Yes | Message to send |
| `tools` | array | No | Tool whitelist for sub-agent |

The delegate tool creates a temporary agent loop with a role-specific system prompt. Recursion is blocked to prevent infinite delegation chains.

## whatsapp

Send WhatsApp messages via Cloud API.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `to` | string | Yes | Recipient phone number |
| `message` | string | Yes | Message text |

## gsheets

Read and write Google Sheets.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: read, write, append |
| `spreadsheet_id` | string | Yes | Google Sheet ID |
| `range` | string | Yes | Cell range (e.g., "A1:B10") |
| `values` | array | Varies | Data to write |

## r8r

Content rating and analysis tool.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `content` | string | Yes | Content to analyze |

## reminder

Persistent reminders with cron-based delivery.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: add, list, complete, snooze, overdue |
| `title` | string | Varies | Reminder title |
| `due` | string | Varies | Due date/time |

## git

Git operations as an agent tool.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: status, diff, log, commit |
| `message` | string | Varies | Commit message (for commit action) |

## project

Project scaffolding and management.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | Project operation to perform |

## stripe

Stripe API integration for payment operations.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | Stripe API action |
| `params` | object | Varies | Action-specific parameters |

## http_request

General-purpose HTTP client for arbitrary API calls.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `method` | string | Yes | HTTP method (GET, POST, PUT, DELETE, etc.) |
| `url` | string | Yes | Request URL |
| `headers` | object | No | Request headers |
| `body` | string | No | Request body |

## pdf_read

Extract text from PDF files. Requires `--features tool-pdf`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Path to PDF file |

## transcribe

Audio transcription with provider abstraction.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `path` | string | Yes | Path to audio file |
| `provider` | string | No | Transcription provider |

## screenshot

Capture webpage screenshots. Requires `--features screenshot`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `url` | string | Yes | URL to screenshot |

## find_skills

Search the skill registry for available skills.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `query` | string | Yes | Search query |

## install_skill

Install a skill from the registry.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `name` | string | Yes | Skill name to install |

## android

Control an Android device via ADB. Requires `--features android`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | One of: screen, tap, type, swipe, scroll, back, home, screenshot, launch, open_url, etc. |
| Various | Various | Varies | Action-specific parameters (x, y, text, package, url, etc.) |

**Security:** URL scheme allowlist (blocks javascript:, file:, intent:), shell metacharacter blocking, busybox/toybox wrapper detection.

## hardware

GPIO, serial, and USB peripheral operations. Requires `--features hardware`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `action` | string | Yes | Hardware operation to perform |
| `device` | string | Varies | Device identifier |
