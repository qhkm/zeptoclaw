---
title: Introduction
description: What is ZeptoClaw and why should you use it?
---

**ZeptoClaw** is an ultra-lightweight personal AI assistant built in Rust. It packages streaming LLM responses, agent swarms, a plugin system, batch processing, 8 messaging channels, and container isolation into a single ~4MB binary.

## Why ZeptoClaw?

### Tiny footprint

- **~4MB binary** — Smaller than most app icons
- **~6MB RSS** — Runs on the cheapest VPS
- **~50ms startup** — Ready before you finish typing

### Built in Rust

- **Memory safe** — No runtime crashes, no garbage collector
- **Async-first** — Tokio runtime for non-blocking I/O
- **2,300+ tests** — Thoroughly tested across unit, integration, and doc tests

### Production-ready features

- **Streaming** — Real-time SSE from both Claude and OpenAI
- **Agent swarms** — Delegate subtasks to specialized sub-agents
- **Plugin system** — Extend with JSON manifest plugins
- **Container isolation** — Run shell commands in Docker or Apple Container
- **8 channels** — Telegram, Slack, Discord, WhatsApp Cloud, Lark, Email, Webhook, and CLI
- **29 built-in tools** — Shell, filesystem, web, git, stripe, PDF, transcription, and more
- **Agent modes** — Observer, Assistant, Autonomous — category-based tool access control
- **Secret encryption** — XChaCha20-Poly1305 AEAD for API keys at rest

## What can you build?

- **Personal AI assistant** — Chat via Telegram, Slack, or Discord
- **Automated workflows** — Schedule cron jobs that use AI to act
- **Code review bots** — Agent reads code, runs tests, reports findings
- **Data pipelines** — Batch-process hundreds of prompts from files
- **Multi-agent systems** — Swarms with specialized roles and tool whitelists

## How it works

```bash
# One-shot CLI mode
$ zeptoclaw agent -m "Summarize the last 5 git commits"

# Gateway mode — serves Telegram, Slack, Discord, Webhook
$ zeptoclaw gateway

# Batch mode — process prompts from a file
$ zeptoclaw batch --input prompts.txt --format jsonl
```

The agent receives a message, builds a system prompt with context, calls an LLM provider, executes any tool calls the model requests, and returns the final response. Tools include shell execution, file operations, web search, memory storage, and more.

## Comparison

| Feature | ZeptoClaw | LangChain | AutoGPT |
|---------|-----------|-----------|---------|
| Binary size | ~4MB | 100MB+ | 200MB+ |
| Language | Rust | Python | Python |
| Self-hosted | Single binary | pip install | Docker |
| Container isolation | Built-in | No | Docker only |
| Streaming | SSE native | Varies | No |
| Tests | 2,300+ | Varies | Varies |

## Next steps

Ready to get started? [Install ZeptoClaw](/docs/getting-started/installation/) and run your first agent in under a minute.
