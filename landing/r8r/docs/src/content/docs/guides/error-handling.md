---
title: Error Handling
description: Handle failures gracefully
---

Build resilient workflows with proper error handling.

## Default behavior

By default, a failed node stops the workflow:

```yaml
nodes:
  - name: "api"        # If this fails...
    type: "http/request"
    
  - name: "process"    # ...this never runs
    type: "core/log"
```

## Continue on error

Process continues even if the node fails:

```yaml
nodes:
  - name: "api"
    type: "http/request"
    on_error: continue   # Workflow continues

  - name: "process"      # This still runs
    type: "core/log"
```

Access error info:

```yaml
{{ api.error }}         # true if failed
{{ api.error_message }} # Error description
```

## Ignore error

Completely ignore the failure:

```yaml
nodes:
  - name: "optional"
    type: "http/request"
    on_error: ignore    # No error output, continues silently
```

## Retry with backoff

```yaml
nodes:
  - name: "api"
    type: "http/request"
    on_error:
      action: retry
      max_attempts: 5
      delay: "exponential"  # 1s, 2s, 4s, 8s...
      max_delay: "30s"
```

## Conditional error handling

```yaml
nodes:
  - name: "api"
    type: "http/request"
    on_error:
      action: continue
      
  - name: "notify_error"
    type: "slack/post"
    if: "{{ api.error }}"
    config:
      channel: "#alerts"
      message: "API failed: {{ api.error_message }}"

  - name: "process_success"
    type: "core/log"
    if: "{{ not api.error }}"
    config:
      message: "Success: {{ api.body }}"
```

## Timeout handling

```yaml
nodes:
  - name: "slow_api"
    type: "http/request"
    config:
      url: "https://slow.example.com"
      timeout: "5s"        # Fail if > 5 seconds
    on_error:
      action: continue
      
  - name: "fallback"
    type: "core/log"
    if: "{{ slow_api.error }}"
    config:
      message: "Using cached data"
```

## Circuit breaker

Prevent cascading failures:

```yaml
nodes:
  - name: "fragile_api"
    type: "http/request"
    config:
      url: "https://unreliable.example.com"
    circuit_breaker:
      failure_threshold: 5
      recovery_timeout: "30s"
      half_open_max_calls: 3
```

## Global error handler

Define a workflow-wide error handler:

```yaml
name: "my-workflow"

on_error:
  - name: "log_error"
    type: "core/log"
    config:
      message: "Workflow failed: {{ error.message }}"
  - name: "notify"
    type: "slack/post"
    config:
      channel: "#errors"
      message: "🚨 Workflow {{ workflow.name }} failed"

nodes:
  - name: "step1"
    type: "http/request"
```

## Validation errors

Validate inputs before processing:

```yaml
trigger:
  http:
    path: /api/users
    validate:
      body:
        type: object
        required: ["email"]
        properties:
          email:
            type: string
            format: email
          age:
            type: integer
            minimum: 0
```

Invalid requests return 400 without starting the workflow.
