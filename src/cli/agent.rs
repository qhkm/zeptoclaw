//! Agent command handlers (interactive + stdin mode).

use std::io::{self, BufRead, IsTerminal, Write};
use std::sync::Arc;

use anyhow::{Context, Result};
use rustyline::error::ReadlineError;
use rustyline::Editor;

use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::channels::model_switch::ModelOverride;
use zeptoclaw::config::Config;
use zeptoclaw::gateway::ipc::UsageSnapshot;
use zeptoclaw::health::UsageMetrics;
use zeptoclaw::providers::{
    configured_provider_names, resolve_runtime_provider, RUNTIME_SUPPORTED_PROVIDERS,
};
use zeptoclaw::tools::approval::{ApprovalRequest, ApprovalResponse};

use super::common::{create_agent, create_agent_with_template, resolve_template};
use super::slash::SlashHelper;

const CLI_CHANNEL: &str = "cli";
const CLI_SENDER_ID: &str = "user";
const CLI_CHAT_ID: &str = "cli";
const INTERACTIVE_CLI_METADATA_KEY: &str = "interactive_cli";
const TRUSTED_LOCAL_SESSION_METADATA_KEY: &str = "trusted_local_session";

fn cli_inbound_message(content: &str) -> InboundMessage {
    InboundMessage::new(CLI_CHANNEL, CLI_SENDER_ID, CLI_CHAT_ID, content)
}

fn cli_session_key() -> String {
    format!("{}:{}", CLI_CHANNEL, CLI_CHAT_ID)
}

fn active_model_override(
    model_override: &Option<(Option<String>, String)>,
    default_model: &str,
) -> Option<ModelOverride> {
    model_override
        .as_ref()
        .map(|(provider, model)| ModelOverride {
            provider: provider.clone().or_else(|| {
                default_model
                    .split_once(':')
                    .map(|(provider, _)| provider.to_string())
            }),
            model: model.clone(),
        })
}

