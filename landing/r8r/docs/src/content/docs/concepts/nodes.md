---
title: Nodes
description: The building blocks of workflows
---

Nodes are the individual steps in a workflow. Each node has a type, configuration, and connections.

## Node structure

```yaml
nodes:
  - name: "fetch_data"      # Unique identifier
    type: "http/request"    # Node type
    config:                 # Type-specific config
      url: "https://api.example.com"
      method: GET
    input:                  # Optional input mapping
      query: "{{ trigger.body.query }}"
```

## Built-in types

### Core nodes

| Type | Purpose |
|------|---------|
| `core/log` | Log to stdout |
| `core/condition` | Branch based on condition |
| `core/sleep` | Delay execution |
| `core/template` | Transform data |

### Data nodes

| Type | Purpose |
|------|---------|
| `postgres/query` | PostgreSQL operations |
| `redis/get` | Redis read |
| `redis/set` | Redis write |

### HTTP nodes

| Type | Purpose |
|------|---------|
| `http/request` | Generic HTTP call |
| `http/webhook` | Return response |

### Message nodes

| Type | Purpose |
|------|---------|
| `slack/post` | Send Slack message |
| `discord/post` | Send Discord message |
| `kafka/produce` | Publish to Kafka |

## Custom nodes

Build nodes in Rust:

```rust
use r8r_sdk::{Node, Context, Result};

#[derive(Node)]
#[node(name = "custom/hello")]
struct HelloNode {
    name: String,
}

impl Node for HelloNode {
    async fn execute(&self, ctx: Context) -> Result<Value> {
        Ok(json!({ "message": format!("Hello, {}!", self.name) }))
    }
}
```

Register in `r8r.toml`:

```toml
[nodes]
custom = "./nodes"
```

## Input/output

Nodes communicate via JSON:

```yaml
nodes:
  - name: "fetch"
    type: "http/request"
    # Outputs: { "status": 200, "body": {...} }

  - name: "process"
    type: "core/template"
    input: "{{ fetch.body.items }}"
    # Receives the items array
```

## Error handling

Per-node error control:

```yaml
nodes:
  - name: "api_call"
    type: "http/request"
    config: { ... }
    on_error:
      action: retry
      max_attempts: 3
      delay: 5s
```
