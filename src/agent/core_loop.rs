//! Terminal executor for the middleware pipeline.
//!
//! `CoreLoop` sits at the end of the middleware chain and handles:
//! - The initial LLM call (provider.chat)
//! - The tool execution loop (iterate until no more tool calls)
//! - Token recording after each LLM call
//! - LoopGuard (repetition detection) and ChainTracker (suspicious sequences)
//! - Tool call limit enforcement + synthesis call
//! - Sequential vs parallel tool execution decision
//! - Per-tool: approval gate, agent mode, dry-run, hooks, feedback,
//!   execution via `kernel::execute_tool()`, result sanitization

use std::sync::Arc;

use async_trait::async_trait;
use futures::FutureExt;
use tracing::{debug, error, info, warn};

use super::loop_guard::{truncate_utf8, LoopGuard, LoopGuardAction, ToolCallSig};
use super::middleware::{PipelineContext, PipelineOutput, Subsystems};
use super::pipeline::Terminal;
use super::{ToolFeedback, ToolFeedbackPhase};
use crate::agent::context_monitor::ContextMonitor;
use crate::bus::InboundMessage;
use crate::error::Result;
use crate::providers::{LLMToolCall, Usage};
use crate::session::types::ToolCall;
use crate::session::{Message, Role};
use crate::tools::{ToolCategory, ToolContext, ToolRegistry};

use tokio::sync::RwLock;

const INTERACTIVE_CLI_METADATA_KEY: &str = "interactive_cli";
const TRUSTED_LOCAL_SESSION_METADATA_KEY: &str = "trusted_local_session";

/// Terminal executor at the end of the middleware pipeline.
///
/// Expects that upstream middlewares have already populated:
/// - `ctx.provider` (ProviderResolutionMiddleware)
/// - `ctx.session` (SessionMiddleware + CompactionMiddleware)
/// - `ctx.messages` (ContextBuildMiddleware)
/// - `ctx.tool_definitions` (ContextBuildMiddleware)
/// - `ctx.chat_options` (ContextBuildMiddleware)
/// - `ctx.model` (ProviderResolutionMiddleware)
///
/// All subsystem references are obtained from `ctx.subsystems` at execution
/// time; the CoreLoop itself is stateless.
#[derive(Debug)]
pub struct CoreLoop;

