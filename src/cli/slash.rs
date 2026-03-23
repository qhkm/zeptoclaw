//! Slash command registry, completer, and dispatch for CLI interactive mode.

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::Helper;

/// A slash command available in the CLI interactive mode.
#[derive(Debug, Clone)]
pub struct SlashCommand {
    /// Command name without the leading `/` (e.g. "model", "model list").
    pub name: &'static str,
    /// Short description shown in completions and `/help`.
    pub description: &'static str,
}

/// Returns the built-in slash commands.
pub fn builtin_commands() -> Vec<SlashCommand> {
    vec![
        SlashCommand {
            name: "model",
            description: "Show or switch LLM model",
        },
        SlashCommand {
            name: "model list",
            description: "Show available models",
        },
        SlashCommand {
            name: "model fetch",
            description: "Fetch live models from providers",
        },
        SlashCommand {
            name: "persona",
            description: "Show or switch persona",
        },
        SlashCommand {
            name: "persona list",
            description: "Show persona presets",
        },
        SlashCommand {
            name: "help",
            description: "Show available commands",
        },
        SlashCommand {
            name: "tools",
            description: "List available agent tools",
        },
        SlashCommand {
            name: "memory",
            description: "Show memory command hints",
        },
        SlashCommand {
            name: "history",
            description: "Show history command hints",
        },
        SlashCommand {
            name: "template",
            description: "List available templates",
        },
        SlashCommand {
            name: "template list",
            description: "Show available templates",
        },
        SlashCommand {
            name: "trust",
            description: "Show local trusted-session status",
        },
        SlashCommand {
            name: "trust on",
            description: "Bypass approvals for this interactive CLI session",
        },
        SlashCommand {
            name: "trust off",
            description: "Disable trusted-session bypass",
        },
        SlashCommand {
            name: "clear",
            description: "Clear conversation context",
        },
        SlashCommand {
            name: "quit",
            description: "Exit interactive mode",
        },
    ]
}

/// Rustyline helper that provides tab-completion for `/` commands.
pub struct SlashHelper {
    commands: Vec<SlashCommand>,
}

impl SlashHelper {
    pub fn new() -> Self {
        Self {
            commands: builtin_commands(),
        }
    }
}

impl Completer for SlashHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        // Only complete if input starts with '/' and cursor is past it
        if !line.starts_with('/') || pos < 1 {
            return Ok((0, vec![]));
        }

        // Strip the leading '/' for matching
        let query = line.get(1..pos).unwrap_or("");

        let mut matches: Vec<Pair> = self
            .commands
            .iter()
            .filter(|cmd| cmd.name.starts_with(query))
            .map(|cmd| Pair {
                display: format!("/{:<20} {}", cmd.name, cmd.description),
                replacement: format!("/{}", cmd.name),
            })
            .collect();

        // Sort by name length (shorter = more relevant)
        matches.sort_by_key(|p| p.replacement.len());

        // Start replacement from position 0 (replace entire input)
        Ok((0, matches))
    }
}

impl Hinter for SlashHelper {
    type Hint = String;
}

impl Highlighter for SlashHelper {}

impl Validator for SlashHelper {}

impl Helper for SlashHelper {}

/// Format the `/help` output listing all commands.
pub fn format_help() -> String {
    let commands = builtin_commands();
    let mut out = String::from("Available commands:\n\n");
    for cmd in &commands {
        // Skip subcommands in help listing (they show under parent)
        if cmd.name.contains(' ') {
            continue;
        }
        out.push_str(&format!("  /{:<16} {}\n", cmd.name, cmd.description));
    }
    out.push_str("\nType '/' and press Tab to autocomplete.");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_commands_not_empty() {
        let cmds = builtin_commands();
        assert!(!cmds.is_empty());
    }

    #[test]
    fn test_builtin_commands_no_leading_slash() {
        for cmd in builtin_commands() {
            assert!(
                !cmd.name.starts_with('/'),
                "Command name should not start with /: {}",
                cmd.name
            );
        }
    }

    #[test]
    fn test_builtin_commands_have_descriptions() {
        for cmd in builtin_commands() {
            assert!(
                !cmd.description.is_empty(),
                "Command {} has empty description",
                cmd.name
            );
        }
    }

    #[test]
    fn test_completer_no_slash_returns_empty() {
        let helper = SlashHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (pos, matches) = helper.complete("hello", 5, &ctx).unwrap();
        assert_eq!(pos, 0);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_completer_slash_alone_returns_all() {
        let helper = SlashHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, matches) = helper.complete("/", 1, &ctx).unwrap();
        assert_eq!(matches.len(), builtin_commands().len());
    }

    #[test]
    fn test_completer_slash_mo_filters() {
        let helper = SlashHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, matches) = helper.complete("/mo", 3, &ctx).unwrap();
        // Should match: model, model list
        assert!(matches.len() >= 2);
        assert!(matches.iter().all(|m| m.replacement.starts_with("/mo")));
    }

    #[test]
    fn test_completer_slash_model_space_filters_subcommands() {
        let helper = SlashHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, matches) = helper.complete("/model ", 7, &ctx).unwrap();
        // Should match: "model list" (prefix "model " matches "model list")
        assert!(matches.iter().any(|m| m.replacement == "/model list"));
    }

    #[test]
    fn test_completer_no_match() {
        let helper = SlashHelper::new();
        let history = rustyline::history::DefaultHistory::new();
        let ctx = rustyline::Context::new(&history);
        let (_, matches) = helper.complete("/zzz", 4, &ctx).unwrap();
        assert!(matches.is_empty());
    }

    #[test]
    fn test_format_help_includes_commands() {
        let help = format_help();
        assert!(help.contains("/model"));
        assert!(help.contains("/persona"));
        assert!(help.contains("/help"));
        assert!(help.contains("/trust"));
        assert!(help.contains("/quit"));
        // Subcommands should NOT appear as top-level entries
        assert!(!help.contains("/model list"));
    }

    #[test]
    fn test_format_help_includes_tab_hint() {
        let help = format_help();
        assert!(help.contains("Tab"));
    }

    #[test]
    fn test_builtin_commands_stub_descriptions_match_behavior() {
        let cmds = builtin_commands();
        let template = cmds.iter().find(|cmd| cmd.name == "template").unwrap();
        let history = cmds.iter().find(|cmd| cmd.name == "history").unwrap();
        let memory = cmds.iter().find(|cmd| cmd.name == "memory").unwrap();
        let trust = cmds.iter().find(|cmd| cmd.name == "trust").unwrap();

        assert_eq!(template.description, "List available templates");
        assert_eq!(history.description, "Show history command hints");
        assert_eq!(memory.description, "Show memory command hints");
        assert_eq!(trust.description, "Show local trusted-session status");
    }
}
