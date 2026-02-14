---
title: Plugins
description: Extend ZeptoClaw with custom tools using JSON manifest plugins
---

The plugin system lets you add custom tools to your agent without modifying ZeptoClaw's source code. Plugins are JSON manifest files that define a tool's name, parameters, and a command template.

## Plugin structure

Create a JSON file in `~/.zeptoclaw/plugins/`:

```json
{
  "name": "github_pr",
  "description": "Create a GitHub pull request",
  "version": "1.0.0",
  "parameters": {
    "type": "object",
    "properties": {
      "title": {
        "type": "string",
        "description": "PR title"
      },
      "branch": {
        "type": "string",
        "description": "Source branch"
      }
    },
    "required": ["title", "branch"]
  },
  "command": "gh pr create --title {{title}} --head {{branch}} --body 'Created by ZeptoClaw'"
}
```

## How plugins work

1. **Discovery** — At startup, ZeptoClaw scans configured plugin directories for JSON files
2. **Validation** — Each manifest is validated for required fields and valid JSON schema
3. **Registration** — Valid plugins are wrapped in a `PluginTool` adapter and registered in the tool registry
4. **Execution** — When the agent calls the tool, parameters are interpolated into the command template and executed via shell

## Plugin manifest fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Tool name (used by the LLM) |
| `description` | string | Yes | Tool description (shown to the LLM) |
| `version` | string | No | Plugin version |
| `parameters` | object | Yes | JSON Schema for tool parameters |
| `command` | string | Yes | Command template with `{{param}}` placeholders |

## Parameter interpolation

Parameter values are substituted into the command template using `{{param_name}}` syntax. All values are automatically shell-escaped to prevent command injection.

```json
{
  "command": "curl -X POST {{url}} -d {{data}}"
}
```

If `url` is `https://api.example.com` and `data` is `{"key": "value"}`, the executed command becomes:

```bash
curl -X POST 'https://api.example.com' -d '{"key": "value"}'
```

## Configuration

Enable plugins in your config:

```json
{
  "plugins": {
    "enabled": true,
    "directories": ["~/.zeptoclaw/plugins", "/opt/zeptoclaw/plugins"]
  }
}
```

## Example: Slack notifier

```json
{
  "name": "slack_notify",
  "description": "Send a notification to a Slack channel",
  "parameters": {
    "type": "object",
    "properties": {
      "channel": {
        "type": "string",
        "description": "Slack channel name"
      },
      "text": {
        "type": "string",
        "description": "Message text"
      }
    },
    "required": ["channel", "text"]
  },
  "command": "curl -X POST https://hooks.slack.com/services/YOUR/WEBHOOK/URL -H 'Content-Type: application/json' -d '{\"channel\": {{channel}}, \"text\": {{text}}}'"
}
```

## Security

- Parameter values are shell-escaped (wrapped in single quotes with proper escaping)
- Commands run through the same shell blocklist as the `shell` tool
- Container isolation applies to plugin commands when enabled
- The approval gate can require approval for specific plugin tools
