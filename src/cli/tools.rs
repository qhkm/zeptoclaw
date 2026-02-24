//! Tools CLI command handlers — tool discovery and info.

use anyhow::Result;
use zeptoclaw::config::Config;

use super::ToolsAction;

/// Static tool info for CLI display.
struct ToolInfo {
    name: &'static str,
    description: &'static str,
    requires_config: bool,
    config_hint: &'static str,
}

const TOOLS: &[ToolInfo] = &[
    ToolInfo {
        name: "echo",
        description: "Echo input back (testing)",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "read_file",
        description: "Read a file from workspace",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "write_file",
        description: "Write content to a file in workspace",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "list_dir",
        description: "List directory contents in workspace",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "edit_file",
        description: "Edit a file with search/replace",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "shell",
        description: "Execute shell commands (with runtime isolation)",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "web_search",
        description: "Search the web (Brave with API key, DuckDuckGo free fallback)",
        requires_config: false,
        config_hint: "Optional: Set tools.web.search.api_key for Brave Search",
    },
    ToolInfo {
        name: "web_fetch",
        description: "Fetch and extract content from URLs",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "memory_search",
        description: "Search workspace markdown memory",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "memory_get",
        description: "Get specific workspace memory file",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "longterm_memory",
        description: "Persistent key-value memory (set/get/search/delete/list)",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "message",
        description: "Send proactive messages to channels",
        requires_config: true,
        config_hint: "Configure at least one channel (telegram, slack, discord)",
    },
    ToolInfo {
        name: "cron",
        description: "Schedule recurring tasks",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "spawn",
        description: "Delegate tasks to background workers",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "delegate",
        description: "Delegate to specialized sub-agents",
        requires_config: false,
        config_hint: "",
    },
    ToolInfo {
        name: "whatsapp_send",
        description: "Send WhatsApp messages via Cloud API",
        requires_config: true,
        config_hint: "Set tools.whatsapp.phone_number_id + access_token",
    },
    ToolInfo {
        name: "google_sheets",
        description: "Read/write Google Sheets",
        requires_config: true,
        config_hint: "Set tools.google_sheets.access_token",
    },
    ToolInfo {
        name: "r8r",
        description: "Execute R8r deterministic workflows",
        requires_config: true,
        config_hint: "Set R8R_API_URL env var",
    },
    ToolInfo {
        name: "reminder",
        description: "Persistent reminders (add/complete/snooze/overdue)",
        requires_config: false,
        config_hint: "",
    },
];

pub(crate) async fn cmd_tools(action: ToolsAction) -> Result<()> {
    match action {
        ToolsAction::List => cmd_tools_list().await,
        ToolsAction::Info { name } => cmd_tools_info(name).await,
    }
}

async fn cmd_tools_list() -> Result<()> {
    let config = Config::load().unwrap_or_default();

    println!("Available Tools ({} total)", TOOLS.len());
    println!("{}", "=".repeat(60));
    println!();

    for tool in TOOLS {
        let configured = !tool.requires_config || is_tool_configured(&config, tool.name);
        let status_icon = if configured { "+" } else { "-" };

        println!("  [{}] {}", status_icon, tool.name);
        println!("      {}", tool.description);
        if !configured {
            println!("      Setup: {}", tool.config_hint);
        }
        println!();
    }

    Ok(())
}

async fn cmd_tools_info(name: String) -> Result<()> {
    let config = Config::load().unwrap_or_default();

    match TOOLS.iter().find(|t| t.name == name) {
        Some(t) => {
            println!("Tool: {}", t.name);
            println!("{}", "-".repeat(40));
            println!("Description: {}", t.description);
            let configured = !t.requires_config || is_tool_configured(&config, t.name);
            println!(
                "Status: {}",
                if configured { "ready" } else { "needs setup" }
            );
            if !configured {
                println!("Setup: {}", t.config_hint);
            }
        }
        None => {
            println!(
                "Unknown tool '{}'. Run 'zeptoclaw tools list' to see all tools.",
                name
            );
        }
    }

    Ok(())
}

fn is_tool_configured(config: &Config, name: &str) -> bool {
    match name {
        "web_search" => true, // Always available: Brave with key, DDG fallback without
        "whatsapp_send" => {
            config
                .tools
                .whatsapp
                .phone_number_id
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
                && config
                    .tools
                    .whatsapp
                    .access_token
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty())
        }
        "google_sheets" => {
            config
                .tools
                .google_sheets
                .access_token
                .as_ref()
                .is_some_and(|v| !v.trim().is_empty())
                || config
                    .tools
                    .google_sheets
                    .service_account_base64
                    .as_ref()
                    .is_some_and(|v| !v.trim().is_empty())
        }
        "message" => {
            config.channels.telegram.as_ref().is_some_and(|c| c.enabled)
                || config.channels.slack.as_ref().is_some_and(|c| c.enabled)
                || config.channels.discord.as_ref().is_some_and(|c| c.enabled)
        }
        "r8r" => std::env::var("R8R_API_URL").is_ok(),
        _ => true,
    }
}

/// Print a compact tools summary for the status command.
pub fn print_tools_summary(config: &Config) {
    let mut ready = 0;
    let mut needs_setup = 0;

    for tool in TOOLS {
        let configured = !tool.requires_config || is_tool_configured(config, tool.name);
        if configured {
            println!("  + {}", tool.name);
            ready += 1;
        } else {
            println!("  - {} ({})", tool.name, tool.config_hint);
            needs_setup += 1;
        }
    }

    println!();
    println!("  {} ready, {} need setup", ready, needs_setup);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tools_list_count() {
        assert_eq!(TOOLS.len(), 19);
    }

    #[test]
    fn test_tool_names_unique() {
        let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "Duplicate tool names found");
    }

    #[test]
    fn test_is_tool_configured_default_tools() {
        let config = Config::default();
        // Tools that don't require config should always be configured
        assert!(is_tool_configured(&config, "echo"));
        assert!(is_tool_configured(&config, "shell"));
        assert!(is_tool_configured(&config, "cron"));
    }

    #[test]
    fn test_is_tool_configured_web_search_no_key() {
        let config = Config::default();
        assert!(is_tool_configured(&config, "web_search")); // DDG fallback always available
    }

    #[test]
    fn test_is_tool_configured_web_search_with_key() {
        let mut config = Config::default();
        config.tools.web.search.api_key = Some("test-key".to_string());
        assert!(is_tool_configured(&config, "web_search"));
    }

    #[test]
    fn test_is_tool_configured_unknown_tool() {
        let config = Config::default();
        assert!(is_tool_configured(&config, "unknown_tool"));
    }
}