impl CoreLoop {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CoreLoop {
    fn default() -> Self {
        Self::new()
    }
}

fn is_trusted_local_session(msg: &InboundMessage) -> bool {
    msg.channel == "cli"
        && msg
            .metadata
            .get(INTERACTIVE_CLI_METADATA_KEY)
            .is_some_and(|value| value == "true")
        && msg
            .metadata
            .get(TRUSTED_LOCAL_SESSION_METADATA_KEY)
            .is_some_and(|value| value == "true")
        && msg
            .metadata
            .get("is_batch")
            .is_none_or(|value| value != "true")
}

/// Returns `true` if any tool in the batch may cause ordering-sensitive side effects
/// and the batch should be executed sequentially rather than in parallel.
async fn needs_sequential_execution(
    tools: &Arc<RwLock<ToolRegistry>>,
    tool_calls: &[LLMToolCall],
) -> bool {
    let guard = tools.read().await;
    tool_calls.iter().any(|tc| {
        guard
            .get(&tc.name)
            .map(|t| {
                matches!(
                    t.category(),
                    ToolCategory::FilesystemWrite | ToolCategory::Shell
                )
            })
            .unwrap_or(true)
    })
}

/// Check the loop guard for repeated tool-call patterns.
///
/// Returns `true` if the circuit breaker tripped and the caller should break.
fn check_loop_guard(
    guard: &mut LoopGuard,
    tool_calls: &[LLMToolCall],
    session: &mut crate::session::Session,
) -> bool {
    let call_sigs: Vec<ToolCallSig<'_>> = tool_calls
        .iter()
        .map(|tc| ToolCallSig {
            name: tc.name.as_str(),
            arguments: tc.arguments.as_str(),
        })
        .collect();
    match guard.check(&call_sigs) {
        LoopGuardAction::Allow => false,
        LoopGuardAction::Warn {
            reason,
            suggested_delay_ms,
        } => {
            warn!(reason = %reason, "Loop guard warning");
            let delay_hint = suggested_delay_ms
                .map(|ms| format!(" (suggested delay: {}ms)", ms))
                .unwrap_or_default();
            session.add_message(Message::system(&format!(
                "[LoopGuard] {reason}{delay_hint}.",
            )));
            false
        }
        LoopGuardAction::Block { reason } => {
            warn!(reason = %reason, "Loop guard blocked tool call");
            session.add_message(Message::system(&format!("[LoopGuard] blocked: {reason}.",)));
            true
        }
        LoopGuardAction::CircuitBreak { total_repetitions } => {
            warn!(
                total_repetitions = total_repetitions,
                "Loop guard circuit breaker triggered"
            );
            session.add_message(Message::system(&format!(
                "[LoopGuard] circuit breaker tripped ({total_repetitions} total repetitions).",
            )));
            true
        }
    }
}

/// Record tool outcomes with the loop guard and check for repeated identical results.
///
/// Returns `true` if the circuit breaker tripped and the caller should break.
fn check_loop_guard_outcomes(
    guard: &mut LoopGuard,
    tool_calls: &[LLMToolCall],
    results: &[(String, String)],
    session: &mut crate::session::Session,
) -> bool {
    let call_map: std::collections::HashMap<&str, (&str, &str)> = tool_calls
        .iter()
        .map(|tc| (tc.id.as_str(), (tc.name.as_str(), tc.arguments.as_str())))
        .collect();

    for (id, result) in results {
        if let Some((name, args)) = call_map.get(id.as_str()) {
            let prefix = truncate_utf8(result, 1000);
            if let Some(action) = guard.record_outcome(name, args, prefix) {
                match action {
                    LoopGuardAction::Block { reason } => {
                        warn!(reason = %reason, "Loop guard blocked repeated outcome");
                        session.add_message(Message::system(&format!(
                            "[LoopGuard] blocked: {reason}.",
                        )));
                        return true;
                    }
                    LoopGuardAction::CircuitBreak { total_repetitions } => {
                        warn!(
                            total_repetitions = total_repetitions,
                            "Loop guard circuit breaker triggered via outcome"
                        );
                        session.add_message(Message::system(&format!(
                            "[LoopGuard] circuit breaker tripped ({total_repetitions} total repetitions).",
                        )));
                        return true;
                    }
                    LoopGuardAction::Warn {
                        reason,
                        suggested_delay_ms,
                    } => {
                        warn!(reason = %reason, "Loop guard outcome warning");
                        let delay_hint = suggested_delay_ms
                            .map(|ms| format!(" (suggested delay: {}ms)", ms))
                            .unwrap_or_default();
                        session.add_message(Message::system(&format!(
                            "[LoopGuard] {reason}{delay_hint}.",
                        )));
                    }
                    LoopGuardAction::Allow => {}
                }
            }
        }
    }
    false
}

/// Format a dry-run result describing what a tool call would do.
fn dry_run_result(name: &str, args: &serde_json::Value, raw_args: &str, budget: usize) -> String {
    let args_display = serde_json::to_string_pretty(args).unwrap_or_else(|_| raw_args.to_string());
    let sanitized = crate::utils::sanitize::sanitize_tool_result(&args_display, budget);
    format!(
        "[DRY RUN] Would execute tool '{}' with arguments: {}",
        name, sanitized
    )
}

/// Type alias for the boxed future returned by an approval handler.
type ApprovalFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = crate::tools::approval::ApprovalResponse> + Send>,
>;

/// Type alias for an approval handler function.
type ApprovalHandler =
    Arc<dyn Fn(crate::tools::approval::ApprovalRequest) -> ApprovalFuture + Send + Sync>;

/// Resolve tool approval, returning an error message if blocked.
async fn resolve_tool_approval(
    gate: &crate::tools::approval::ApprovalGate,
    approval_handler: Option<&ApprovalHandler>,
    tool_name: &str,
    args: &serde_json::Value,
) -> Option<String> {
    use crate::tools::approval::ApprovalResponse;

    if !gate.requires_approval(tool_name) {
        return None;
    }

    if let Some(handler) = approval_handler {
        match handler(gate.create_request(tool_name, args)).await {
            ApprovalResponse::Approved => None,
            ApprovalResponse::Denied(reason) => Some(format!(
                "Tool '{}' was denied by user approval. {}",
                tool_name, reason
            )),
            ApprovalResponse::TimedOut => Some(format!(
                "Tool '{}' approval timed out and was not executed.",
                tool_name
            )),
        }
    } else {
        let prompt = gate.format_approval_request(tool_name, args);
        Some(format!(
            "Tool '{}' requires user approval and was not executed. {}",
            tool_name, prompt
        ))
    }
}

/// Build messages from session using the context builder, resolving images
/// and filtering empty messages.
///
/// This is the same logic as `AgentLoop::build_resolved_messages`, extracted
/// here so CoreLoop can rebuild messages during tool iterations without
/// needing a reference to AgentLoop.
async fn build_resolved_messages(
    subsystems: &Subsystems,
    session: &crate::session::Session,
    memory_override: Option<&str>,
) -> Vec<Message> {
    let mut msgs = subsystems
        .context_builder
        .build_messages_with_memory_override(&session.messages, "", memory_override);

    if let Some(dir) = subsystems.session_manager.sessions_dir() {
        resolve_images_to_base64(&mut msgs, dir).await;
    }

    msgs.retain(|m| !(m.role == Role::User && m.content.is_empty() && !m.has_images()));
    msgs
}

/// Resolve any `ImageSource::FilePath` entries in `messages` to
/// `ImageSource::Base64` so that LLM providers can consume them directly.
async fn resolve_images_to_base64(messages: &mut [Message], sessions_dir: &std::path::Path) {
    use crate::session::{ContentPart, ImageSource};
    use base64::Engine as _;

    for msg in messages.iter_mut() {
        let mut needs_resolve = false;
        for part in &msg.content_parts {
            if matches!(
                part,
                ContentPart::Image {
                    source: ImageSource::FilePath { .. },
                    ..
                }
            ) {
                needs_resolve = true;
                break;
            }
        }
        if !needs_resolve {
            continue;
        }

        let mut resolved_parts: Vec<ContentPart> = Vec::new();
        for part in std::mem::take(&mut msg.content_parts) {
            match part {
                ContentPart::Image {
                    source: ImageSource::FilePath { ref path },
                    ref media_type,
                } => {
                    let abs_path = sessions_dir.join(path);
                    if let Ok(data) = tokio::fs::read(&abs_path).await {
                        resolved_parts.push(ContentPart::Image {
                            source: ImageSource::Base64 {
                                data: base64::engine::general_purpose::STANDARD.encode(&data),
                            },
                            media_type: media_type.clone(),
                        });
                    }
                    // Unreadable file → silently drop this image part.
                }
                other => resolved_parts.push(other),
            }
        }
        msg.content_parts = resolved_parts;
    }
}

/// Record usage tokens on all applicable metrics sinks.
fn record_usage(
    subsystems: &Subsystems,
    usage: Option<&Usage>,
    token_budget: &crate::agent::budget::TokenBudget,
) {
    if let Some(usage) = usage {
        {
            let metrics = subsystems.usage_metrics.try_read();
            if let Ok(metrics) = metrics {
                metrics.record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
            }
        }
        subsystems
            .metrics_collector
            .record_tokens(usage.prompt_tokens as u64, usage.completion_tokens as u64);
        token_budget.record(usage.prompt_tokens as u64, usage.completion_tokens as u64);
    }
}

/// Send a tool feedback event if a sender is configured.
async fn send_feedback(subsystems: &Subsystems, feedback: ToolFeedback) {
    if let Some(tx) = subsystems.tool_feedback_tx.read().await.as_ref() {
        let _ = tx.send(feedback);
    }
}

#[async_trait]
impl Terminal for CoreLoop {
    async fn execute(&self, ctx: &mut PipelineContext) -> Result<PipelineOutput> {
        let provider = ctx
            .provider
            .as_ref()
            .expect("ProviderResolutionMiddleware must run first")
            .clone();
        let messages = ctx
            .messages
            .take()
            .expect("ContextBuildMiddleware must run first");
        let tool_definitions = ctx
            .tool_definitions
            .take()
            .expect("ContextBuildMiddleware must set tool_definitions");
        let options = ctx
            .chat_options
            .take()
            .expect("ContextBuildMiddleware must set chat_options");
        let model_string = ctx.model.clone().unwrap_or_default();
        let model = if model_string.is_empty() {
            None
        } else {
            Some(model_string.as_str())
        };

        // Grab refs we'll need throughout the tool loop.
        let subsystems = Arc::clone(&ctx.subsystems);
        let config = Arc::clone(&ctx.config);
        let is_dry_run = ctx.dry_run;
        let memory_override = ctx.memory_override.clone();
        let inbound = &ctx.inbound;

        // --- Initial LLM call ---

        send_feedback(
            &subsystems,
            ToolFeedback {
                tool_name: String::new(),
                phase: ToolFeedbackPhase::Thinking,
                args_json: None,
            },
        )
        .await;

        let mut response = provider
            .chat(messages, tool_definitions, model, options.clone())
            .await?;

        send_feedback(
            &subsystems,
            ToolFeedback {
                tool_name: String::new(),
                phase: ToolFeedbackPhase::ThinkingDone,
                args_json: None,
            },
        )
        .await;

        record_usage(
            &subsystems,
            response.usage.as_ref(),
            &subsystems.token_budget,
        );

        // Grab the mutable session. Upstream SessionMiddleware populated it.
        let session = ctx
            .session
            .as_mut()
            .expect("SessionMiddleware must run first");

        // --- Tool loop ---
        let max_iterations = config.agents.defaults.max_tool_iterations;
        let mut iteration = 0;
        let mut chain_tracker = crate::safety::chain_alert::ChainTracker::new();
        let mut loop_guard = if config.agents.defaults.loop_guard.enabled {
            Some(LoopGuard::new(config.agents.defaults.loop_guard.clone()))
        } else {
            None
        };

        let tool_call_limit = &subsystems.tool_call_limit;
        let token_budget = &subsystems.token_budget;

        while response.has_tool_calls() && iteration < max_iterations {
            iteration += 1;
            debug!("Tool iteration {} of {}", iteration, max_iterations);

            // Enforce tool call limit BEFORE recording metrics or adding
            // the assistant message to the session.
            if tool_call_limit.is_exceeded() {
                info!(
                    count = tool_call_limit.count(),
                    limit = ?tool_call_limit.limit(),
                    "Tool call limit already reached, skipping tool execution"
                );
                break;
            }
            // Truncate batch to remaining budget so we never overshoot.
            if let Some(remaining) = tool_call_limit.remaining() {
                let allowed = remaining as usize;
                if allowed < response.tool_calls.len() {
                    info!(
                        batch_size = response.tool_calls.len(),
                        remaining = allowed,
                        "Truncating tool call batch to remaining budget"
                    );
                    response.tool_calls.truncate(allowed);
                }
            }

            // Record metrics AFTER truncation so counts reflect actual execution.
            {
                let metrics = subsystems.usage_metrics.read().await;
                metrics.record_tool_calls(response.tool_calls.len() as u64);
            }

            // Add assistant message with tool calls (post-truncation).
            let mut assistant_msg = Message::assistant(&response.content);
            assistant_msg.tool_calls = Some(
                response
                    .tool_calls
                    .iter()
                    .map(|tc| ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect(),
            );
            session.add_message(assistant_msg);

            // Build tool context.
            let workspace = config.workspace_path();
            let workspace_str = workspace.to_string_lossy();
            let tool_ctx = ToolContext::new()
                .with_channel(&inbound.channel, &inbound.chat_id)
                .with_workspace(&workspace_str)
                .with_batch(
                    inbound
                        .metadata
                        .get("is_batch")
                        .is_some_and(|v| v == "true"),
                );

            let hook_engine = Arc::new(
                crate::hooks::HookEngine::new(config.hooks.clone())
                    .with_bus(Arc::new(subsystems.bus.clone())),
            );

            // Compute dynamic tool result budget based on remaining context space.
            let current_tokens = ContextMonitor::estimate_tokens(&session.messages);
            let context_limit = config.compaction.context_limit;
            let max_result_bytes = config.agents.defaults.max_tool_result_bytes;
            let result_budget = crate::utils::sanitize::compute_tool_result_budget(
                context_limit,
                current_tokens,
                response.tool_calls.len(),
                max_result_bytes,
            );

            let trusted_local_session = is_trusted_local_session(inbound);

            let run_sequential = (!trusted_local_session
                && subsystems.approval_handler.is_some()
                && response.tool_calls.iter().any(|tool_call| {
                    subsystems.approval_gate.as_ref().is_some_and(
                        |g: &crate::tools::approval::ApprovalGate| {
                            g.requires_approval(&tool_call.name)
                        },
                    )
                }))
                || needs_sequential_execution(&subsystems.tools, &response.tool_calls).await;

            let tool_timeout_secs = if config.agents.defaults.tool_timeout_secs > 0 {
                config.agents.defaults.tool_timeout_secs
            } else {
                config.agents.defaults.agent_timeout_secs
            };
            let tool_timeout = std::time::Duration::from_secs(tool_timeout_secs.max(1));

            // Clone inbound metadata for routing propagation in tool `for_user` messages.
            let inbound_metadata = inbound.metadata.clone();

            let tool_futures: Vec<_> = response
                .tool_calls
                .iter()
                .map(|tool_call| {
                    // Clone Arc<Subsystems> for each future — each future accesses
                    // subsystem fields through this shared reference.
                    let subs = Arc::clone(&subsystems);
                    let ctx = tool_ctx.clone();
                    let name = tool_call.name.clone();
                    let id = tool_call.id.clone();
                    let raw_args = tool_call.arguments.clone();
                    let hooks = Arc::clone(&hook_engine);
                    let budget = result_budget;
                    let dry_run = is_dry_run;
                    let inbound_meta = inbound_metadata.clone();

                    async move {
                        let args: serde_json::Value = match serde_json::from_str(&raw_args) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!(tool = %name, error = %e, "Invalid JSON in tool arguments");
                                serde_json::json!({"_parse_error": format!("Invalid arguments JSON: {}", e)})
                            }
                        };

                        // Check hooks before executing
                        let channel_name = ctx.channel.as_deref().unwrap_or("cli");
                        let chat_id = ctx.chat_id.as_deref().unwrap_or(channel_name);
                        if let crate::hooks::HookResult::Block(msg) =
                            hooks.before_tool(&name, &args, channel_name, chat_id)
                        {
                            return (id, format!("Tool '{}' blocked by hook: {}", name, msg), false);
                        }

                        let agent_mode = subs.agent_mode;

                        // Agent mode enforcement (before approval gate).
                        {
                            let mode_policy = crate::security::ModePolicy::new(agent_mode);
                            let tools_guard = subs.tools.read().await;
                            if let Some(tool) = tools_guard.get(&name) {
                                let tool_category = tool.category();
                                match mode_policy.check(tool_category) {
                                    crate::security::CategoryPermission::Blocked => {
                                        info!(tool = %name, mode = %agent_mode, category = ?tool_category, "Tool blocked by agent mode");
                                        return (
                                            id,
                                            format!(
                                                "Tool '{}' is blocked in {} mode (category: {})",
                                                name, agent_mode, tool_category
                                            ),
                                            false,
                                        );
                                    }
                                    crate::security::CategoryPermission::RequiresApproval => {
                                        if trusted_local_session {
                                            info!(tool = %name, mode = %agent_mode, category = ?tool_category, "Trusted local session bypassed approval-gated tool");
                                        } else if !subs.approval_gate.as_ref().is_some_and(|g: &crate::tools::approval::ApprovalGate| g.requires_approval(&name)) {
                                            info!(tool = %name, mode = %agent_mode, category = ?tool_category, "Tool requires approval per agent mode");
                                            return (
                                                id,
                                                format!(
                                                    "Tool '{}' requires approval in {} mode (category: {}). Not executed.",
                                                    name, agent_mode, tool_category
                                                ),
                                                false,
                                            );
                                        }
                                        // Fall through to approval gate
                                    }
                                    crate::security::CategoryPermission::Allowed => {}
                                }
                            }
                        }

                        // Check approval gate before executing
                        if !trusted_local_session {
                            if let Some(ref gate) = subs.approval_gate {
                                if let Some(message) = resolve_tool_approval(
                                    gate,
                                    subs.approval_handler.as_ref(),
                                    &name,
                                    &args,
                                )
                                .await
                                {
                                    info!(tool = %name, "Tool requires approval, blocking execution");
                                    return (id, message, false);
                                }
                            }
                        }

                        // Dry-run mode: describe what would happen without executing
                        if dry_run {
                            return (id, dry_run_result(&name, &args, &raw_args, budget), false);
                        }

                        // Send tool starting feedback
                        if let Some(tx) = subs.tool_feedback_tx.read().await.as_ref() {
                            let _ = tx.send(ToolFeedback {
                                tool_name: name.clone(),
                                phase: ToolFeedbackPhase::Starting,
                                args_json: Some(raw_args.clone()),
                            });
                        }
                        #[cfg(feature = "panel")]
                        if let Some(ref bus) = subs.event_bus {
                            bus.send(crate::api::events::PanelEvent::ToolStarted {
                                tool: name.clone(),
                            });
                        }

                        let tool_start = std::time::Instant::now();
                        let tools_ref = Arc::clone(&subs.tools);
                        let safety_ref = subs.safety_layer.clone();
                        let taint_ref = subs.taint.clone();
                        let execution = std::panic::AssertUnwindSafe(async {
                            let tools_guard = tools_ref.read().await;
                            crate::kernel::execute_tool(
                                &tools_guard,
                                &name,
                                args,
                                &ctx,
                                safety_ref.as_ref().map(|s| s.as_ref()),
                                &subs.metrics_collector,
                                taint_ref.as_ref().map(|t| t.as_ref()),
                            )
                            .await
                        })
                        .catch_unwind();

                        let (result, success, tool_output) =
                            match tokio::time::timeout(tool_timeout, execution).await {
                                Ok(Ok(Ok(output))) => {
                                    let success = !output.is_error;
                                    let for_llm = output.for_llm.clone();
                                    (for_llm, success, Some(output))
                                }
                                Ok(Ok(Err(e))) => (format!("Error: {}", e), false, None),
                                Ok(Err(_panic)) => {
                                    error!(tool = %name, "Tool panicked during execution");
                                    (
                                        format!(
                                            "Error: Tool '{}' panicked during execution",
                                            name
                                        ),
                                        false,
                                        None,
                                    )
                                }
                                Err(_) => {
                                    error!(tool = %name, timeout_secs = tool_timeout.as_secs(), "Tool execution timed out");
                                    (
                                        format!(
                                            "Error: Tool '{}' timed out after {}s",
                                            name,
                                            tool_timeout.as_secs()
                                        ),
                                        false,
                                        None,
                                    )
                                }
                            };

                        let pause = tool_output.as_ref().is_some_and(|o| o.pause_for_input);
                        let elapsed = tool_start.elapsed();
                        let latency_ms = elapsed.as_millis() as u64;

                        // Send to user if tool opted in
                        if let Some(ref output) = tool_output {
                            if let Some(ref user_msg) = output.for_user {
                                let mut outbound = crate::bus::OutboundMessage::new(
                                    ctx.channel.as_deref().unwrap_or(""),
                                    ctx.chat_id.as_deref().unwrap_or(""),
                                    user_msg,
                                );
                                if let Some(tid) = inbound_meta.get("telegram_thread_id") {
                                    outbound
                                        .metadata
                                        .insert("telegram_thread_id".to_string(), tid.clone());
                                }
                                let bus_ref = &subs.bus;
                                let _ = bus_ref.publish_outbound(outbound).await;
                            }
                        }

                        if success {
                            debug!(tool = %name, latency_ms = latency_ms, "Tool executed successfully");
                            hooks.after_tool(&name, &result, elapsed, channel_name, chat_id);
                            if let Some(tx) = subs.tool_feedback_tx.read().await.as_ref() {
                                let _ = tx.send(ToolFeedback {
                                    tool_name: name.clone(),
                                    phase: ToolFeedbackPhase::Done {
                                        elapsed_ms: latency_ms,
                                    },
                                    args_json: Some(raw_args.clone()),
                                });
                            }
                            #[cfg(feature = "panel")]
                            if let Some(ref bus) = subs.event_bus {
                                bus.send(crate::api::events::PanelEvent::ToolDone {
                                    tool: name.clone(),
                                    duration_ms: latency_ms,
                                });
                            }
                        } else {
                            error!(tool = %name, latency_ms = latency_ms, error = %result, "Tool execution failed");
                            hooks.on_error(&name, &result, channel_name, chat_id);
                            {
                                let metrics = subs.usage_metrics.read().await;
                                metrics.record_error();
                            }
                            if let Some(tx) = subs.tool_feedback_tx.read().await.as_ref() {
                                let _ = tx.send(ToolFeedback {
                                    tool_name: name.clone(),
                                    phase: ToolFeedbackPhase::Failed {
                                        elapsed_ms: latency_ms,
                                        error: result.clone(),
                                    },
                                    args_json: Some(raw_args.clone()),
                                });
                            }
                            #[cfg(feature = "panel")]
                            if let Some(ref bus) = subs.event_bus {
                                bus.send(crate::api::events::PanelEvent::ToolFailed {
                                    tool: name.clone(),
                                    error: result.clone(),
                                });
                            }
                        }

                        // Sanitize the result with dynamic budget
                        let sanitized =
                            crate::utils::sanitize::sanitize_tool_result(&result, budget);

                        (id, sanitized, pause)
                    }
                })
                .collect();

            let results = if run_sequential {
                let mut out = Vec::with_capacity(tool_futures.len());
                for fut in tool_futures {
                    out.push(fut.await);
                }
                out
            } else {
                futures::future::join_all(tool_futures).await
            };

            // Record tool names for chain alerting
            let tool_names: Vec<String> = response
                .tool_calls
                .iter()
                .map(|tc| tc.name.clone())
                .collect();
            chain_tracker.record(&tool_names);

            let results: Vec<(String, String, bool)> = results;
            let should_pause = results.iter().any(|(_, _, pause)| *pause);
            for (id, result, _) in &results {
                session.add_message(Message::tool_result(id, result));
            }

            if should_pause {
                break;
            }

            // Increment tool call counter after execution.
            tool_call_limit.increment(response.tool_calls.len() as u32);
            // If the limit is now hit, make one final LLM call WITHOUT tools
            // so the model can synthesize the tool results into a proper answer.
            if tool_call_limit.is_exceeded() {
                info!(
                    count = tool_call_limit.count(),
                    limit = ?tool_call_limit.limit(),
                    "Tool call limit reached, making final synthesis call"
                );
                // Respect token budget — skip the synthesis call if already over.
                if token_budget.is_exceeded() {
                    info!(budget = %token_budget.summary(), "Token budget also exceeded, skipping synthesis call");
                    response.content =
                        "Tool call limit reached. Token budget exceeded.".to_string();
                    break;
                }
                let messages =
                    build_resolved_messages(&subsystems, session, memory_override.as_deref()).await;
                response = provider
                    .chat(messages, vec![], model, options.clone())
                    .await?;
                record_usage(&subsystems, response.usage.as_ref(), token_budget);
                break;
            }

            if let Some(guard) = loop_guard.as_mut() {
                if check_loop_guard(guard, &response.tool_calls, session) {
                    response.content =
                        "Stopped tool loop due to repeated tool-call pattern.".to_string();
                    break;
                }

                // Record outcomes for outcome-aware blocking.
                let results_for_guard: Vec<(String, String)> = results
                    .iter()
                    .map(|(id, r, _)| (id.clone(), r.clone()))
                    .collect();
                if check_loop_guard_outcomes(
                    guard,
                    &response.tool_calls,
                    &results_for_guard,
                    session,
                ) {
                    response.content =
                        "Stopped tool loop due to repeated identical outcomes.".to_string();
                    break;
                }
            }

            // Get fresh tool definitions for the next LLM call
            let tool_definitions = {
                let tools = subsystems.tools.read().await;
                tools.definitions_with_options(config.agents.defaults.compact_tools)
            };

            // Check token budget before next LLM call
            if token_budget.is_exceeded() {
                info!(budget = %token_budget.summary(), "Token budget exceeded during tool loop");
                break;
            }

            // Call LLM again with tool results
            let messages =
                build_resolved_messages(&subsystems, session, memory_override.as_deref()).await;

            send_feedback(
                &subsystems,
                ToolFeedback {
                    tool_name: String::new(),
                    phase: ToolFeedbackPhase::Thinking,
                    args_json: None,
                },
            )
            .await;

            response = provider
                .chat(messages, tool_definitions, model, options.clone())
                .await?;

            send_feedback(
                &subsystems,
                ToolFeedback {
                    tool_name: String::new(),
                    phase: ToolFeedbackPhase::ThinkingDone,
                    args_json: None,
                },
            )
            .await;

            record_usage(&subsystems, response.usage.as_ref(), token_budget);
        }