fn format_tool_list(tool_names: &[&str]) -> String {
    if tool_names.is_empty() {
        return "No tools registered.".to_string();
    }

    let mut out = format!("Available tools ({}):\n\n", tool_names.len());
    for name in tool_names {
        out.push_str("  ");
        out.push_str(name);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn prompt_cli_approval(request: ApprovalRequest) -> ApprovalResponse {
    let args_display = serde_json::to_string_pretty(&request.arguments)
        .unwrap_or_else(|_| request.arguments.to_string());

    println!();
    println!("[Approval Required]");
    println!("Tool: {}", request.tool_name);
    println!("Arguments:\n{}", args_display);
    println!();

    loop {
        print!("Approve execution? [y/N]: ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        match io::stdin().lock().read_line(&mut input) {
            Ok(0) => {
                return ApprovalResponse::Denied(
                    "Approval prompt closed before a response was provided.".to_string(),
                );
            }
            Ok(_) => match input.trim().to_ascii_lowercase().as_str() {
                "y" | "yes" => return ApprovalResponse::Approved,
                "" | "n" | "no" => {
                    return ApprovalResponse::Denied("Execution not approved.".to_string());
                }
                _ => {
                    println!("Please answer 'yes' or 'no'.");
                }
            },
            Err(e) => {
                return ApprovalResponse::Denied(format!(
                    "Failed to read approval response: {}",
                    e
                ));
            }
        }
    }
}

fn is_interactive_cli_terminal(stdin_terminal: bool, stdout_terminal: bool) -> bool {
    stdin_terminal && stdout_terminal
}

fn has_interactive_cli_terminal() -> bool {
    is_interactive_cli_terminal(io::stdin().is_terminal(), io::stdout().is_terminal())
}

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

    // Spawn feedback printer with shimmer + step tracking
    tokio::spawn(async move {
        use super::shimmer::{
            extract_args_hint, format_tool_done, format_tool_failed, format_tool_start,
            print_response_separator, ShimmerSpinner,
        };
        use zeptoclaw::agent::ToolFeedbackPhase;

        let mut step: usize = 0;
        let mut shimmer: Option<ShimmerSpinner> = None;
        let mut had_tools = false;

        while let Some(fb) = feedback_rx.recv().await {
            match fb.phase {
                ToolFeedbackPhase::Thinking => {
                    shimmer = Some(ShimmerSpinner::start());
                }
                ToolFeedbackPhase::ThinkingDone => {
                    if let Some(s) = shimmer.take() {
                        s.stop();
                    }
                }
                ToolFeedbackPhase::Starting => {
                    // Stop shimmer if still running (LLM returned tool calls)
                    if let Some(s) = shimmer.take() {
                        s.stop();
                    }
                    step += 1;
                    had_tools = true;
                    let hint = fb
                        .args_json
                        .as_deref()
                        .and_then(|a| extract_args_hint(&fb.tool_name, a));
                    let line = format_tool_start(step, &fb.tool_name, hint.as_deref());
                    eprintln!("{}", line);
                }
                ToolFeedbackPhase::Done { elapsed_ms } => {
                    let hint = fb
                        .args_json
                        .as_deref()
                        .and_then(|a| extract_args_hint(&fb.tool_name, a));
                    // Move cursor up and overwrite the "Starting" line
                    eprint!("\x1b[1A\x1b[2K");
                    let line = format_tool_done(step, &fb.tool_name, hint.as_deref(), elapsed_ms);
                    eprintln!("{}", line);
                }
                ToolFeedbackPhase::Failed { elapsed_ms, error } => {
                    let hint = fb
                        .args_json
                        .as_deref()
                        .and_then(|a| extract_args_hint(&fb.tool_name, a));
                    eprint!("\x1b[1A\x1b[2K");
                    let line = format_tool_failed(
                        step,
                        &fb.tool_name,
                        hint.as_deref(),
                        elapsed_ms,
                        &error,
                    );
                    eprintln!("{}", line);
                }
                ToolFeedbackPhase::ResponseReady => {
                    if let Some(s) = shimmer.take() {
                        s.stop();
                    }
                    if had_tools {
                        print_response_separator();
                    }
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
        let inbound = cli_inbound_message(&msg);
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

        // Track model/persona overrides set via /model and /persona commands.
        // Injected into InboundMessage metadata so the agent loop uses them.
        let mut model_override: Option<(Option<String>, String)> = None; // (provider, model)
        let mut persona_override: Option<String> = None;

        let interactive_cli = has_interactive_cli_terminal();
        // Try rustyline for interactive terminals; fall back to raw stdin if line editing
        // is unavailable or terminal support is limited.
        let mut rl = if interactive_cli {
            match Editor::new() {
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
            }
        } else {
            None
        };
        let mut trusted_session = false;

        if interactive_cli {
            agent
                .set_approval_handler(|request| async move {
                    match tokio::task::spawn_blocking(move || prompt_cli_approval(request)).await {
                        Ok(response) => response,
                        Err(e) => ApprovalResponse::Denied(format!(
                            "Interactive approval prompt failed: {}",
                            e
                        )),
                    }
                })
                .await;
        }

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
                // Fallback: raw stdin for non-interactive input or terminals where
                // line editing is unavailable.
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
            if let Some(cmd) = input.strip_prefix('/') {
                match cmd {
                    "quit" | "exit" => {
                        println!("Goodbye!");
                        break;
                    }
                    "help" => {
                        println!("{}", super::slash::format_help());
                        continue;
                    }
                    _ if cmd == "model" || cmd.starts_with("model ") => {
                        use zeptoclaw::channels::model_switch::{
                            format_model_list, parse_model_command, ModelCommand,
                        };
                        use zeptoclaw::providers::configured_provider_models;
                        if let Some(mcmd) = parse_model_command(input) {
                            match mcmd {
                                ModelCommand::Show => {
                                    if let Some((ref p, ref m)) = model_override {
                                        let provider = p.as_deref().unwrap_or("auto");
                                        println!("Current model: {}:{} (override)", provider, m);
                                    } else {
                                        println!(
                                            "Current model: {} (default)",
                                            config.agents.defaults.model
                                        );
                                    }
                                }
                                ModelCommand::List => {
                                    let providers = configured_provider_names(&config)
                                        .into_iter()
                                        .map(|s| s.to_string())
                                        .collect::<Vec<_>>();
                                    let models = configured_provider_models(&config);
                                    let current = active_model_override(
                                        &model_override,
                                        &config.agents.defaults.model,
                                    );
                                    let list =
                                        format_model_list(&providers, current.as_ref(), &models);
                                    println!("{}", list);
                                }
                                ModelCommand::Set(ov) => {
                                    // Store override; injected via InboundMessage metadata
                                    // so the agent loop uses it (same pattern as Telegram).
                                    model_override = Some((ov.provider.clone(), ov.model.clone()));
                                    if let Some(p) = &ov.provider {
                                        println!("Switched to {}:{}", p, ov.model);
                                    } else {
                                        println!("Switched to {}", ov.model);
                                    }
                                }
                                ModelCommand::Reset => {
                                    model_override = None;
                                    println!(
                                        "Model reset to default: {}",
                                        config.agents.defaults.model
                                    );
                                }
                            }
                        } else {
                            println!("Current model: {}", config.agents.defaults.model);
                        }
                        continue;
                    }
                    _ if cmd == "persona" || cmd.starts_with("persona ") => {
                        use zeptoclaw::channels::persona_switch::{
                            parse_persona_command, PersonaCommand, PERSONA_PRESETS,
                        };
                        if let Some(pcmd) = parse_persona_command(input) {
                            match pcmd {
                                PersonaCommand::Show => {
                                    if let Some(ref p) = persona_override {
                                        println!("Current persona: {} (override)", p);
                                    } else {
                                        println!("Current persona: default");
                                    }
                                }
                                PersonaCommand::List => {
                                    println!("Available personas:\n");
                                    for preset in PERSONA_PRESETS {
                                        let marker =
                                            if persona_override.as_deref() == Some(preset.name) {
                                                " (active)"
                                            } else {
                                                ""
                                            };
                                        println!(
                                            "  {:<16} {}{}",
                                            preset.name, preset.label, marker
                                        );
                                    }
                                }
                                PersonaCommand::Set(name) => {
                                    persona_override = Some(name.clone());
                                    println!("Persona set to: {}", name);
                                }
                                PersonaCommand::Reset => {
                                    persona_override = None;
                                    println!("Persona reset to default.");
                                }
                            }
                        } else {
                            println!("Current persona: default");
                        }
                        continue;
                    }
                    "tools" => {
                        let tool_names = agent.tool_names().await;
                        let refs: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();
                        println!("{}", format_tool_list(&refs));
                        continue;
                    }
                    _ if cmd == "template" || cmd.starts_with("template ") => {
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
                        println!("Use 'zeptoclaw history list' for full history.");
                        println!("This session's messages are tracked automatically.");
                        continue;
                    }
                    "memory" => {
                        println!(
                            "Use 'zeptoclaw memory list' or 'zeptoclaw memory search <query>'."
                        );
                        continue;
                    }
                    "clear" => {
                        match agent.session_manager().delete(&cli_session_key()).await {
                            Ok(_) => println!("Conversation cleared."),
                            Err(e) => eprintln!("Warning: failed to clear session: {}", e),
                        }
                        continue;
                    }
                    "trust" => {
                        if interactive_cli {
                            let status = if trusted_session { "ON" } else { "OFF" };
                            println!("Trusted local session is {}.", status);
                            if !trusted_session {
                                println!(
                                    "Use /trust on to bypass approval prompts for this local interactive CLI session."
                                );
                            }
                        } else {
                            println!("Trusted session override is only available in interactive CLI mode.");
                        }
                        continue;
                    }
                    "trust on" => {
                        if interactive_cli {
                            trusted_session = true;
                            println!(
                                "Trusted local session enabled for this interactive CLI session."
                            );
                        } else {
                            println!("Trusted session override is only available in interactive CLI mode.");
                        }
                        continue;
                    }
                    "trust off" => {
                        trusted_session = false;
                        println!("Trusted local session disabled.");
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

            // Process message through agent, injecting any active overrides
            let mut inbound = cli_inbound_message(input);
            if interactive_cli {
                inbound = inbound.with_metadata(INTERACTIVE_CLI_METADATA_KEY, "true");
                if trusted_session {
                    inbound = inbound.with_metadata(TRUSTED_LOCAL_SESSION_METADATA_KEY, "true");
                }
            }
            if let Some((ref provider, ref model)) = model_override {
                inbound = inbound.with_metadata("model_override", model);
                if let Some(ref p) = provider {
                    inbound = inbound.with_metadata("provider_override", p);
                }
            }
            if let Some(ref persona) = persona_override {
                inbound = inbound.with_metadata("persona_override", persona);
            }
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

    #[test]
    fn test_cli_session_key_matches_inbound_message() {
        let inbound = cli_inbound_message("hello");
        assert_eq!(inbound.session_key, cli_session_key());
    }

    #[test]
    fn test_format_tool_list_lists_each_tool() {
        let output = format_tool_list(&["echo", "filesystem", "web_fetch"]);
        assert!(output.contains("Available tools (3):"));
        assert!(output.contains("  echo"));
        assert!(output.contains("  filesystem"));
        assert!(output.contains("  web_fetch"));
    }

    #[test]
    fn test_format_tool_list_handles_empty_registry() {
        assert_eq!(format_tool_list(&[]), "No tools registered.");
    }

    #[test]
    fn test_active_model_override_uses_explicit_provider() {
        let current = active_model_override(
            &Some((Some("openai".to_string()), "gpt-5.1".to_string())),
            "anthropic:claude-sonnet-4-5-20250929",
        )
        .unwrap();
        assert_eq!(current.provider.as_deref(), Some("openai"));
        assert_eq!(current.model, "gpt-5.1");
    }

    #[test]
    fn test_active_model_override_falls_back_to_default_provider() {
        let current = active_model_override(
            &Some((None, "claude-haiku-4-5-20251001".to_string())),
            "anthropic:claude-sonnet-4-5-20250929",
        )
        .unwrap();
        assert_eq!(current.provider.as_deref(), Some("anthropic"));
        assert_eq!(current.model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn test_active_model_override_marks_current_in_formatted_list() {
        let current = active_model_override(
            &Some((Some("openai".to_string()), "gpt-5.1".to_string())),
            "anthropic:claude-sonnet-4-5-20250929",
        );
        let providers = vec!["openai".to_string()];
        let list =
            zeptoclaw::channels::model_switch::format_model_list(&providers, current.as_ref(), &[]);
        assert!(list.contains("gpt-5.1 GPT-5.1 (current)"));
    }

    #[test]
    fn test_has_interactive_cli_terminal_requires_stdin_and_stdout_tty() {
        assert!(is_interactive_cli_terminal(true, true));
        assert!(!is_interactive_cli_terminal(true, false));
        assert!(!is_interactive_cli_terminal(false, true));
        assert!(!is_interactive_cli_terminal(false, false));
    }
}
