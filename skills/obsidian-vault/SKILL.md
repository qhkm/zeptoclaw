---
name: obsidian-vault
description: Manage Obsidian vaults using built-in filesystem tools. Understands vault structure, frontmatter, wikilinks, tags, tasks, daily notes, and templates.
metadata: {"zeptoclaw":{"emoji":"📚"}}
---

# Obsidian Vault Skill

Manage Obsidian vaults directly via filesystem tools. No external CLI required — uses built-in read, write, list, and edit tools to work with vault files.

## Vault Structure

An Obsidian vault is a directory containing markdown files and a `.obsidian/` config folder:

```
MyVault/
├── .obsidian/           # Obsidian config (DO NOT modify unless asked)
│   ├── app.json         # App settings
│   ├── appearance.json  # Theme settings
│   ├── community-plugins.json  # Installed plugin IDs
│   └── plugins/         # Plugin configs
├── Daily Notes/         # Common daily notes folder
├── Templates/           # Common templates folder
├── Attachments/         # Images, PDFs, etc.
├── Notes/               # User's notes
└── *.md                 # Root-level notes
```

**Identify a vault:** A directory is a vault if it contains `.obsidian/` subdirectory.

## Frontmatter (Properties)

Notes use YAML frontmatter between `---` delimiters at the top of the file:

```markdown
---
title: Meeting Notes
date: 2026-02-26
tags:
  - meeting
  - project-alpha
status: draft
aliases:
  - Alpha Meeting
---

# Meeting Notes

Content starts here...
```

### Read frontmatter

Read the file and parse everything between the first `---` pair.

### Update frontmatter

Use the edit tool to replace frontmatter fields. Preserve existing fields — only change what's requested.

### Add frontmatter to a file without one

Prepend `---\n<fields>\n---\n\n` before existing content.

## Tags

Tags appear in two places:

1. **Frontmatter tags** — `tags: [meeting, project]` in YAML
2. **Inline tags** — `#tag-name` anywhere in the body text (NOT inside code blocks)

Nested tags use `/`: `#project/alpha`, `#status/in-progress`

### Find all tags in vault

Search for `#[\w/-]+` pattern across all `.md` files, plus parse `tags:` from frontmatter.

### Find files with a specific tag

Search for both `#tag-name` in body text AND `tag-name` in frontmatter `tags` arrays.

## Wikilinks

Obsidian links use double-bracket syntax:

| Syntax | Meaning |
|--------|---------|
| `[[Note Name]]` | Link to note (matches filename without `.md`) |
| `[[Note Name\|Display Text]]` | Link with custom display text |
| `[[Note Name#Heading]]` | Link to specific heading |
| `[[Note Name#^block-id]]` | Link to specific block |
| `![[Note Name]]` | Embed (transclude) entire note |
| `![[image.png]]` | Embed image from vault |

### Find backlinks

To find all files linking TO `MyNote.md`, search all `.md` files for `[[MyNote]]` or `[[MyNote|` or `[[MyNote#`.

### Find broken links

Extract all `[[target]]` references, then check if `target.md` exists in the vault.

## Tasks

Obsidian uses markdown checkboxes with optional metadata:

```markdown
- [ ] Incomplete task
- [x] Completed task
- [ ] Task with due date [due:: 2026-03-01]
- [ ] Task with priority [priority:: high]
- [ ] #project/alpha Review the design doc
```

### Find all tasks

Search for lines matching `- \[[ x]\] ` across all `.md` files.

### Find incomplete tasks

Search for `- \[ \] ` (with space between brackets).

### Complete a task

Edit the file, replacing `- [ ]` with `- [x]` on the target line.

### Find overdue tasks

Search for `\[due:: (\d{4}-\d{2}-\d{2})\]` and compare dates.

## Daily Notes

Daily notes follow a date-based naming convention. Common patterns:

| Pattern | Example Path |
|---------|-------------|
| `Daily Notes/YYYY-MM-DD.md` | `Daily Notes/2026-02-26.md` |
| `Journal/YYYY/MM-DD.md` | `Journal/2026/02-26.md` |
| `YYYY-MM-DD.md` (root) | `2026-02-26.md` |

### Detect daily notes folder

Check `.obsidian/daily-notes.json` for `folder` and `format` settings. If not configured, check for common folder names: `Daily Notes`, `Journal`, `Dailies`.

### Create today's daily note

```markdown
---
date: 2026-02-26
tags:
  - daily
---

# 2026-02-26

## Tasks

- [ ]

## Notes

```

### List recent daily notes

List files in the daily notes folder, sort by filename (date) descending.

## Templates

Templates live in a configurable folder (check `.obsidian/templates.json` for `folder`). Common locations: `Templates/`, `_templates/`.

### Apply a template

1. Read the template file
2. Replace template variables:
   - `{{date}}` or `{{date:YYYY-MM-DD}}` → current date
   - `{{time}}` or `{{time:HH:mm}}` → current time
   - `{{title}}` → note filename without `.md`
3. Write as the new note content

## Search

### Full-text search

Use the shell tool with grep/ripgrep to search vault content:

```bash
# Search all markdown files for a term
grep -rl "search term" /path/to/vault --include="*.md"

# Search with context
grep -rn "search term" /path/to/vault --include="*.md" -C 2
```

Or use the filesystem list tool to enumerate files, then read and filter.

### Search by property

Parse frontmatter from each file and filter by property values.

## Common Operations

### Create a new note

Write a new `.md` file with frontmatter:

```markdown
---
title: Note Title
date: 2026-02-26
tags: []
---

# Note Title

Content here.
```

### Move/rename a note

1. Read the old file
2. Write to the new path
3. Delete the old file
4. **Update backlinks**: Search all `.md` files for `[[OldName]]` and replace with `[[NewName]]`

### List all notes

List all `*.md` files in the vault, excluding `.obsidian/` and `.trash/`.

### Vault statistics

- Count `.md` files (total notes)
- Extract and count unique tags
- Count `[[` references (total links)
- Count `- [ ]` lines (open tasks)
- Count `- [x]` lines (completed tasks)

## Obsidian Config Files

These live in `.obsidian/` — read but don't modify unless explicitly asked:

| File | Contains |
|------|----------|
| `app.json` | Editor settings, vim mode, etc. |
| `appearance.json` | Theme, font, CSS snippet list |
| `community-plugins.json` | Array of installed plugin IDs |
| `daily-notes.json` | `folder`, `format`, `template` for daily notes |
| `templates.json` | `folder` for templates |
| `bookmarks.json` | Bookmarked files and searches |
| `graph.json` | Graph view settings |
| `hotkeys.json` | Custom keyboard shortcuts |

## Tips

- Always preserve existing frontmatter fields when editing — only change what's asked
- Wikilinks are case-insensitive for matching but preserve original case
- Files in `.trash/` are soft-deleted — check there before declaring something missing
- Attachments (images, PDFs) are typically in `Attachments/` or alongside the note
- Obsidian ignores dotfiles and `node_modules/` — don't create notes there
- When creating notes, match the vault's existing naming convention (kebab-case, Title Case, etc.)
- Canvas files (`.canvas`) are JSON — read with filesystem tools, parse as JSON
