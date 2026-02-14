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
) -> Result<()> {
    // Load configuration
    let config = Config::load().with_context(|| "Failed to load configuration")?;

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
                                eprintln!("\nStream error: {}", e);
                                std::process::exit(1);
                            }
                            StreamEvent::ToolCalls(_) => {}
                        }
                    }
                    println!(); // newline after streaming
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    std::process::exit(1);
                }
            }
        } else {
            match agent.process_message(&inbound).await {
                Ok(response) => {
                    println!("{}", response);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
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
                                            eprintln!("\nStream error: {}", e);
                                        }
                                        StreamEvent::ToolCalls(_) => {}
                                    }
                                }
                                println!();
                                println!();
                            }
                            Err(e) => {
                                eprintln!("Error: {}", e);
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
                                eprintln!("Error: {}", e);
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
