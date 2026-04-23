# ZeptoClaw Utility/Lib Crate Evaluation (Issue #389)

**Date:** 2026-04-22  
**Status:** Completed  
**Candidate:** https://github.com/0xPlaygrounds/rig

## Goal

Evaluate whether ZeptoClaw should build core agent functionality on top of a utility/lib crate abstraction.

## Evaluation Criteria

1. Fit with ZeptoClaw's current architecture (provider stack, tool runtime, channels, safety pipeline)
2. Migration risk and blast radius
3. Feature parity requirements (streaming/tool-calls/fallback/retry/config hot-reload)
4. Operational constraints (security posture, deterministic behavior, small binary goals)
5. Long-term maintenance cost

## Findings

### 1. Architectural Fit

- ZeptoClaw already has deep integration points across `agent/`, `kernel/`, `tools/`, `channels/`, and `providers/`.
- The runtime is not a thin model SDK wrapper; it includes policy/safety enforcement, channel lifecycle supervision, plugin/MCP wiring, and compatibility shims.
- Adopting an external utility/lib core would require adapter layers across most of the current execution path.

**Result:** Partial fit, but high adaptation overhead.

### 2. Migration Risk

- A full migration would touch cross-cutting behavior (tool execution path, telemetry, approval gates, provider wrappers).
- Risk of regressions is high in streaming parity, tool-call accounting, and config compatibility behavior.
- The migration does not map to a single subsystem; it is multi-module and behavior-sensitive.

**Result:** High risk for limited near-term gain.

### 3. Feature Parity

- ZeptoClaw includes bespoke capabilities (multi-channel gateway flow, per-tool safety checks, fallback/retry provider composition, runtime isolation integrations).
- Equivalent parity on top of a generic utility/lib would still require substantial Zepto-specific code.

**Result:** Utility/lib would not eliminate most Zepto-specific implementation complexity.

### 4. Operational/Security Constraints

- Current guardrails are integrated at runtime boundaries (filesystem/shell/network checks, tool approval policy, taint/safety flows).
- Re-platforming core execution increases the chance of subtle security and policy bypass gaps during transition.

**Result:** Security validation burden is significant.

### 5. Maintenance Tradeoff

- Potential upside: some lower-level abstractions could be reused from an external crate.
- Downside: lock-in to another project's roadmap and compatibility surface.
- Current architecture already supports incremental modularization without hard dependency on a new core.

**Result:** Incremental in-repo refactors provide a better risk/reward path.

## Decision

**Do not adopt a utility/lib crate as ZeptoClaw's core runtime foundation at this stage.**

## Recommended Path

1. Keep ZeptoClaw core architecture in-repo.
2. Continue targeted extraction of reusable pieces into internal modules.
3. Revisit external utility/lib adoption only if:
   - Zepto runtime boundaries are already simplified, and
   - an external crate can satisfy streaming/tool/safety parity with low adapter cost.
