---
title: Connections
description: How data flows between nodes
---

Connections define how data flows from one node to another in a workflow.

## Implicit connections

By default, nodes execute sequentially:

```yaml
nodes:
  - name: "step1"
    type: "http/request"
  - name: "step2"    # Runs after step1 completes
    type: "core/log"
```

## Explicit connections

Define explicit data flow:

```yaml
nodes:
  - name: "fetch_user"
    type: "http/request"
    config:
      url: "https://api.example.com/users/{{ trigger.body.user_id }}"

  - name: "fetch_orders"
    type: "http/request"
    config:
      url: "https://api.example.com/users/{{ fetch_user.body.id }}/orders"

  - name: "send_email"
    type: "email/send"
    input:
      to: "{{ fetch_user.body.email }}"
      orders: "{{ fetch_orders.body }}"
```

## Parallel execution

Nodes without dependencies run in parallel:

```yaml
nodes:
  - name: "fetch_a"    # Starts immediately
    type: "http/request"
  - name: "fetch_b"    # Also starts immediately
    type: "http/request"
  - name: "combine"    # Waits for both
    type: "core/template"
    input:
      a: "{{ fetch_a.body }}"
      b: "{{ fetch_b.body }}"
```

## Conditional connections

```yaml
nodes:
  - name: "check"
    type: "http/request"

  - name: "success_path"
    type: "core/log"
    if: "{{ check.status }} == 200"

  - name: "error_path"
    type: "slack/post"
    if: "{{ check.status }} >= 400"
```

## Templating syntax

Use [Tera](https://keats.github.io/tera/) templates:

```yaml
input:
  # Variable access
  name: "{{ user.name }}"
  
  # Filters
  upper: "{{ user.name | upper }}"
  default: "{{ user.nickname | default: 'Anonymous' }}"
  
  # Conditionals
  status: "{% if score > 50 %}pass{% else %}fail{% endif %}"
```

## Error propagation

Failed nodes stop downstream execution unless configured otherwise:

```yaml
nodes:
  - name: "optional_check"
    type: "http/request"
    on_error: ignore  # Continue even if this fails

  - name: "always_runs"
    type: "core/log"  # Runs regardless
```
