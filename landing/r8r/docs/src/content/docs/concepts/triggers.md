---
title: Triggers
description: Events that start workflows
---

Triggers define when and how a workflow executes.

## HTTP trigger

Start workflows via HTTP requests:

```yaml
trigger:
  http:
    path: /api/users          # URL path
    method: POST              # GET | POST | PUT | DELETE
    auth:
      type: bearer            # none | basic | bearer | api_key
    validate:
      body:                   # JSON Schema validation
        type: object
        required: ["email"]
```

Access trigger data:

```yaml
{{ trigger.body }}           # Request body
{{ trigger.query }}          # Query parameters
{{ trigger.headers }}        # Headers
{{ trigger.params }}         # URL path params
```

## Schedule trigger

Cron-based execution:

```yaml
trigger:
  schedule: "0 9 * * MON"     # Every Monday at 9 AM
```

Cron syntax:

```
* * * * *
│ │ │ │ └─── Day of week (0-7, 0=Sunday)
│ │ │ └───── Month (1-12)
│ │ └─────── Day of month (1-31)
│ └───────── Hour (0-23)
└─────────── Minute (0-59)
```

Common patterns:

| Expression | Meaning |
|------------|---------|
| `*/5 * * * *` | Every 5 minutes |
| `0 * * * *` | Every hour |
| `0 0 * * *` | Daily at midnight |
| `0 9 * * MON` | Weekly on Monday |

## Webhook trigger

Receive external webhooks:

```yaml
trigger:
  webhook:
    provider: github
    secret: "${WEBHOOK_SECRET}"
    events:
      - push
      - pull_request
```

## Queue trigger

Process messages from queues:

```yaml
trigger:
  queue:
    type: kafka
    brokers:
      - localhost:9092
    topic: events
    group: r8r-consumers
```

## Manual trigger

Start via CLI or API:

```yaml
trigger:
  manual: {}
```

```bash
r8r trigger my-workflow --data '{"key": "value"}'
```

## Multiple triggers

A workflow can have multiple triggers:

```yaml
triggers:
  - http:
      path: /api/users
  - schedule: "0 9 * * *"
  - webhook:
      provider: stripe
```
