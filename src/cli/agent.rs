//! Agent command handlers (interactive + stdin mode).

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;

use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::config::Config;
use zeptoclaw::gateway::ipc::UsageSnapshot;
use zeptoclaw::health::UsageMetrics;
use zeptoclaw::providers::{
    configured_provider_names, resolve_runtime_provider, RUNTIME_SUPPORTED_PROVIDERS,
};

use super::common::{create_agent, create_agent_with_template, resolve_template};
use super::slash::SlashHelper;

/// Interactive or single-message agent mode.
pub(crate) async fn cmd_agent(
    message: Option<String>,
    template_name: Option<String>,
    stream: bool,
    dry_run: bool,
    mode: Option<String>,
) -> Result<()> {
    // Load configuration
    let mut config = Config::load().with_context(|| "Failed to load configuration")?;

    // Override agent mode from CLI flag if provided
    if let Some(ref mode_str) = mode {
        config.agent_mode.mode = mode_str.clone();
    }

    // Create message bus
    let bus = Arc::new(MessageBus::new());

    let template = if let Some(name) = template_name.as_deref() {
        Some(resolve_template(name)?)
    } else {
        None
    };

    // Create agent
    let agent = if template.is_some() {
        create_agent_with_template(config.clone(), bus.clone(), template).await?
    } else {
        create_agent(config.clone(), bus.clone()).await?
    };

    // Enable dry-run mode if requested
    if dry_run {
        agent.set_dry_run(true);
        eprintln!("[DRY RUN] Tool execution disabled — showing what would happen");
    }

    // Set up tool execution feedback (shows progress on stderr)
    let (feedback_tx, mut feedback_rx) = tokio::sync::mpsc::unbounded_channel();
    agent.set_tool_feedback(feedback_tx).await;

    // Spawn feedback printer to stderr
    tokio::spawn(async move {
        use zeptoclaw::agent::ToolFeedbackPhase;
        while let Some(fb) = feedback_rx.recv().await {
            match fb.phase {
                ToolFeedbackPhase::Starting => {
                    eprint!("  [{}] Running...", fb.tool_name);
                }
                ToolFeedbackPhase::Done { elapsed_ms } => {
                    eprintln!(" done ({:.1}s)", elapsed_ms as f64 / 1000.0);
                }
                ToolFeedbackPhase::Failed { elapsed_ms, error } => {
                    eprintln!(" failed ({:.1}s): {}", elapsed_ms as f64 / 1000.0, error);
                }
            }
        }
    });

    // Check whether the runtime can use at least one configured provider.
    if resolve_runtime_provider(&config).is_none() {
        let configured = configured_provider_names(&config);
        if configured.is_empty() {
            eprintln!(
                "Warning: No AI provider configured. Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY"
            );
            eprintln!("or add your API key to {:?}", Config::path());
        } else {
            eprintln!(
                "Warning: Configured provider(s) are not supported by this runtime: {}",
                configured.join(", ")
            );
            eprintln!(
                "Currently supported runtime providers: {}",
                RUNTIME_SUPPORTED_PROVIDERS.join(", ")
            );
        }
        eprintln!();
    }

    if let Some(msg) = message {
        // Single message mode
        let inbound = InboundMessage::new("cli", "user", "cli", &msg);
        let streaming = stream || config.agents.defaults.streaming;

        if streaming {
            use zeptoclaw::providers::StreamEvent;
            match agent.process_message_streaming(&inbound).await {
                Ok(mut rx) => {
                    while let Some(event) = rx.recv().await {
                        match event {
                            StreamEvent::Delta(text) => {
                                print!("{}", text);
                                let _ = io::stdout().flush();
                            }
                            StreamEvent::Done { .. } => break,
                            StreamEvent::Error(e) => {
                                eprintln!("{}", format_cli_error(&e));
                                std::process::exit(1);
                            }
                            StreamEvent::ToolCalls(_) => {}
                        }
                    }
                    println!(); // newline after streaming
                }
                Err(e) => {
                    eprintln!("{}", format_cli_error(&e));
                    std::process::exit(1);
                }
            }
        } else {
            match agent.process_message(&inbound).await {
                Ok(response) => {
                    println!("{}", response);
                }
                Err(e) => {
                    eprintln!("{}", format_cli_error(&e));
                    std::process::exit(1);
                }
            }
        }
    } else {
        // Interactive mode with rustyline (tab completion for slash commands)
        println!("ZeptoClaw Interactive Agent");
        println!("Type your message and press Enter. Type /help for commands, /quit to exit.");
        println!();

        // Try rustyline; fall back to raw stdin if terminal is unavailable.
        let mut rl = match Editor::new() {
            Ok(mut editor) => {
                editor.set_helper(Some(SlashHelper::new()));
                // Persist history across sessions
                let history_path =
                    dirs::home_dir().map(|h| h.join(".zeptoclaw/state/repl_history"));
                if let Some(ref path) = history_path {
                    let _ = editor.load_history(path);
                }
                Some((editor, history_path))
            }
            Err(_) => None,
        };

        loop {
            let input = if let Some((ref mut editor, _)) = rl {
                match editor.readline("> ") {
                    Ok(line) => line,
                    Err(ReadlineError::Eof | ReadlineError::Interrupted) => {
                        println!("Goodbye!");
                        break;
                    }
                    Err(e) => {
                        eprintln!("Error reading input: {}", e);
                        break;
                    }
                }
            } else {
                // Fallback: raw stdin (piped/non-TTY)
                print!("> ");
                io::stdout().flush()?;
                let mut buf = String::new();
                match io::stdin().lock().read_line(&mut buf) {
                    Ok(0) => {
                        println!();
                        break;
                    }
                    Ok(_) => buf,
                    Err(e) => {
                        eprintln!("Error reading input: {}", e);
                        break;
                    }
                }
            };

            let input = input.trim();
            if input.is_empty() {
                continue;
            }

            // Add to history
            if let Some((ref mut editor, _)) = rl {
                let _ = editor.add_history_entry(input);
            }

            // Handle slash commands
            if input.starts_with('/') {
                let cmd = &input[1..]; // strip leading /
                match cmd {
                    "quit" | "exit" => {
                        println!("Goodbye!");
                        break;
                    }
                    "help" => {
                        println!("{}", super::slash::format_help());
                        continue;
                    }
                    _ if cmd.starts_with("model") => {
                        use zeptoclaw::channels::model_switch::{
                            format_model_list, parse_model_command, ModelCommand,
                        };
                        use zeptoclaw::providers::configured_provider_models;
                        if let Some(mcmd) = parse_model_command(input) {
                            match mcmd {
                                ModelCommand::Show => {
                                    println!(
                                        "Current model: {}",
                                        config.agents.defaults.model
                                    );
                                }
                                ModelCommand::List => {
                                    let providers = configured_provider_names(&config)
                                        .into_iter()
                                        .map(|s| s.to_string())
                                        .collect::<Vec<_>>();
                                    let models = configured_provider_models(&config);
                                    let list =
                                        format_model_list(&providers, None, &models);
                                    println!("{}", list);
                                }
                                ModelCommand::Set(ov) => {
                                    config.agents.defaults.model = ov.model.clone();
                                    if let Some(p) = &ov.provider {
                                        println!("Switched to {}:{}", p, ov.model);
                                    } else {
                                        println!("Switched to {}", ov.model);
                                    }
                                }
                                ModelCommand::Reset => {
                                    if let Ok(fresh) = Config::load() {
                                        config.agents.defaults.model =
                                            fresh.agents.defaults.model;
                                    }
                                    println!(
                                        "Model reset to default: {}",
                                        config.agents.defaults.model
                                    );
                                }
                            }
                        } else {
                            println!(
                                "Current model: {}",
                                config.agents.defaults.model
                            );
                        }
                        continue;
                    }
                    _ if cmd.starts_with("persona") => {
                        use zeptoclaw::channels::persona_switch::{
                            parse_persona_command, PersonaCommand, PERSONA_PRESETS,
                        };
                        if let Some(pcmd) = parse_persona_command(input) {
                            match pcmd {
                                PersonaCommand::Show => {
                                    println!("Current persona: default");
                                }
                                PersonaCommand::List => {
                                    println!("Available personas:\n");
                                    for preset in PERSONA_PRESETS {
                                        println!(
                                            "  {:<16} {}",
                                            preset.name, preset.label
                                        );
                                    }
                                }
                                PersonaCommand::Set(name) => {
                                    println!("Persona set to: {}", name);
                                }
                                PersonaCommand::Reset => {
                                    println!("Persona reset to default.");
                                }
                            }
                        } else {
                            println!("Current persona: default");
                        }
                        continue;
                    }
                    "tools" => {
                        // AgentLoop exposes tool_count() but not an iterator.
                        // Print count and redirect to CLI command for full list.
                        let count = agent.tool_count().await;
                        println!(
                            "{} tools registered. Run 'zeptoclaw tools list' for details.",
                            count
                        );
                        continue;
                    }
                    _ if cmd.starts_with("template") => {
                        use zeptoclaw::config::templates::TemplateRegistry;
                        if cmd == "template list" || cmd == "template" {
                            let registry = TemplateRegistry::new();
                            println!("Available templates:\n");
                            for t in registry.list() {
                                println!("  {:<16} {}", t.name, t.description);
                            }
                        } else {
                            println!("Usage: /template list");
                        }
                        continue;
                    }
                    "history" => {
                        println!(
                            "Use 'zeptoclaw history list' for full history."
                        );
                        println!(
                            "This session's messages are tracked automatically."
                        );
                        continue;
                    }
                    "memory" => {
                        println!(
                            "Use 'zeptoclaw memory list' or 'zeptoclaw memory search <query>'."
                        );
                        continue;
                    }
                    "clear" => {
                        // SessionManager::delete() removes the session by key.
                        // The CLI session key is "cli" (from InboundMessage::new("cli", ...)).
                        let _ = agent.session_manager().delete("cli").await;
                        println!("Conversation cleared.");
                        continue;
                    }
                    _ => {
                        eprintln!("Unknown command: /{}", cmd);
                        eprintln!("Type /help to see available commands.");
                        continue;
                    }
                }
            }

            // Legacy quit/exit support (without slash)
            if input == "quit" || input == "exit" {
                println!("Goodbye!");
                break;
            }

            // Process message through agent
            let inbound = InboundMessage::new("cli", "user", "cli", input);
            let streaming = stream || config.agents.defaults.streaming;

            if streaming {
                use zeptoclaw::providers::StreamEvent;
                match agent.process_message_streaming(&inbound).await {
                    Ok(mut rx) => {
                        println!();
                        while let Some(event) = rx.recv().await {
                            match event {
                                StreamEvent::Delta(text) => {
                                    print!("{}", text);
                                    let _ = io::stdout().flush();
                                }
                                StreamEvent::Done { .. } => break,
                                StreamEvent::Error(e) => {
                                    eprintln!("{}", format_cli_error(&e));
                                }
                                StreamEvent::ToolCalls(_) => {}
                            }
                        }
                        println!();
                        println!();
                    }
                    Err(e) => {
                        eprintln!("{}", format_cli_error(&e));
                        eprintln!();
                    }
                }
            } else {
                match agent.process_message(&inbound).await {
                    Ok(response) => {
                        println!();
                        println!("{}", response);
                        println!();
                    }
                    Err(e) => {
                        eprintln!("{}", format_cli_error(&e));
                        eprintln!();
                    }
                }
            }
        }

        // Save history on exit
        if let Some((ref mut editor, Some(ref path))) = rl {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = editor.save_history(path);
        }
    }

    Ok(())
}

