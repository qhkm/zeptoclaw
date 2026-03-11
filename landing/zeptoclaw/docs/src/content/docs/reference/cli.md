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
| `--tunnel [PROVIDER]` | Enable tunnel (auto, cloudflare, ngrok, tailscale) |

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
zeptoclaw onboard [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--full` | Run the full 10-step wizard instead of express setup |

Walks through provider key setup, channel configuration, and workspace initialization.

## heartbeat

View heartbeat service status.

```bash
zeptoclaw heartbeat --show
```

## uninstall

Remove ZeptoClaw state and optionally the current binary.

```bash
zeptoclaw uninstall [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--remove-binary` | Remove the current binary for direct installs in `~/.local/bin` or `/usr/local/bin` |
| `-y, --yes` | Skip the confirmation prompt |

### Examples

```bash
# Remove ~/.zeptoclaw
zeptoclaw uninstall --yes

# Remove ~/.zeptoclaw and a direct-install binary
zeptoclaw uninstall --remove-binary --yes
```

## skills

Manage agent skills.

```bash
zeptoclaw skills list
```

List available skills from `~/.zeptoclaw/skills/`.

## secrets

Manage secret encryption at rest.

```bash
zeptoclaw secrets <SUBCOMMAND>
```

### secrets encrypt

Encrypt plaintext API keys and tokens in your config file using XChaCha20-Poly1305.

```bash
zeptoclaw secrets encrypt
```

### secrets decrypt

Decrypt secrets for editing.

```bash
zeptoclaw secrets decrypt
```

### secrets rotate

Re-encrypt with a new master key.

```bash
zeptoclaw secrets rotate
```

## memory

Manage long-term memory from the CLI.

```bash
zeptoclaw memory <SUBCOMMAND>
```

### memory list

```bash
zeptoclaw memory list [--category <CATEGORY>]
```

### memory search

```bash
zeptoclaw memory search <QUERY>
```

### memory set

```bash
zeptoclaw memory set <KEY> <VALUE> [--category <CATEGORY>] [--tags <TAGS>]
```

### memory delete

```bash
zeptoclaw memory delete <KEY>
```

### memory stats

```bash
zeptoclaw memory stats
```

## tools

Discover available tools.

```bash
zeptoclaw tools <SUBCOMMAND>
```

### tools list

List all available tools and their status.

```bash
zeptoclaw tools list
```

### tools info

Show detailed info about a specific tool.

```bash
zeptoclaw tools info <NAME>
```

## watch

Monitor a URL for changes.

```bash
zeptoclaw watch <URL> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--interval <DURATION>` | Check interval (e.g., 1h, 30m) |
| `--notify <CHANNEL>` | Channel for notifications |

## channel

Manage channels.

```bash
zeptoclaw channel <SUBCOMMAND>
```

### channel list

```bash
zeptoclaw channel list
```

### channel setup

```bash
zeptoclaw channel setup <NAME>
```

### channel test

```bash
zeptoclaw channel test <NAME>
```

## migrate

Import config and skills from an OpenClaw installation.

```bash
zeptoclaw migrate [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--from <PATH>` | Path to OpenClaw installation |
| `--dry-run` | Preview migration without writing files |
| `--yes` | Non-interactive (skip prompts) |
