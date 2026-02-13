---
title: Node Types
description: Built-in node types reference
---

Reference for all built-in node types in r8r.

## Core nodes

### core/log

Log a message to stdout.

```yaml
- name: "log"
  type: "core/log"
  config:
    message: "Hello, {{ user.name }}!"
    level: info  # trace | debug | info | warn | error
```

### core/condition

Branch execution based on a condition.

```yaml
- name: "branch"
  type: "core/condition"
  config:
    condition: "{{ status }} == 200"
    then:
      - name: "success"
        type: "core/log"
    else:
      - name: "error"
        type: "core/log"
```

### core/sleep

Pause execution.

```yaml
- name: "wait"
  type: "core/sleep"
  config:
    duration: "5s"  # 5s, 1m, 1h, etc.
```

### core/template

Transform data using templates.

```yaml
- name: "format"
  type: "core/template"
  config:
    template: |
      {
        "user": "{{ input.name | upper }}",
        "timestamp": "{{ now() }}"
      }
```

## HTTP nodes

### http/request

Make HTTP requests.

```yaml
- name: "api"
  type: "http/request"
  config:
    url: "https://api.example.com/users"
    method: POST
    headers:
      Authorization: "Bearer {{ env.API_KEY }}"
    body:
      name: "{{ input.name }}"
    timeout: "10s"
    retry:
      count: 3
      delay: "1s"
```

Output:

```json
{
  "status": 200,
  "headers": {...},
  "body": {...}
}
```

## Database nodes

### postgres/query

Execute PostgreSQL queries.

```yaml
- name: "users"
  type: "postgres/query"
  config:
    sql: "SELECT * FROM users WHERE id = $1"
    params:
      - "{{ input.user_id }}"
```

### redis/get

Read from Redis.

```yaml
- name: "cache"
  type: "redis/get"
  config:
    key: "user:{{ input.id }}"
```

### redis/set

Write to Redis.

```yaml
- name: "set_cache"
  type: "redis/set"
  config:
    key: "user:{{ input.id }}"
    value: "{{ input.data }}"
    ttl: 3600  # seconds
```

## Message nodes

### slack/post

Send Slack messages.

```yaml
- name: "notify"
  type: "slack/post"
  config:
    channel: "#alerts"
    text: "⚠️ Error occurred: {{ error.message }}"
    blocks: [...]  # Rich formatting
```

### kafka/produce

Publish to Kafka.

```yaml
- name: "publish"
  type: "kafka/produce"
  config:
    topic: "events"
    key: "{{ event.user_id }}"
    value: "{{ event | to_json }}"
```
