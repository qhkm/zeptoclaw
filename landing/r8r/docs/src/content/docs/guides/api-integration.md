---
title: API Integration
description: Connect to external APIs
---

Integrate with external APIs using r8r's HTTP nodes and authentication features.

## Basic API call

```yaml
nodes:
  - name: "fetch_users"
    type: "http/request"
    config:
      url: "https://api.example.com/users"
      method: GET
```

## Authentication

### Bearer token

```yaml
nodes:
  - name: "secure_api"
    type: "http/request"
    config:
      url: "https://api.example.com/data"
      headers:
        Authorization: "Bearer {{ env.API_TOKEN }}"
```

### API key

```yaml
nodes:
  - name: "api_call"
    type: "http/request"
    config:
      url: "https://api.example.com/data"
      query:
        api_key: "{{ env.API_KEY }}"
```

### Basic auth

```yaml
nodes:
  - name: "legacy_api"
    type: "http/request"
    config:
      url: "https://api.example.com/data"
      auth:
        type: basic
        username: "{{ env.API_USER }}"
        password: "{{ env.API_PASS }}"
```

## Dynamic URLs

```yaml
nodes:
  - name: "get_user"
    type: "http/request"
    config:
      url: "https://api.example.com/users/{{ trigger.body.user_id }}"
```

## POST requests with JSON

```yaml
nodes:
  - name: "create_user"
    type: "http/request"
    config:
      url: "https://api.example.com/users"
      method: POST
      headers:
        Content-Type: "application/json"
      body:
        name: "{{ trigger.body.name }}"
        email: "{{ trigger.body.email }}"
```

## Handling responses

```yaml
nodes:
  - name: "api"
    type: "http/request"
    config:
      url: "https://api.example.com/data"

  - name: "check_status"
    type: "core/condition"
    config:
      condition: "{{ api.status }} == 200"
      then:
        - name: "process"
          type: "core/log"
          config:
            message: "Data: {{ api.body }}"
      else:
        - name: "error"
          type: "core/log"
          config:
            message: "Failed: {{ api.body.error }}"
```

## Pagination

Handle paginated APIs:

```yaml
nodes:
  - name: "fetch_page"
    type: "http/request"
    config:
      url: "https://api.example.com/items"
      query:
        page: "{{ trigger.query.page | default: 1 }}"
        per_page: "100"

  - name: "has_more"
    type: "core/condition"
    config:
      condition: "{{ fetch_page.body.has_more }}"
      then:
        - name: "trigger_next"
          type: "http/request"
          config:
            url: "https://api.example.com/items"
            query:
              page: "{{ trigger.query.page | int | add: 1 }}"
```

## Rate limiting

Respect API rate limits:

```yaml
nodes:
  - name: "api_call"
    type: "http/request"
    config:
      url: "https://api.example.com/data"
      retry:
        count: 5
        delay: "2s"
        on_status: [429, 503]  # Too Many Requests, Service Unavailable
```

## Webhook verification

Verify incoming webhooks:

```yaml
trigger:
  webhook:
    provider: github
    secret: "${WEBHOOK_SECRET}"
    verify_signature: true
```

## Error handling

```yaml
nodes:
  - name: "api"
    type: "http/request"
    config:
      url: "https://api.example.com/data"
    on_error:
      action: continue
      output:
        error: true
        message: "API unavailable"

  - name: "fallback"
    type: "core/log"
    if: "{{ api.error }}"
    config:
      message: "Using cached data"
```
