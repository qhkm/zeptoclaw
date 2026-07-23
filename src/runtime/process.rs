//! Shared subprocess environment and lifecycle hardening for runtimes.

use std::ffi::OsStr;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

use super::types::{CommandOutput, RuntimeError, RuntimeResult};

#[cfg(unix)]
const TERMINATION_GRACE: Duration = Duration::from_millis(250);

/// Replace inherited environment entries with a scrubbed snapshot, then apply
/// explicitly configured values. Names in `passthrough` opt back into
/// inheritance, including names that match the sensitive-name filter.
pub(crate) fn configure_environment(
    command: &mut Command,
    explicit: &[(String, String)],
    passthrough: &[String],
) {
    command.env_clear();

    for (name, value) in std::env::vars_os() {
        if should_inherit(&name, passthrough) {
            command.env(name, value);
        }
    }

    // Explicit command-scoped values always win over inherited values.
    for (name, value) in explicit {
        command.env(name, value);
    }
}

/// Read explicitly allowed inherited variables for container CLI arguments.
pub(crate) fn passthrough_values(names: &[String]) -> Vec<(String, String)> {
    names
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|value| (name.clone(), value)))
        .collect()
}

fn should_inherit(name: &OsStr, passthrough: &[String]) -> bool {
    let name = name.to_string_lossy();
    passthrough
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&name))
        || !is_sensitive_name(&name)
}

fn is_sensitive_name(name: &str) -> bool {
    const SENSITIVE_SEGMENTS: &[&str] = &[
        "AUTH",
        "COOKIE",
        "CREDENTIAL",
        "CREDENTIALS",
        "KEY",
        "PASS",
        "PASSWD",
        "PASSWORD",
        "SECRET",
        "SESSION",
        "TOKEN",
    ];

    let upper = name.to_ascii_uppercase();
    if upper.ends_with("_API_BASE")
        || upper.ends_with("_BASE_URL")
        || upper.ends_with("DATABASE_URL")
        || upper.ends_with("DB_URL")
        || upper.ends_with("_DSN")
        || upper.contains("SERVICE_ACCOUNT")
    {
        return true;
    }

    upper
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|segment| !segment.is_empty())
        .any(|segment| {
            SENSITIVE_SEGMENTS
                .iter()
                .any(|sensitive| segment.eq_ignore_ascii_case(sensitive))
        })
}

/// Spawn a command, collect its output, and enforce cleanup on timeout.
///
/// Unix children are placed in a new process group. On timeout the whole group
/// receives SIGTERM followed by SIGKILL after a short grace period, and the
/// leader is reaped before this function returns. Other platforms terminate
/// and reap the direct child.
pub(crate) async fn run_command(
    mut command: Command,
    timeout_secs: u64,
) -> RuntimeResult<CommandOutput> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|error| RuntimeError::ExecutionFailed(error.to_string()))?;
    let process_id = child.id();
    let stdout = child.stdout.take().ok_or_else(|| {
        RuntimeError::ExecutionFailed("failed to capture command stdout".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        RuntimeError::ExecutionFailed("failed to capture command stderr".to_string())
    })?;

    let timeout = Duration::from_secs(timeout_secs);
    match tokio::time::timeout(timeout, collect_output(&mut child, stdout, stderr)).await {
        Ok(result) => result,
        Err(_) => {
            terminate_and_reap(&mut child, process_id).await;
            Err(RuntimeError::Timeout(timeout_secs))
        }
    }
}

async fn collect_output<Out, Err>(
    child: &mut Child,
    stdout: Out,
    stderr: Err,
) -> RuntimeResult<CommandOutput>
where
    Out: AsyncRead + Unpin,
    Err: AsyncRead + Unpin,
{
    let (status, stdout, stderr) =
        tokio::try_join!(child.wait(), read_to_end(stdout), read_to_end(stderr),)
            .map_err(|error| RuntimeError::ExecutionFailed(error.to_string()))?;

    Ok(CommandOutput::new(
        String::from_utf8_lossy(&stdout).to_string(),
        String::from_utf8_lossy(&stderr).to_string(),
        status.code(),
    ))
}

async fn read_to_end<R: AsyncRead + Unpin>(mut reader: R) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).await?;
    Ok(bytes)
}

