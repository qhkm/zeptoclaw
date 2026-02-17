---
name: skill-creator
description: Create or update ZeptoClaw skills. Use when designing, structuring, or authoring new agent skills.
metadata: {"zeptoclaw":{"emoji":"🛠️","requires":{}}}
---

# Skill Creator

Guide users through designing and building ZeptoClaw skills. Follow these six steps in order — do not skip ahead.

## Step 1 — Gather Concrete Examples

Before designing anything, ask the user for 3-5 real queries or tasks the skill should handle. Collect actual example inputs and expected behaviors. Do not proceed until you have concrete use cases — vague ideas produce vague skills.

## Step 2 — Analyze Reusable Content

For each example, ask: "What would need to be looked up, computed, or re-done each time without this skill?" The answers become the skill's content. Discard anything the agent already knows how to do without help.

Categories of reusable content:
- **Commands/scripts** — exact CLI invocations, API calls, shell pipelines
- **References** — URLs, lookup tables, domain-specific facts the agent wouldn't know
- **Templates** — message formats, boilerplate, structured output patterns
- **Workflows** — multi-step procedures with specific ordering constraints

If nothing survives this filter, the user may not need a skill — say so.

## Step 3 — Choose Structure and Freedom Level

Pick the structural pattern that best fits the content:

| Pattern | When to use | Example |
|---------|-------------|---------|
| **Workflow** | Ordered multi-step process | deploy-docker, release-notes |
| **Task-based** | Independent tasks, any order | shopee, github |
| **Reference** | Lookup information | weather, api-cheatsheet |
| **Capabilities** | Agent behaviors/personas | code-reviewer, translator |

Then assign a freedom level to each piece of content:

- **High freedom** (text instructions): Multiple valid approaches exist. Let the agent decide how to execute. Use for guidelines, principles, and flexible tasks.
- **Medium freedom** (pseudocode or parameterized commands): A preferred pattern exists but details vary. Use for common workflows with variable inputs.
- **Low freedom** (exact commands/scripts): Fragile or error-prone operations where precision matters. Use for API calls with specific syntax, deployment commands, or data formats.

Match specificity to fragility — only lock down what breaks when done differently.

## Step 4 — Scaffold the Skill

Create the skill directory and file:

```bash
zeptoclaw skills create <skill-name>
```

This creates `~/.zeptoclaw/skills/<skill-name>/SKILL.md` with a starter template.

If the skill needs bundled resources, create subdirectories:
- `scripts/` — executable code the agent runs (deterministic, token-efficient)
- `references/` — documentation loaded into context on demand
- `assets/` — output files (templates, icons) never loaded into context

**Naming rules:** Lowercase letters, digits, and hyphens only. Max 64 characters. No leading, trailing, or consecutive hyphens. Prefer verb-led names: `deploy-docker`, `track-expenses`, `format-invoice`.

## Step 5 — Write the Skill Content

### Frontmatter

Every SKILL.md starts with YAML frontmatter:

```yaml
---
name: skill-name
description: One-line description of what this skill does.
metadata: {"zeptoclaw":{"emoji":"📦","requires":{"bins":["curl"],"any_bins":[],"env":[]},"install":[],"always":false,"os":[]}}
---
```

**ZeptoClaw metadata fields:**
- `emoji` — Display emoji for skill listings
- `requires.bins` — All listed binaries must exist in PATH
- `requires.any_bins` — At least one listed binary must exist
- `requires.env` — All listed environment variables must be set
- `install` — Installation hints: `[{"id":"...", "kind":"brew|apt|cargo|npm|pip", "formula":"...", "bins":["..."], "label":"..."}]`
- `always` — If `true`, skill content is always injected into agent context
- `os` — Platform filter: `["darwin", "linux", "win32"]`. Empty means all platforms.

Only include fields you need. Minimal example:

```yaml
metadata: {"zeptoclaw":{"emoji":"🔧","requires":{}}}
```

### Body

Apply progressive disclosure — three levels of loading:

1. **Metadata** (frontmatter name + description): Always visible in skill listings. Keep under 100 words total. This is the skill's "elevator pitch."
2. **SKILL.md body**: Loaded when the skill is triggered. Target under 2000 words. This is the skill's core — instructions, commands, examples, templates.
3. **Bundled resources** (scripts/references/assets): Loaded on demand when the agent needs them. No size limit. Use for detailed reference docs, large templates, or helper scripts.

Push detail down. If content is only needed for specific subtasks, put it in a `references/` file and instruct the agent to read it when relevant.

### What NOT to include

- Information the agent already knows (general programming, common tools)
- README, CHANGELOG, or human-facing documentation
- Multiple examples illustrating the same point — one clear example is enough
- Generic advice like "be concise" or "handle errors" — the agent knows

Every paragraph must justify its token cost. If removing it wouldn't change the agent's behavior, remove it.

## Step 6 — Validate and Iterate

Check the skill:
1. `SKILL.md` exists with valid YAML frontmatter
2. `name` is lowercase-hyphenated, max 64 characters
3. `description` exists and is a non-empty string
4. Body contains actionable instructions (not just placeholders)
5. Run `zeptoclaw skills show <name>` to verify it loads correctly
6. Run `zeptoclaw skills list` to confirm it appears

Then test it: replay the example queries from Step 1 against the agent with the skill loaded. Observe where the agent struggles or produces wrong output. Update the skill content and repeat.

Skills improve through use. Ship a working first version, then refine based on real failures.
