---
title: Workflows
description: Understanding r8r workflows
---

A workflow in r8r is a collection of nodes connected by data flow, triggered by events.

## Workflow structure

```yaml
name: "my-workflow"           # Required: unique identifier
description: "Does something" # Optional: human-readable
trigger:                      # Required: how to start
  http:
    path: /api/webhook
nodes:                        # Required: what to execute
  - name: "step1"
    type: "http/request"
  - name: "step2"
    type: "core/log"
```

## Triggers

Every workflow needs a trigger — the event that starts execution:

### HTTP trigger

```yaml
trigger:
  http:
    path: /webhook
    method: POST
    auth:
      type: bearer
```

### Schedule trigger

```yaml
trigger:
  schedule: "0 */6 * * *"  # Cron expression
```

### Webhook trigger

```yaml
trigger:
  webhook:
    provider: github
    secret: "${GITHUB_SECRET}"
```

## Execution model

Workflows execute as a directed graph:

1. Trigger fires → workflow starts
2. Entry nodes execute
3. Outputs flow to connected nodes
4. Parallel branches run concurrently
5. Workflow completes when all terminals finish

## State and context

Each execution has a context object:

```yaml
{{ trigger.body }}       # HTTP body
{{ trigger.query }}      # Query params
{{ trigger.headers }}    # Headers
{{ step1.output }}       # Output from node "step1"
{{ env.API_KEY }}        # Environment variable
```

## Error handling

Control error behavior per node:

```yaml
nodes:
  - name: "risky"
    type: "http/request"
    config:
      url: "https://api.example.com"
    on_error: continue      # continue | stop | retry
    retry:
      count: 3
      backoff: exponential
```

## Conditional execution

Run nodes conditionally:

```yaml
nodes:
  - name: "notify"
    type: "slack/post"
    if: "{{ fetch.status }} == 200"
```