/// Run agent in stdin/stdout mode for containerized execution.
pub(crate) async fn cmd_agent_stdin() -> Result<()> {
    let mut config = Config::load().with_context(|| "Failed to load configuration")?;

    // Read JSON request from stdin
    let stdin = io::stdin();
    let mut input = String::new();
    stdin
        .lock()
        .read_line(&mut input)
        .with_context(|| "Failed to read from stdin")?;

    let request: zeptoclaw::gateway::AgentRequest =
        serde_json::from_str(&input).map_err(|e| anyhow::anyhow!("Invalid request JSON: {}", e))?;

    if let Err(e) = request.validate() {
        let response = zeptoclaw::gateway::AgentResponse::error(
            &request.request_id,
            &e.to_string(),
            "INVALID_REQUEST",
        );
        println!("{}", response.to_marked_json());
        io::stdout().flush()?;
        return Ok(());
    }

    let zeptoclaw::gateway::AgentRequest {
        request_id,
        message,
        agent_config,
        session,
    } = request;

    // Apply request-scoped agent defaults.
    config.agents.defaults = agent_config;

    // Create agent with merged config
    let bus = Arc::new(MessageBus::new());
    let agent = create_agent(config, bus.clone()).await?;

    // Set up usage metrics so the agent loop tracks tokens and tool calls.
    let usage_metrics = Arc::new(UsageMetrics::new());
    agent.set_usage_metrics(Arc::clone(&usage_metrics)).await;

    // Seed provided session state before processing.
    if let Some(ref seed_session) = session {
        agent.session_manager().save(seed_session).await?;
    }

    // Process the message
    let response = match agent.process_message(&message).await {
        Ok(content) => {
            let updated_session = agent.session_manager().get(&message.session_key).await?;
            zeptoclaw::gateway::AgentResponse::success(&request_id, &content, updated_session)
                .with_usage(UsageSnapshot::from_metrics(&usage_metrics))
        }
        Err(e) => {
            zeptoclaw::gateway::AgentResponse::error(&request_id, &e.to_string(), "PROCESS_ERROR")
                .with_usage(UsageSnapshot::from_metrics(&usage_metrics))
        }
    };

    // Write response with markers to stdout
    println!("{}", response.to_marked_json());
    io::stdout().flush()?;

    Ok(())
}

