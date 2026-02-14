---
title: CLI Reference
description: Complete command reference for the ZeptoClaw CLI
tableOfContents:
  minHeadingLevel: 2
  maxHeadingLevel: 3
---

ZeptoClaw uses a subcommand-based CLI built with [clap](https://docs.rs/clap).

## Global options

```
zeptoclaw [OPTIONS] <COMMAND>
```

| Option | Description |
|--------|-------------|
| `--help` | Show help message |
| `--version` | Show version |

## agent

Run a single agent interaction.

```bash
zeptoclaw agent [OPTIONS] -m <MESSAGE>
```

| Option | Description |
|--------|-------------|
| `-m, --message <TEXT>` | Message to send to the agent |
| `--stream` | Enable streaming (token-by-token output) |
| `--template <NAME>` | Use an agent template (coder, researcher, writer, analyst) |
| `--workspace <PATH>` | Set workspace directory |

### Examples

```bash
# Simple message
zeptoclaw agent -m "Hello"

# With streaming
zeptoclaw agent --stream -m "Explain async Rust"

# With template
zeptoclaw agent --template coder -m "Write a CSV parser"
```

## gateway

Start the multi-channel gateway.

```bash
zeptoclaw gateway [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--containerized [RUNTIME]` | Enable container isolation (auto, docker, apple) |

### Examples

```bash
# Start gateway
zeptoclaw gateway

# With container isolation
zeptoclaw gateway --containerized docker
```

## batch

Process multiple prompts from a file.

```bash
zeptoclaw batch [OPTIONS] --input <FILE>
```

| Option | Description |
|--------|-------------|
| `--input <FILE>` | Input file (text or JSONL) |
| `--output <FILE>` | Output file (default: stdout) |
| `--format <FORMAT>` | Output format: text, jsonl |
| `--template <NAME>` | Agent template to use |
| `--stream` | Enable streaming per prompt |
| `--stop-on-error` | Stop on first error |

### Examples

```bash
# Process text file
zeptoclaw batch --input prompts.txt

# JSONL output
zeptoclaw batch --input prompts.txt --format jsonl --output results.jsonl

# With template and error handling
zeptoclaw batch --input prompts.jsonl --template researcher --stop-on-error
```

## config check

Validate configuration file.

```bash
zeptoclaw config check
```

Reports unknown fields, missing required values, and type errors.

## history

Manage conversation history.

```bash
zeptoclaw history <SUBCOMMAND>
```

### history list

```bash
zeptoclaw history list [--limit <N>]
```

List recent sessions with timestamps and titles.

### history show

```bash
zeptoclaw history show <QUERY>
```

Show a session by fuzzy-matching the query against session titles and keys.

### history cleanup

```bash
zeptoclaw history cleanup [--keep <N>]
```

Remove old sessions, keeping the most recent N (default: 50).

## template

Manage agent templates.

```bash
zeptoclaw template <SUBCOMMAND>
```

### template list

List all available templates (built-in and custom).

### template show

```bash
zeptoclaw template show <NAME>
```

Show template details including system prompt, model, and overrides.

## onboard

Run the interactive setup wizard.

```bash
zeptoclaw onboard
```

Walks through provider key setup, channel configuration, and workspace initialization.

## heartbeat

View heartbeat service status.

```bash
zeptoclaw heartbeat --show
```

## skills

Manage agent skills.

```bash
zeptoclaw skills list
```

List available skills from `~/.zeptoclaw/skills/`.
