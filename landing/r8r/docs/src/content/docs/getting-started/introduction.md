---
title: Introduction
description: What is r8r and why should you use it?
---

**r8r** (pronounced "rator") is a lightning-fast workflow automation tool built in Rust. It's designed for developers who need reliable, high-performance automation without the bloat of traditional tools.

## Why r8r?

### 🚀 Blazing Fast

- **15MB binary** — Smaller than most JavaScript dependencies
- **Sub-millisecond latency** — Processes workflows faster than you can blink
- **50,000+ ops/sec** — Handle massive throughput on modest hardware

### 🦀 Built in Rust

- **Memory safe** — No runtime crashes, no memory leaks
- **Zero-cost abstractions** — Pay only for what you use
- **Single static binary** — Deploy anywhere without dependencies

### 🔌 Developer First

- **YAML or code** — Define workflows declaratively or write custom nodes in Rust
- **200+ integrations** — Native support for databases, APIs, queues, and more
- **Observable** — Built-in metrics and tracing with OpenTelemetry export

## What can you build?

- **ETL pipelines** — Extract, transform, and load data between systems
- **API integrations** — Connect services and automate data flows
- **Scheduled tasks** — Cron-like automation with better observability
- **Event processing** — React to webhooks, queue messages, or database changes
- **DevOps automation** — Deploy, monitor, and respond to infrastructure events

## How it works

```yaml
name: "Daily Report"
trigger:
  schedule: "0 9 * * *"

nodes:
  - name: "fetch_sales"
    type: "postgres/query"
    config:
      sql: |
        SELECT * FROM sales 
        WHERE date > NOW() - INTERVAL '24h'

  - name: "format_report"
    type: "template"
    input: "{{ fetch_sales.rows }}"

  - name: "send_slack"
    type: "slack/post"
    config:
      channel: "#daily-reports"
```

## Comparison

| Feature | r8r | n8n | Zapier |
|---------|-----|-----|--------|
| Binary size | 15MB | 500MB+ | Cloud only |
| Self-hosted | ✅ | ✅ | ❌ |
| Custom nodes | Rust/WASM | JavaScript | Limited |
| Open source | ✅ | ✅ | ❌ |
| Performance | Native | Node.js | Cloud dependent |

## Next steps

Ready to get started? [Install r8r](/getting-started/installation/) and build your first workflow in minutes.
