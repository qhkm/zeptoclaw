//! Batch command handler.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};

use zeptoclaw::batch::{format_results, load_prompts, BatchOutputFormat, BatchResult};
use zeptoclaw::bus::{InboundMessage, MessageBus};
use zeptoclaw::config::Config;
use zeptoclaw::providers::StreamEvent;

use super::common::{create_agent, create_agent_with_template, resolve_template};
use super::BatchFormat;

/// Process prompts from a file.
pub(crate) async fn cmd_batch(
    input: PathBuf,
    output: Option<PathBuf>,
    format: BatchFormat,
    stop_on_error: bool,
    stream: bool,
    template: Option<String>,
) -> Result<()> {
    let prompts = load_prompts(&input).with_context(|| {
        format!(
            "Failed to load prompts from batch input file {}",
            input.display()
        )
    })?;

    let config = Config::load().with_context(|| "Failed to load configuration")?;
    let use_streaming = stream || config.agents.defaults.streaming;

    let bus = Arc::new(MessageBus::new());
    let agent = if let Some(name) = template.as_deref() {
        let tpl = resolve_template(name)?;
        create_agent_with_template(config, bus, Some(tpl)).await?
    } else {
        create_agent(config, bus).await?
    };

    let mut results = Vec::with_capacity(prompts.len());
    let mut failed = 0usize;

    for (index, prompt) in prompts.into_iter().enumerate() {
        let start = Instant::now();
        let inbound = InboundMessage::new("cli", "batch", &format!("batch-{}", index), &prompt);

        let response = if use_streaming {
            process_streaming(&agent, &inbound).await
        } else {
            agent
                .process_message(&inbound)
                .await
                .map_err(anyhow::Error::from)
        };

        let duration_ms = start.elapsed().as_millis() as u64;
        match response {
            Ok(content) => results.push(BatchResult {
                index,
                prompt,
                response: Some(content),
                error: None,
                duration_ms,
            }),
            Err(err) => {
                failed += 1;
                results.push(BatchResult {
                    index,
                    prompt,
                    response: None,
                    error: Some(err.to_string()),
                    duration_ms,
                });
                if stop_on_error {
                    break;
                }
            }
        }
    }

    let output_format = match format {
        BatchFormat::Text => BatchOutputFormat::Text,
        BatchFormat::Jsonl => BatchOutputFormat::Jsonl,
    };
    let rendered = format_results(&results, &output_format);

    if let Some(path) = output {
        std::fs::write(&path, rendered)
            .with_context(|| format!("Failed to write batch output to {}", path.display()))?;
        println!(
            "Wrote {} result(s) to {}",
            results.len(),
            path.as_path().display()
        );
    } else {
        println!("{}", rendered);
    }

    if failed > 0 {
        anyhow::bail!("{} prompt(s) failed during batch processing", failed);
    }

    Ok(())
}

async fn process_streaming(
    agent: &Arc<zeptoclaw::agent::AgentLoop>,
    inbound: &InboundMessage,
) -> Result<String> {
    let mut response = String::new();
    let mut rx = agent.process_message_streaming(inbound).await?;
    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Delta(chunk) => response.push_str(&chunk),
            StreamEvent::Done { .. } => break,
            StreamEvent::Error(err) => anyhow::bail!("stream error: {}", err),
            StreamEvent::ToolCalls(_) => {}
        }
    }
    Ok(response)
}