async fn terminate_and_reap(child: &mut Child, process_id: Option<u32>) {
    #[cfg(unix)]
    if let Some(process_id) = process_id {
        signal_process_group(process_id, libc::SIGTERM);
        tokio::time::sleep(TERMINATION_GRACE).await;
        signal_process_group(process_id, libc::SIGKILL);
    }

    // This is also the fallback when process-group signaling is unavailable or
    // the child exited before its process group could be signaled.
    if let Err(error) = child.start_kill() {
        if error.kind() != std::io::ErrorKind::InvalidInput {
            tracing::warn!(%error, "failed to terminate timed-out runtime child");
        }
    }
    if let Err(error) = child.wait().await {
        tracing::warn!(%error, "failed to reap timed-out runtime child");
    }
}

#[cfg(unix)]
fn signal_process_group(process_id: u32, signal: i32) {
    let Ok(process_group) = i32::try_from(process_id) else {
        tracing::warn!(
            process_id,
            "runtime child PID does not fit in a Unix process-group ID"
        );
        return;
    };

    // SAFETY: `kill` is called with a negative, validated child PID to target
    // the process group created for this command. No pointers are involved.
    let result = unsafe { libc::kill(-process_group, signal) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            tracing::warn!(%error, process_id, signal, "failed to signal runtime process group");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_environment_names_are_scrubbed() {
        for name in [
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "AWS_SECRET_ACCESS_KEY",
            "SSH_AUTH_SOCK",
            "ZEPTOCLAW_MASTER_KEY",
            "SERVICE_PASSWORD",
            "OPENAI_API_BASE",
            "DATABASE_URL",
            "SENTRY_DSN",
            "GOOGLE_SERVICE_ACCOUNT_BASE64",
        ] {
            assert!(is_sensitive_name(name), "expected {name} to be sensitive");
        }

        for name in ["PATH", "HOME", "LANG", "RUST_LOG"] {
            assert!(
                !is_sensitive_name(name),
                "expected {name} to remain non-sensitive"
            );
        }
    }

    #[tokio::test]
    async fn sensitive_environment_is_removed() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "printf '%s' \"${ZEPTOCLAW_TEST_SECRET_TOKEN-unset}\""])
            .env("ZEPTOCLAW_TEST_SECRET_TOKEN", "inherited");
        configure_environment(&mut command, &[], &[]);

        let output = run_command(command, 5).await.unwrap();
        assert_eq!(output.stdout, "unset");
    }

    #[tokio::test]
    async fn configured_environment_replaces_scrubbed_value() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "printf '%s' \"$ZEPTOCLAW_TEST_SECRET_TOKEN\""])
            .env("ZEPTOCLAW_TEST_SECRET_TOKEN", "inherited");
        configure_environment(
            &mut command,
            &[(
                "ZEPTOCLAW_TEST_SECRET_TOKEN".to_string(),
                "explicit".to_string(),
            )],
            &[],
        );

        let output = run_command(command, 5).await.unwrap();
        assert_eq!(output.stdout, "explicit");
    }

    #[test]
    fn sensitive_environment_can_be_explicitly_passed_through() {
        let name = "ZEPTOCLAW_PROCESS_TEST_TOKEN";
        assert!(should_inherit(OsStr::new(name), &[name.to_string()]));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_terminates_descendant_process_group() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("descendant.pid");
        let mut command = Command::new("sh");
        command.args([
            "-c",
            "trap '' TERM; sleep 30 & echo $! > \"$PID_FILE\"; wait",
        ]);
        configure_environment(
            &mut command,
            &[(
                "PID_FILE".to_string(),
                pid_file.to_string_lossy().to_string(),
            )],
            &[],
        );

        let result = run_command(command, 1).await;
        assert!(matches!(result, Err(RuntimeError::Timeout(1))));

        let process_id = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        for _ in 0..20 {
            if !process_exists(process_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed-out descendant process {process_id} survived cleanup");
    }

    #[cfg(unix)]
    fn process_exists(process_id: i32) -> bool {
        // SAFETY: signal 0 performs an existence/permission check only.
        let result = unsafe { libc::kill(process_id, 0) };
        if result == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
