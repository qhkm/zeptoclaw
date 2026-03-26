//! Thin kernel — coordination, not orchestration.
//!
//! Owns subsystems, wires them once at boot, provides entry points.
//! See `docs/plans/2026-03-03-thin-kernel-design.md` for design rationale.

mod gate;
pub mod provider;
mod registrar;

pub use gate::execute_tool;
pub use provider::build_provider_chain;
pub use registrar::ToolFilter;

use std::sync::Arc;

use tracing::{info, warn};

use crate::bus::MessageBus;
use crate::config::{Config, MemoryBackend};
use crate::cron::CronService;
use crate::hands::HandManifest;
use crate::hooks::HookEngine;
use crate::memory::factory::create_searcher_with_provider;
use crate::memory::longterm::LongTermMemory;
use crate::providers::LLMProvider;
use crate::runtime::{create_runtime, ContainerRuntime, NativeRuntime};
use crate::safety::taint::TaintEngine;
use crate::safety::SafetyLayer;
use crate::tools::mcp::client::McpClient;
use crate::tools::ToolRegistry;
use crate::utils::metrics::MetricsCollector;

/// The thin kernel — coordination, not orchestration.
///
/// Owns subsystems assembled at boot, shared across agent sessions.
/// Does NOT own per-session state (AgentLoop, ContextBuilder, SessionManager).
pub struct ZeptoKernel {
    /// Immutable configuration snapshot.
    pub config: Arc<Config>,
    /// Assembled provider chain (base → fallback → retry → quota).
    /// `None` when no runtime provider is configured (plugin-only setups).
    pub provider: Option<Arc<dyn LLMProvider>>,
    /// All registered tools (built-in + MCP + plugins + composed).
    pub tools: ToolRegistry,
    /// Safety layer for injection/leak/policy checks. `None` when disabled.
    pub safety: Option<SafetyLayer>,
    /// Per-tool call stats and token tracking.
    pub metrics: Arc<MetricsCollector>,
    /// Config-driven hooks (before_tool, after_tool, on_error).
    pub hooks: Arc<HookEngine>,
    /// MCP clients tracked for graceful shutdown (kill stdio child processes).
    pub mcp_clients: Vec<Arc<McpClient>>,
    /// Shared long-term memory for both per-message injection and tool access.
    pub ltm: Option<Arc<tokio::sync::Mutex<LongTermMemory>>>,
    /// Taint tracking engine for data-flow-aware security.
    /// `None` when taint tracking is disabled.
    pub taint: Option<Arc<std::sync::RwLock<TaintEngine>>>,
}

impl ZeptoKernel {
    /// Boot a kernel from config, assembling all shared subsystems.
    ///
    /// This replaces the first ~600 lines of `create_agent_with_template()` that
    /// build config-driven subsystems. Per-session state (AgentLoop, SpawnTool,
    /// DelegateTool, model-switch registry) is NOT created here — see
    /// `create_agent_with_template()` which consumes the kernel.
    pub async fn boot(
        config: Config,
        bus: Arc<MessageBus>,
        template: Option<&crate::config::templates::AgentTemplate>,
        hand: Option<&HandManifest>,
    ) -> anyhow::Result<Self> {
        // 1. Build tool filter from config/template/hand
        let filter = ToolFilter::from_config(&config, template, hand);

        // 2. Build provider chain
        let provider: Option<Arc<dyn LLMProvider>> =
            if let Some((chain, names)) = provider::build_provider_chain(&config).await {
                let chain_label = names.join(" -> ");
                info!(
                    provider_chain = %chain_label,
                    "Assembled provider chain"
                );
                Some(chain)
            } else {
                // No runtime provider — plugin providers may be set later by
                // `create_agent_with_template()`.
                None
            };

        // 3. Safety layer
        let safety = if config.safety.enabled {
            Some(SafetyLayer::new(config.safety.clone()))
        } else {
            None
        };

        // 3b. Taint engine (data-flow tracking)
        let taint = if config.safety.enabled && config.safety.taint.enabled {
            Some(Arc::new(std::sync::RwLock::new(TaintEngine::new(
                config.safety.taint.clone(),
            ))))
        } else {
            None
        };

        // 4. Metrics + hooks
        let metrics = Arc::new(MetricsCollector::new());
        let hooks = Arc::new(HookEngine::new(config.hooks.clone()));

        // 5. Memory searcher + shared LTM
        let embedding_provider: Option<Arc<dyn LLMProvider>> =
            if matches!(config.memory.backend, MemoryBackend::Embedding) {
                provider::build_runtime_provider_chain(&config)
                    .await
                    .map(|(chain, _)| Arc::from(chain))
            } else {
                None
            };
        let memory_searcher = create_searcher_with_provider(&config.memory, embedding_provider);

        let ltm: Option<Arc<tokio::sync::Mutex<LongTermMemory>>> =
            if !matches!(config.memory.backend, MemoryBackend::Disabled) {
                let ltm_path = Config::dir().join("memory").join("longterm.json");
                match LongTermMemory::with_path_and_searcher(ltm_path, memory_searcher.clone()) {
                    Ok(ltm) => Some(Arc::new(tokio::sync::Mutex::new(ltm))),
                    Err(e) => {
                        warn!("Failed to load long-term memory: {}", e);
                        None
                    }
                }
            } else {
                None
            };

        // 6. Container runtime
        let runtime: Arc<dyn ContainerRuntime> = match create_runtime(&config.runtime).await {
            Ok(r) => {
                info!("Using {} runtime for shell commands", r.name());
                r
            }
            Err(e) => {
                if config.runtime.allow_fallback_to_native {
                    warn!(
                        "Failed to create configured runtime: {}. Falling back to native.",
                        e
                    );
                    Arc::new(NativeRuntime::new())
                } else {
                    return Err(anyhow::anyhow!(
                        "Configured runtime '{:?}' unavailable: {}. \
                         Enable runtime.allow_fallback_to_native to opt in to native fallback.",
                        config.runtime.runtime_type,
                        e
                    ));
                }
            }
        };

        // 7. Cron service
        let cron_store_path = Config::dir().join("cron").join("jobs.json");
        let cron_service = Arc::new(CronService::with_jitter(
            cron_store_path,
            bus.clone(),
            config.routines.jitter_ms,
        ));
        cron_service.start(&config.routines.on_miss).await?;

        // 8. Register all tools
        let mut tools = ToolRegistry::new();
        let deps = registrar::ToolDeps {
            runtime,
            bus,
            cron_service,
            memory_searcher,
            shared_ltm: ltm.clone(),
            template: template.cloned(),
        };
        let (mcp_clients, external_tool_names) =
            registrar::register_all_tools(&mut tools, &config, &filter, &deps).await?;

        // Feed external tool names into the taint engine for default-deny labeling
        if let Some(ref taint_lock) = taint {
            if let Ok(mut engine) = taint_lock.write() {
                engine.register_external_tools(external_tool_names);
            }
        }

        info!("Kernel boot: {} tools registered", tools.len());

        Ok(ZeptoKernel {
            config: Arc::new(config),
            provider,
            tools,
            safety,
            metrics,
            hooks,
            mcp_clients,
            ltm,
            taint,
        })
    }

