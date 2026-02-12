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
                .with_extra_mounts(extra_mounts);

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

            if !runtime.is_available().await {
                return Err(RuntimeError::NotAvailable(
                    "Docker is not installed or not running".to_string(),
                ));
            }

            Ok(Arc::new(runtime))
        }
        RuntimeType::AppleContainer => {
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
}