        if iteration >= max_iterations && response.has_tool_calls() {
            info!(
                iterations = iteration,
                "Tool loop reached maximum iterations, returning partial response"
            );
        }

        // Signal that tools are done and response is ready
        send_feedback(
            &subsystems,
            ToolFeedback {
                tool_name: String::new(),
                phase: ToolFeedbackPhase::ResponseReady,
                args_json: None,
            },
        )
        .await;

        // Add final assistant response
        session.add_message(Message::assistant(&response.content));

        Ok(PipelineOutput::Sync {
            response: response.content,
            usage: response.usage,
            cached: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::middleware::test_helpers::*;
    use crate::agent::pipeline::Pipeline;
    use crate::providers::{ChatOptions, LLMProvider, LLMResponse, ToolDefinition};
    use crate::session::Session;

    #[derive(Debug)]
    struct SimpleProvider {
        response: String,
    }

    #[async_trait]
    impl LLMProvider for SimpleProvider {
        fn name(&self) -> &str {
            "test"
        }
        fn default_model(&self) -> &str {
            "test-model"
        }
        async fn chat(
            &self,
            _messages: Vec<Message>,
            _tools: Vec<ToolDefinition>,
            _model: Option<&str>,
            _options: ChatOptions,
        ) -> Result<LLMResponse> {
            Ok(LLMResponse::text(&self.response))
        }
    }

    #[tokio::test]
    async fn core_loop_basic_response() {
        let subsystems = test_subsystems();
        let core = CoreLoop::new();
        let pipeline = Pipeline::builder().build(core);

        let mut ctx = test_context(Arc::clone(&subsystems));
        // Set up the fields that upstream middlewares would populate.
        ctx.provider = Some(Arc::new(SimpleProvider {
            response: "hello world".to_string(),
        }));
        ctx.model = Some("test-model".to_string());
        ctx.messages = Some(vec![
            Message::system("You are a helper."),
            Message::user("Hi"),
        ]);
        ctx.tool_definitions = Some(vec![]);
        ctx.chat_options = Some(ChatOptions::new());
        ctx.session = Some(Session::new("test"));

        let output = pipeline.execute(&mut ctx).await.unwrap();
        assert_eq!(output.response(), Some("hello world"));
        assert!(!output.is_cached());
    }

    #[tokio::test]
    async fn core_loop_adds_final_assistant_message_to_session() {
        let subsystems = test_subsystems();
        let core = CoreLoop::new();
        let pipeline = Pipeline::builder().build(core);

        let mut ctx = test_context(Arc::clone(&subsystems));
        ctx.provider = Some(Arc::new(SimpleProvider {
            response: "final answer".to_string(),
        }));
        ctx.model = Some("test-model".to_string());
        ctx.messages = Some(vec![Message::user("Q")]);
        ctx.tool_definitions = Some(vec![]);
        ctx.chat_options = Some(ChatOptions::new());
        ctx.session = Some(Session::new("test"));

        let _ = pipeline.execute(&mut ctx).await.unwrap();
        let session = ctx.session.as_ref().unwrap();
        let last = session.messages.last().unwrap();
        assert_eq!(last.role, Role::Assistant);
        assert_eq!(last.content, "final answer");
    }

    #[tokio::test]
    async fn core_loop_debug_impl() {
        let subsystems = test_subsystems();
        let _subsystems = subsystems;
        let core = CoreLoop::new();
        let debug_str = format!("{:?}", core);
        assert!(debug_str.contains("CoreLoop"));
    }
}
