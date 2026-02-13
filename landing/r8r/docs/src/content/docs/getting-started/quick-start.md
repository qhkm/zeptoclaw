---
title: Quick Start
description: Build your first workflow in under 5 minutes
---

Let's create a simple workflow that fetches data from an API and sends a notification. This will take under 5 minutes.

## 1. Initialize a new workflow

Create a new directory and initialize your first workflow:

```bash
mkdir my-workflows && cd my-workflows
r8r init hello-world
```

This creates a `hello-world.yaml` file with a basic structure:

```yaml
name: "hello-world"
trigger:
  http:
    path: /webhook
    method: POST

nodes:
  - name: "echo"
    type: "core/log"
    config:
      message: "Hello, World!"
```

## 2. Run the workflow

Start the r8r server:

```bash
r8r run
```

You should see:

```
🚀 r8r server running on http://localhost:3000
📁 Loaded 1 workflow from ./
⚡ Press Ctrl+C to stop
```

## 3. Test it

Trigger the workflow with curl:

```bash
curl -X POST http://localhost:3000/webhook \
  -H "Content-Type: application/json" \
  -d '{"name": "r8r"}'
```

Check the logs — you'll see the echo node printed "Hello, World!".

## 4. Make it dynamic

Edit `hello-world.yaml` to use the input data:

```yaml
name: "hello-world"
trigger:
  http:
    path: /webhook
    method: POST

nodes:
  - name: "greet"
    type: "core/log"
    config:
      message: "Hello, {{ trigger.body.name | default: 'World' }}!"
```

Restart r8r (`Ctrl+C`, then `r8r run`) and test again:

```bash
curl -X POST http://localhost:3000/webhook \
  -H "Content-Type: application/json" \
  -d '{"name": "Alice"}'
```

Now it prints: `Hello, Alice!`

## 5. Add more nodes

Let's fetch data from an API and process it:

```yaml
name: "weather-check"
trigger:
  schedule: "0 */6 * * *"  # Every 6 hours

nodes:
  - name: "fetch_weather"
    type: "http/request"
    config:
      url: "https://api.open-meteo.com/v1/forecast"
      method: GET
      query:
        latitude: 51.5074
        longitude: -0.1278
        current_weather: true

  - name: "check_temperature"
    type: "core/condition"
    config:
      condition: "{{ fetch_weather.body.current_weather.temperature }} > 25"
      then:
        - name: "notify_hot"
          type: "slack/post"
          config:
            channel: "#weather"
            message: "🔥 It's hot! {{ fetch_weather.body.current_weather.temperature }}°C"
      else:
        - name: "notify_normal"
          type: "core/log"
          config:
            message: "Temperature is normal"
```

## 6. Deploy it

When you're ready to deploy:

```bash
# Build optimized binary
r8r build --release

# Or deploy to cloud
r8r deploy --platform=fly
```

## What's next?

- Learn about [workflows](/concepts/workflows/) and how they're structured
- Explore [built-in node types](/reference/node-types/)
- Build [custom nodes](/guides/custom-nodes/) in Rust
