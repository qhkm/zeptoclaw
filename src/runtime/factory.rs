//! Runtime factory for creating container runtimes from configuration

use std::sync::Arc;

use crate::config::{RuntimeConfig, RuntimeType};
use crate::security::validate_extra_mounts;

use super::docker::DockerRuntime;
use super::native::NativeRuntime;
use super::types::{ContainerRuntime, RuntimeError, RuntimeResult};

#[cfg(target_os = "macos")]
use super::apple::AppleContainerRuntime;

/// Create a container runtime from configuration
pub async fn create_runtime(config: &RuntimeConfig) -> RuntimeResult<Arc<dyn ContainerRuntime>> {
    match config.runtime_type {
        RuntimeType::Native => Ok(Arc::new(NativeRuntime::new())),
        RuntimeType::Docker => {
            let extra_mounts =
                validate_extra_mounts(&config.docker.extra_mounts, &config.mount_allowlist_path)
                    .map_err(|e| RuntimeError::NotAvailable(e.to_string()))?;

            let runtime = DockerRuntime::new(&config.docker.image)
                .with_network(&config.docker.network)
                .with_extra_mounts(extra_mounts)
                .with_stop_timeout(config.docker.stop_timeout_secs);

            let runtime = if let Some(ref mem) = config.docker.memory_limit {
                runtime.with_memory_limit(mem)
            } else {
                runtime
            };

            let runtime = if let Some(ref cpu) = config.docker.cpu_limit {
                runtime.with_cpu_limit(cpu)
            } else {
                runtime
            };

            let runtime = if let Some(pids) = config.docker.pids_limit {
                runtime.with_pids_limit(pids)
            } else {
                runtime
            };

            if !runtime.is_available().await {
                return Err(RuntimeError::NotAvailable(
                    "Docker is not installed or not running".to_string(),
                ));
            }

            Ok(Arc::new(runtime))
        }
        RuntimeType::AppleContainer => {
            if !config.apple.allow_experimental {
                return Err(RuntimeError::NotAvailable(
                    "Apple Container runtime is experimental. Set `allow_experimental: true` in runtime.apple config or ZEPTOCLAW_RUNTIME_APPLE_ALLOW_EXPERIMENTAL=true to enable.".to_string(),
                ));
            }
            #[cfg(target_os = "macos")]
            {
                let extra_mounts =
                    validate_extra_mounts(&config.apple.extra_mounts, &config.mount_allowlist_path)
                        .map_err(|e| RuntimeError::NotAvailable(e.to_string()))?;

                let runtime = if config.apple.image.is_empty() {
                    AppleContainerRuntime::new()
                } else {
                    AppleContainerRuntime::with_image(&config.apple.image)
                };

                let runtime = runtime.with_extra_mounts(extra_mounts);

                if !runtime.is_available().await {
                    return Err(RuntimeError::NotAvailable(
                        "Apple Container is not available (requires macOS 15+)".to_string(),
                    ));
                }

                Ok(Arc::new(runtime))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(RuntimeError::NotAvailable(
                    "Apple Container is only available on macOS".to_string(),
                ))
            }
        }

        #[cfg(target_os = "linux")]
        RuntimeType::Landlock => {
            use super::landlock::LandlockRuntime;
            Ok(Arc::new(LandlockRuntime::new(config.landlock.clone())))
        }

        #[cfg(target_os = "linux")]
        RuntimeType::Firejail => create_firejail_runtime(config).await,

        #[cfg(target_os = "linux")]
        RuntimeType::Bubblewrap => create_bubblewrap_runtime(config).await,
    }
}

/// Helper to create the Firejail runtime — split out to avoid `clippy::needless_return`
/// from the `#[cfg(not(feature))] return` / `#[cfg(feature)] { ... }` pattern.
#[cfg(target_os = "linux")]
async fn create_firejail_runtime(
    config: &RuntimeConfig,
) -> RuntimeResult<Arc<dyn ContainerRuntime>> {
    #[cfg(not(feature = "sandbox-firejail"))]
    {
        let _ = config;
        Err(RuntimeError::NotAvailable(
            "Recompile with --features sandbox-firejail to use the Firejail runtime.".to_string(),
        ))
    }
    #[cfg(feature = "sandbox-firejail")]
    {
        use super::firejail::FirejailRuntime;
        let runtime = FirejailRuntime::new(config.firejail.clone());
        if !runtime.is_available().await {
            return Err(RuntimeError::NotAvailable(
                "firejail binary not found on PATH. Install with: apt install firejail".to_string(),
            ));
        }
        Ok(Arc::new(runtime))
    }
}

