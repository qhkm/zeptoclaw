---
title: Agent Loop
description: How ZeptoClaw processes messages and executes tools
---

The agent loop is the core of ZeptoClaw. It receives a message, builds context, calls an LLM, executes tool calls, and returns the response.

## Message flow

```
Message In
    │
    ▼
┌─────────────┐
│ Context     │  System prompt + conversation history + memory
│ Builder     │
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ Token       │  Check budget before calling LLM
│ Budget      │
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ LLM Call    │  Claude or OpenAI (with retry + fallback)
│             │
└──────┬──────┘
       │
       ▼
┌─────────────┐
│ Tool Exec   │  Approval gate → parallel execution → sanitize results
│             │
└──────┬──────┘
       │
       ▼ (loop back to LLM if more tool calls needed)
       │
    Response Out
```

## Context building

Before each LLM call, the context builder assembles:

1. **System prompt** — Base instructions plus any template overrides
2. **Conversation history** — Previous messages in the session
3. **Workspace memory** — Relevant markdown chunks from the workspace
4. **Tool definitions** — Available tools with parameter schemas

## Tool execution

When the LLM returns tool calls:

1. **Approval gate** — Checks if the tool requires approval based on configured policies
2. **Parallel execution** — Independent tool calls run concurrently via `futures::join_all`
3. **Result sanitization** — Strips base64 URIs, hex blobs, and truncates to 50KB
4. **Loop** — Results are sent back to the LLM for the next turn

The loop continues until the LLM returns a text response without tool calls, or the token budget is exhausted.

## Token budget

Each session can have a token budget that limits total token usage:

```json
{
  "agents": {
    "defaults": {
      "token_budget": 100000
    }
  }
}
```

The budget is tracked atomically using lock-free `AtomicU64` counters. When the budget is exhausted, the agent returns a message indicating the limit was reached.

## Streaming

When streaming is enabled (`--stream` flag or config), the agent loop uses SSE (Server-Sent Events) to deliver tokens in real-time:

```bash
zeptoclaw agent --stream -m "Explain monads"
```

Both Claude and OpenAI providers support streaming. Tool calls are still executed between streaming chunks.

## Timeouts

An agent-level timeout (default 300 seconds) wraps the entire message processing loop. This prevents runaway agent sessions from consuming resources indefinitely.

Configure via:
```bash
export ZEPTOCLAW_AGENTS_DEFAULTS_AGENT_TIMEOUT_SECS=600
```

## Hooks

The hook system provides three extension points:

- **before_tool** — Runs before each tool execution
- **after_tool** — Runs after each tool execution
- **on_error** — Runs when a tool fails

Hook actions include `Log`, `Metric`, and `Notify` (sends a message to a channel via the MessageBus).
