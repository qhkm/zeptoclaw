---
title: CLI Reference
description: Command-line interface for r8r
---

The `r8r` command-line tool provides everything you need to create, run, and manage workflows.

## Global options

```
Options:
  -c, --config <FILE>    Config file path [default: r8r.toml]
  -v, --verbose          Enable verbose logging
  -q, --quiet            Suppress non-error output
  -h, --help             Print help
  -V, --version          Print version
```

## Commands

### `r8r init`

Create a new workflow from a template.

```bash
r8r init <name> [options]

Options:
  -t, --template <NAME>   Template to use [default: http]
  -d, --dir <PATH>        Target directory
```

Examples:

```bash
# Create basic HTTP-triggered workflow
r8r init my-workflow

# Create scheduled workflow
r8r init daily-report --template schedule

# Create from custom template
r8r init custom --template ./templates/custom.yaml
```

### `r8r run`

Start the r8r server and execute workflows.

```bash
r8r run [options]

Options:
  -w, --workflows <DIR>   Workflows directory [default: ./]
  -p, --port <PORT>       HTTP port [default: 3000]
  -H, --host <HOST>       Bind address [default: 0.0.0.0]
```

Examples:

```bash
# Run with defaults
r8r run

# Custom port
r8r run --port 8080

# Specific workflows directory
r8r run --workflows ./my-flows
```

### `r8r trigger`

Manually trigger a workflow.

```bash
r8r trigger <workflow> [options]

Options:
  -d, --data <JSON>       Trigger data
  -f, --file <PATH>       Load data from file
```

Examples:

```bash
# Trigger with inline data
r8r trigger my-workflow --data '{"key": "value"}'

# Trigger from file
r8r trigger my-workflow --file ./payload.json
```

### `r8r validate`

Validate workflow files without running them.

```bash
r8r validate [options] [files...]

Options:
  --strict               Strict validation mode
```

Examples:

```bash
# Validate all workflows
r8r validate

# Validate specific file
r8r validate ./my-workflow.yaml
```

### `r8r build`

Build optimized release binary.

```bash
r8r build [options]

Options:
  --release              Release build (optimized)
  --target <TRIPLE>      Cross-compilation target
```

### `r8r deploy`

Deploy to cloud platforms.

```bash
r8r deploy [options]

Options:
  -p, --platform <NAME>   Target platform (fly, railway, render)
  --dry-run              Show what would be deployed
```

### `r8r logs`

View workflow execution logs.

```bash
r8r logs [options]

Options:
  -f, --follow           Follow log output
  -n, --lines <N>        Number of lines [default: 100]
  -w, --workflow <NAME>  Filter by workflow
```

### `r8r completions`

Generate shell completions.

```bash
r8r completions <SHELL>

Supported shells: bash, zsh, fish, powershell, elvish
```

Examples:

```bash
# Bash
r8r completions bash > /etc/bash_completion.d/r8r

# Zsh
r8r completions zsh > ~/.zsh/completions/_r8r
```
