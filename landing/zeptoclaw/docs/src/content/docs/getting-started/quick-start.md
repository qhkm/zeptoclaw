---
title: Quick Start
description: Run your first ZeptoClaw agent in under a minute
---

This guide walks you through setting up ZeptoClaw and running your first agent interaction.

## 1. Run the setup wizard

ZeptoClaw includes an interactive onboarding command that configures your provider keys and workspace:

```bash
zeptoclaw onboard
```

This creates `~/.zeptoclaw/config.json` with your settings.

## 2. Or configure manually

Create the config directory and add your API key:

```bash
mkdir -p ~/.zeptoclaw

# Set your provider key
export ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...
# or
export ZEPTOCLAW_PROVIDERS_OPENAI_API_KEY=sk-...
```

## 3. Send your first message

```bash
zeptoclaw agent -m "Hello! What can you help me with?"
```

Add `--stream` for real-time token-by-token output:

```bash
zeptoclaw agent --stream -m "List the files in my current directory"
```

## 4. Use a template

ZeptoClaw includes 4 built-in agent templates with specialized system prompts:

```bash
# Research mode
zeptoclaw agent --template researcher -m "What are the latest Rust async patterns?"

# Code assistant
zeptoclaw agent --template coder -m "Write a function to parse CSV files"

# List available templates
zeptoclaw template list
```

## 5. Validate your configuration

Check that everything is wired correctly:

```bash
zeptoclaw config check
```

This validates your config file and reports any issues.

## 6. Run in gateway mode

To serve your agent on Telegram, Slack, Discord, or Webhook:

```bash
# Set your channel token
export ZEPTOCLAW_CHANNELS_TELEGRAM_BOT_TOKEN=123456:ABC...

# Start the gateway
zeptoclaw gateway
```

## 7. Process prompts in batch

For bulk processing, create a text file with one prompt per line:

```bash
echo "Summarize the Rust ownership model" > prompts.txt
echo "Explain async/await in 3 sentences" >> prompts.txt

zeptoclaw batch --input prompts.txt --format jsonl
```

## What's next?

- Learn about the [agent loop](/concepts/agent-loop/) to understand how messages are processed
- Browse available [tools](/reference/tools/) your agent can use
- Set up [channels](/concepts/channels/) for Telegram, Slack, or Discord
- Explore [plugins](/guides/plugins/) to extend your agent with custom tools
