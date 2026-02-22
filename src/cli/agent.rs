//! Agent command handlers (interactive + stdin mode).

use std::io::{self, BufRead, Write};
use std::sync::Arc;

use anyhow::{Context, Result};

use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::config::Config;
use zeptoclaw::providers::{
    configured_provider_names, resolve_runtime_provider, RUNTIME_SUPPORTED_PROVIDERS,
};

use super::common::{create_agent, create_agent_with_template, resolve_template};

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
        // Interactive mode
        println!("ZeptoClaw Interactive Agent");
        println!("Type your message and press Enter. Type 'quit' or 'exit' to stop.");
        println!();

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        loop {
            print!("> ");
            stdout.flush()?;

            let mut input = String::new();
            match stdin.lock().read_line(&mut input) {
                Ok(0) => {
                    // EOF
                    println!();
                    break;
                }
                Ok(_) => {
                    let input = input.trim();
                    if input.is_empty() {
                        continue;
                    }
                    if input == "quit" || input == "exit" {
                        println!("Goodbye!");
                        break;
                    }

                    // Process message
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
                Err(e) => {
                    eprintln!("Error reading input: {}", e);
                    break;
                }
            }
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

    // Seed provided session state before processing.
    if let Some(ref seed_session) = session {
        agent.session_manager().save(seed_session).await?;
    }

    // Process the message
    let response = match agent.process_message(&message).await {
        Ok(content) => {
            let updated_session = agent.session_manager().get(&message.session_key).await?;
            zeptoclaw::gateway::AgentResponse::success(&request_id, &content, updated_session)
        }
        Err(e) => {
            zeptoclaw::gateway::AgentResponse::error(&request_id, &e.to_string(), "PROCESS_ERROR")
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