/// Format agent errors with actionable guidance for CLI users.
fn format_cli_error(e: &dyn std::fmt::Display) -> String {
    let msg = e.to_string();

    if msg.contains("Authentication error") {
        format!(
            "{}\n\n  Fix: Check your API key. Run 'zeptoclaw auth status' to verify.\n  Or:  Set ZEPTOCLAW_PROVIDERS_ANTHROPIC_API_KEY=sk-ant-...",
            msg
        )
    } else if msg.contains("Billing error") {
        format!(
            "{}\n\n  Fix: Add a payment method to your AI provider account.",
            msg
        )
    } else if msg.contains("Rate limit") {
        format!(
            "{}\n\n  Fix: Wait a moment and try again. Or set up a fallback provider.",
            msg
        )
    } else if msg.contains("Model not found") {
        format!(
            "{}\n\n  Fix: Check model name in config. Run 'zeptoclaw config check'.",
            msg
        )
    } else if msg.contains("Timeout") {
        format!(
            "{}\n\n  Fix: Try again. If persistent, check your network connection.",
            msg
        )
    } else if msg.contains("No AI provider configured") || msg.contains("provider") {
        format!(
            "{}\n\n  Fix: Run 'zeptoclaw onboard' to set up an AI provider.",
            msg
        )
    } else {
        msg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_cli_error_auth() {
        let e = anyhow::anyhow!("Authentication error: invalid key");
        let msg = format_cli_error(&e);
        assert!(msg.contains("Fix:"));
        assert!(msg.contains("auth status"));
    }

    #[test]
    fn test_format_cli_error_billing() {
        let e = anyhow::anyhow!("Billing error: payment required");
        let msg = format_cli_error(&e);
        assert!(msg.contains("Fix:"));
        assert!(msg.contains("payment method"));
    }

    #[test]
    fn test_format_cli_error_rate_limit() {
        let e = anyhow::anyhow!("Rate limit exceeded");
        let msg = format_cli_error(&e);
        assert!(msg.contains("Fix:"));
        assert!(msg.contains("Wait"));
    }

    #[test]
    fn test_format_cli_error_generic() {
        let e = anyhow::anyhow!("Something went wrong");
        let msg = format_cli_error(&e);
        assert_eq!(msg, "Something went wrong");
        assert!(!msg.contains("Fix:"));
    }
}