    /// Get tool definitions for MCP server / OpenAI API exposure.
    pub fn tool_definitions(&self) -> Vec<crate::providers::ToolDefinition> {
        self.tools.definitions()
    }

    /// Get the provider for OpenAI-compat API pass-through.
    pub fn provider(&self) -> Option<Arc<dyn LLMProvider>> {
        self.provider.as_ref().map(Arc::clone)
    }

    /// Graceful shutdown: close MCP stdio clients.
    pub async fn shutdown(&self) {
        for client in &self.mcp_clients {
            let _ = client.shutdown().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::EchoTool;
    use crate::utils::metrics::MetricsCollector;

    /// Helper: build a minimal kernel for testing (no real provider).
    fn test_kernel() -> ZeptoKernel {
        let config = Config::default();
        let mut tools = ToolRegistry::new();
        tools.register(Box::new(EchoTool));

        ZeptoKernel {
            config: Arc::new(config.clone()),
            provider: Some(Arc::new(crate::providers::ClaudeProvider::new("test-key"))),
            tools,
            safety: if config.safety.enabled {
                Some(SafetyLayer::new(config.safety.clone()))
            } else {
                None
            },
            metrics: Arc::new(MetricsCollector::new()),
            hooks: Arc::new(HookEngine::new(config.hooks.clone())),
            mcp_clients: vec![],
            ltm: None,
            taint: if config.safety.enabled && config.safety.taint.enabled {
                Some(Arc::new(std::sync::RwLock::new(TaintEngine::new(
                    config.safety.taint.clone(),
                ))))
            } else {
                None
            },
        }
    }

    #[test]
    fn test_kernel_struct_holds_all_subsystems() {
        let kernel = test_kernel();
        assert!(!kernel.tools.is_empty());
        assert!(kernel.tools.has("echo"));
        assert!(kernel.mcp_clients.is_empty());
        assert!(kernel.ltm.is_none());
    }

    #[test]
    fn test_kernel_tool_definitions_delegates_to_registry() {
        let kernel = test_kernel();
        let defs = kernel.tool_definitions();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "echo");
    }

    #[test]
    fn test_kernel_provider_returns_arc_clone() {
        let kernel = test_kernel();
        let p1 = kernel.provider().expect("test kernel has provider");
        let p2 = kernel.provider().expect("test kernel has provider");
        // Both are valid Arc clones (can't compare trait objects, but both usable)
        assert_eq!(p1.name(), p2.name());
    }

    #[test]
    fn test_kernel_safety_none_when_disabled() {
        let mut config = Config::default();
        config.safety.enabled = false;
        let kernel = ZeptoKernel {
            config: Arc::new(config.clone()),
            provider: Some(Arc::new(crate::providers::ClaudeProvider::new("test-key"))),
            tools: ToolRegistry::new(),
            safety: None,
            metrics: Arc::new(MetricsCollector::new()),
            hooks: Arc::new(HookEngine::new(config.hooks.clone())),
            mcp_clients: vec![],
            ltm: None,
            taint: None,
        };
        assert!(kernel.safety.is_none());
    }

    #[test]
    fn test_kernel_safety_some_when_enabled() {
        let mut config = Config::default();
        config.safety.enabled = true;
        let kernel = ZeptoKernel {
            config: Arc::new(config.clone()),
            provider: Some(Arc::new(crate::providers::ClaudeProvider::new("test-key"))),
            tools: ToolRegistry::new(),
            safety: Some(SafetyLayer::new(config.safety.clone())),
            metrics: Arc::new(MetricsCollector::new()),
            hooks: Arc::new(HookEngine::new(config.hooks.clone())),
            mcp_clients: vec![],
            ltm: None,
            taint: Some(Arc::new(std::sync::RwLock::new(TaintEngine::new(
                config.safety.taint.clone(),
            )))),
        };
        assert!(kernel.safety.is_some());
    }

    #[tokio::test]
    async fn test_kernel_shutdown_empty_clients() {
        let kernel = test_kernel();
        // Should not panic with empty client list
        kernel.shutdown().await;
    }
}