/// Helper to create the Bubblewrap runtime — split out for the same reason as Firejail.
#[cfg(target_os = "linux")]
async fn create_bubblewrap_runtime(
    config: &RuntimeConfig,
) -> RuntimeResult<Arc<dyn ContainerRuntime>> {
    #[cfg(not(feature = "sandbox-bubblewrap"))]
    {
        let _ = config;
        Err(RuntimeError::NotAvailable(
            "Recompile with --features sandbox-bubblewrap to use the Bubblewrap runtime."
                .to_string(),
        ))
    }
    #[cfg(feature = "sandbox-bubblewrap")]
    {
        use super::bubblewrap::BubblewrapRuntime;
        let runtime = BubblewrapRuntime::new(config.bubblewrap.clone());
        if !runtime.is_available().await {
            return Err(RuntimeError::NotAvailable(
                "bwrap binary not found on PATH. Install with: apt install bubblewrap".to_string(),
            ));
        }
        Ok(Arc::new(runtime))
    }
}

/// Check which runtimes are available on this system
pub async fn available_runtimes() -> Vec<&'static str> {
    let mut available = vec!["native"]; // Always available

    // Check Docker
    let docker = DockerRuntime::default();
    if docker.is_available().await {
        available.push("docker");
    }

    // Check Apple Container (macOS only)
    #[cfg(target_os = "macos")]
    {
        let apple = AppleContainerRuntime::default();
        if apple.is_available().await {
            available.push("apple");
        }
    }

    // Linux sandbox runtimes
    #[cfg(all(target_os = "linux", feature = "sandbox-landlock"))]
    available.push("landlock");

    #[cfg(all(target_os = "linux", feature = "sandbox-firejail"))]
    {
        use super::firejail::FirejailRuntime;
        use crate::config::FirejailConfig;
        if FirejailRuntime::new(FirejailConfig::default())
            .is_available()
            .await
        {
            available.push("firejail");
        }
    }

    #[cfg(all(target_os = "linux", feature = "sandbox-bubblewrap"))]
    {
        use super::bubblewrap::BubblewrapRuntime;
        use crate::config::BubblewrapConfig;
        if BubblewrapRuntime::new(BubblewrapConfig::default())
            .is_available()
            .await
        {
            available.push("bubblewrap");
        }
    }

    available
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_native_runtime() {
        let config = RuntimeConfig::default();
        let runtime = create_runtime(&config).await.unwrap();
        assert_eq!(runtime.name(), "native");
    }

    #[tokio::test]
    async fn test_available_runtimes_includes_native() {
        let available = available_runtimes().await;
        assert!(available.contains(&"native"));
    }

    #[tokio::test]
    async fn test_create_apple_container_blocked_by_default() {
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::AppleContainer;
        // allow_experimental defaults to false
        assert!(!config.apple.allow_experimental);

        let result = create_runtime(&config).await;
        assert!(result.is_err());
        let err_text = result.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err_text.contains("experimental"));
    }

    #[test]
    fn test_apple_container_config_default_not_experimental() {
        use crate::config::AppleContainerConfig;
        let config = AppleContainerConfig::default();
        assert!(!config.allow_experimental);
    }

    #[test]
    fn test_apple_container_config_deserialize_experimental() {
        use crate::config::AppleContainerConfig;
        let json = r#"{"allow_experimental": true}"#;
        let config: AppleContainerConfig = serde_json::from_str(json).expect("should parse");
        assert!(config.allow_experimental);
    }

    #[tokio::test]
    async fn test_create_docker_runtime_with_extra_mounts_requires_allowlist() {
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Docker;
        config.mount_allowlist_path = "/nonexistent/allowlist.json".to_string();
        config
            .docker
            .extra_mounts
            .push("/tmp:/workspace/tmp".to_string());

        let result = create_runtime(&config).await;
        assert!(result.is_err());
        let err_text = result.err().map(|err| err.to_string()).unwrap_or_default();
        assert!(err_text.contains("allowlist"));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn test_create_landlock_runtime() {
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Landlock;
        let result = create_runtime(&config).await;
        assert!(
            result.is_ok(),
            "Landlock runtime should always be creatable"
        );
        assert_eq!(result.unwrap().name(), "landlock");
    }

    #[cfg(all(target_os = "linux", not(feature = "sandbox-firejail")))]
    #[tokio::test]
    async fn test_create_firejail_without_feature_fails() {
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Firejail;
        let result = create_runtime(&config).await;
        let err = result.err().expect("should be an error");
        assert!(err.to_string().contains("sandbox-firejail"));
    }

    #[cfg(all(target_os = "linux", not(feature = "sandbox-bubblewrap")))]
    #[tokio::test]
    async fn test_create_bubblewrap_without_feature_fails() {
        let mut config = RuntimeConfig::default();
        config.runtime_type = RuntimeType::Bubblewrap;
        let result = create_runtime(&config).await;
        let err = result.err().expect("should be an error");
        assert!(err.to_string().contains("sandbox-bubblewrap"));
    }
}
