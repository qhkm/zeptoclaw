//! Cron service for scheduling background agent turns.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use chrono::{DateTime, Datelike, Duration, Timelike, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{error, info};
use uuid::Uuid;

use crate::bus::{InboundMessage, MessageBus};
use crate::error::{Result, ZeptoError};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CronSchedule {
    At { at_ms: i64 },
    Every { every_ms: i64 },
    Cron { expr: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronPayload {
    pub message: String,
    pub channel: String,
    pub chat_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronJobState {
    pub next_run_at_ms: Option<i64>,
    pub last_run_at_ms: Option<i64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    #[serde(default)]
    pub consecutive_errors: u32,
    pub last_duration_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    pub state: CronJobState,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub delete_after_run: bool,
    /// Optional per-job dispatch timeout in seconds (overrides default 5s).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CronStore {
    version: u32,
    jobs: Vec<CronJob>,
}

impl Default for CronStore {
    fn default() -> Self {
        Self {
            version: 1,
            jobs: Vec::new(),
        }
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

#[cfg(test)]
const DEFAULT_DISPATCH_TIMEOUT_MS: u64 = 50;
#[cfg(not(test))]
const DEFAULT_DISPATCH_TIMEOUT_MS: u64 = 5_000;

/// Deduplication window: if a job's `last_run_at_ms` is within this window of
/// `next_run_at_ms`, we consider it already dispatched (crash recovery guard).
const DEDUP_WINDOW_MS: i64 = 60_000;

const ERROR_BACKOFF_SCHEDULE_MS: [i64; 5] = [
    30_000,      // 1st error  -> 30s
    60_000,      // 2nd error  -> 1m
    5 * 60_000,  // 3rd error  -> 5m
    15 * 60_000, // 4th error  -> 15m
    60 * 60_000, // 5th+ error -> 60m
];

fn dispatch_timeout_ms(timeout_secs: Option<u64>) -> u64 {
    timeout_secs
        .map(|secs| secs.saturating_mul(1000))
        .unwrap_or(DEFAULT_DISPATCH_TIMEOUT_MS)
}

fn should_skip_missed_dispatch(job: &CronJob, next_run_at_ms: i64) -> bool {
    let Some(last_run_at_ms) = job.state.last_run_at_ms else {
        return false;
    };
    if job.state.last_status.as_deref() != Some("ok") {
        return false;
    }
    let delta_ms = i128::from(last_run_at_ms) - i128::from(next_run_at_ms);
    delta_ms >= 0 && delta_ms < i128::from(DEDUP_WINDOW_MS)
}

fn error_backoff_ms(consecutive_errors: u32) -> i64 {
    if consecutive_errors == 0 {
        return 0;
    }
    let idx = ((consecutive_errors - 1) as usize).min(ERROR_BACKOFF_SCHEDULE_MS.len() - 1);
    ERROR_BACKOFF_SCHEDULE_MS[idx]
}

fn parse_cron_field(field: &str, min: u32, max: u32) -> Option<Vec<u32>> {
    if field == "*" {
        return Some((min..=max).collect());
    }
    if let Some(step_str) = field.strip_prefix("*/") {
        let step = step_str.parse::<u32>().ok()?;
        if step == 0 {
            return None;
        }
        return Some((min..=max).step_by(step as usize).collect());
    }

    let mut values = Vec::new();
    for part in field.split(',') {
        let value = part.parse::<u32>().ok()?;
        if !(min..=max).contains(&value) {
            return None;
        }
        values.push(value);
    }
    if values.is_empty() {
        None
    } else {
        Some(values)
    }
}

fn next_run_from_cron_expr(expr: &str, now: i64) -> Option<i64> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return None;
    }

    let minutes = parse_cron_field(fields[0], 0, 59)?;
    let hours = parse_cron_field(fields[1], 0, 23)?;
    let dom = parse_cron_field(fields[2], 1, 31)?;
    let month = parse_cron_field(fields[3], 1, 12)?;
    let dow = parse_cron_field(fields[4], 0, 6)?;

    let mut candidate = DateTime::from_timestamp_millis(now)?
        .with_second(0)?
        .with_nanosecond(0)?
        + Duration::minutes(1);
    let limit = candidate + Duration::days(366);

    while candidate <= limit {
        let m = candidate.minute();
        let h = candidate.hour();
        let d = candidate.day();
        let mon = candidate.month();
        let wd = candidate.weekday().num_days_from_sunday();
        if minutes.contains(&m)
            && hours.contains(&h)
            && dom.contains(&d)
            && month.contains(&mon)
            && dow.contains(&wd)
        {
            return Some(candidate.timestamp_millis());
        }
        candidate += Duration::minutes(1);
    }

    None
}

/// Returns true if the cron expression is valid and has a future run time.
pub fn is_valid_cron_expr(expr: &str) -> bool {
    next_run_from_cron_expr(expr, now_ms()).is_some()
}

fn next_run_at(schedule: &CronSchedule, now: i64) -> Option<i64> {
    match schedule {
        CronSchedule::At { at_ms } => {
            if *at_ms > now {
                Some(*at_ms)
            } else {
                None
            }
        }
        CronSchedule::Every { every_ms } => {
            if *every_ms > 0 {
                Some(now + every_ms)
            } else {
                None
            }
        }
        CronSchedule::Cron { expr } => next_run_from_cron_expr(expr, now),
    }
}

/// Compute a random-ish delay in [0, max_ms) using system clock nanoseconds.
/// Avoids adding `rand` crate — sufficient decorrelation for scheduling jitter.
fn jitter_delay(max_ms: u64) -> std::time::Duration {
    if max_ms == 0 {
        return std::time::Duration::ZERO;
    }
    let jitter = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % max_ms)
        .unwrap_or(0);
    std::time::Duration::from_millis(jitter)
}

/// Policy for handling missed schedules (jobs due while process was down).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum OnMiss {
    /// Skip missed runs, reschedule to next future time (default).
    #[default]
    Skip,
    /// Execute one missed run immediately, then reschedule.
    RunOnce,
}

/// Persistent cron scheduler.
pub struct CronService {
    store_path: PathBuf,
    store: Arc<RwLock<CronStore>>,
    bus: Arc<MessageBus>,
    running: Arc<AtomicBool>,
    handle: Arc<RwLock<Option<JoinHandle<()>>>>,
    jitter_ms: u64,
}

impl CronService {
    /// Create a new cron service.
    pub fn new(store_path: PathBuf, bus: Arc<MessageBus>) -> Self {
        Self::with_jitter(store_path, bus, 0)
    }

    /// Create a new cron service with configurable jitter (milliseconds).
    pub fn with_jitter(store_path: PathBuf, bus: Arc<MessageBus>, jitter_ms: u64) -> Self {
        Self {
            store_path,
            store: Arc::new(RwLock::new(CronStore::default())),
            bus,
            running: Arc::new(AtomicBool::new(false)),
            handle: Arc::new(RwLock::new(None)),
            jitter_ms,
        }
    }

    /// Start scheduler loop (idempotent).
    pub async fn start(&self, on_miss: &OnMiss) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let loaded = self.load_store().await?;
        let missed_payloads: Vec<CronPayload>;
        {
            let mut store = self.store.write().await;
            *store = loaded;
            let now = now_ms();
            let mut missed: Vec<CronPayload> = Vec::new();
            for job in &mut store.jobs {
                if job.enabled {
                    if let Some(next) = job.state.next_run_at_ms {
                        if next <= now {
                            // This job was missed while we were down
                            match on_miss {
                                OnMiss::Skip => {
                                    info!(job_id = %job.id, job_name = %job.name, "Skipping missed schedule");
                                }
                                OnMiss::RunOnce => {
                                    // Dedup guard: if job was already dispatched
                                    // recently (crash between dispatch and save),
                                    // skip to avoid duplicate delivery.
                                    let already_ran = should_skip_missed_dispatch(job, next);
                                    if already_ran {
                                        info!(
                                            job_id = %job.id,
                                            job_name = %job.name,
                                            "Skipping missed schedule (dedup: last_run near next_run)"
                                        );
                                    } else {
                                        info!(job_id = %job.id, job_name = %job.name, "Queueing missed schedule for immediate run");
                                        missed.push(job.payload.clone());
                                    }
                                }
                            }
                            // Either way, reschedule to next future time
                            job.state.next_run_at_ms = next_run_at(&job.schedule, now);
                        }
                        // If next > now, job is correctly scheduled for the future — leave it
                    } else {
                        job.state.next_run_at_ms = next_run_at(&job.schedule, now);
                    }
                }
            }
            missed_payloads = missed;
        }

        // Dispatch missed jobs outside the lock
        for payload in &missed_payloads {
            let inbound =
                InboundMessage::new(&payload.channel, "cron", &payload.chat_id, &payload.message);
            if let Err(e) = self.bus.publish_inbound(inbound).await {
                error!("Failed to dispatch missed job: {}", e);
            }
        }

        self.save_store().await?;

        let store = Arc::clone(&self.store);
        let store_path = self.store_path.clone();
        let bus = Arc::clone(&self.bus);
        let running = Arc::clone(&self.running);
        let jitter_ms = self.jitter_ms;

        let running_clone = Arc::clone(&running);
        let handle = tokio::spawn(async move {
            info!("Cron service started");
            // Use interval instead of sleep to prevent timer drift under load.
            // MissedTickBehavior::Delay ensures ticks don't pile up if a tick
            // takes longer than 1s — the next tick fires 1s after the slow one.
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            while running.load(Ordering::SeqCst) {
                interval.tick().await;
                if let Err(err) = tick(&store, &store_path, &bus, jitter_ms).await {
                    error!("Cron tick failed: {}", err);
                }
            }
            running_clone.store(false, Ordering::SeqCst);
        });

        let mut h = self.handle.write().await;
        *h = Some(handle);

        Ok(())
    }

    /// Stop scheduler loop.
    pub async fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
        let mut h = self.handle.write().await;
        if let Some(handle) = h.take() {
            handle.abort();
        }
    }

    /// Add a new job.
    pub async fn add_job(
        &self,
        name: String,
        schedule: CronSchedule,
        payload: CronPayload,
        delete_after_run: bool,
    ) -> Result<CronJob> {
        self.add_job_with_timeout(name, schedule, payload, delete_after_run, None)
            .await
    }

    /// Add a new job with an optional per-job dispatch timeout.
    pub async fn add_job_with_timeout(
        &self,
        name: String,
        schedule: CronSchedule,
        payload: CronPayload,
        delete_after_run: bool,
        timeout_secs: Option<u64>,
    ) -> Result<CronJob> {
        let now = now_ms();
        let job = CronJob {
            id: Uuid::new_v4().to_string().chars().take(8).collect(),
            name,
            enabled: true,
            schedule: schedule.clone(),
            payload,
            state: CronJobState {
                next_run_at_ms: next_run_at(&schedule, now),
                ..Default::default()
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run,
            timeout_secs,
        };

        {
            let mut store = self.store.write().await;
            store.jobs.push(job.clone());
        }
        self.save_store().await?;
        Ok(job)
    }

    /// List jobs.
    pub async fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        let store = self.store.read().await;
        let mut jobs: Vec<CronJob> = store
            .jobs
            .iter()
            .filter(|job| include_disabled || job.enabled)
            .cloned()
            .collect();
        jobs.sort_by_key(|job| job.state.next_run_at_ms.unwrap_or(i64::MAX));
        jobs
    }

    /// Remove a job by id.
    pub async fn remove_job(&self, job_id: &str) -> Result<bool> {
        let removed = {
            let mut store = self.store.write().await;
            let before = store.jobs.len();
            store.jobs.retain(|job| job.id != job_id);
            store.jobs.len() < before
        };
        if removed {
            self.save_store().await?;
        }
        Ok(removed)
    }

    async fn load_store(&self) -> Result<CronStore> {
        if !self.store_path.exists() {
            return Ok(CronStore::default());
        }
        let content = tokio::fs::read_to_string(&self.store_path).await?;
        let store = serde_json::from_str::<CronStore>(&content)?;
        Ok(store)
    }

    async fn save_store(&self) -> Result<()> {
        if let Some(parent) = self.store_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = {
            let store = self.store.read().await;
            serde_json::to_string_pretty(&*store)?
        };
        tokio::fs::write(&self.store_path, json).await?;
        Ok(())
    }
}

impl Drop for CronService {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

async fn tick(
    store: &Arc<RwLock<CronStore>>,
    store_path: &PathBuf,
    bus: &Arc<MessageBus>,
    jitter_ms: u64,
) -> Result<()> {
    let now = now_ms();
    let due_jobs: Vec<CronJob> = {
        let store_guard = store.read().await;
        store_guard
            .jobs
            .iter()
            .filter(|job| {
                job.enabled && job.state.next_run_at_ms.map(|n| n <= now).unwrap_or(false)
            })
            .cloned()
            .collect()
    };

    if due_jobs.is_empty() {
        return Ok(());
    }

    let mut results: Vec<(String, bool, Option<String>, i64, i64)> = Vec::new();
    for job in &due_jobs {
        let started_at = now_ms();
        let inbound = InboundMessage::new(
            &job.payload.channel,
            "cron",
            &job.payload.chat_id,
            &job.payload.message,
        );
        if jitter_ms > 0 {
            tokio::time::sleep(jitter_delay(jitter_ms)).await;
        }
        let timeout_ms = dispatch_timeout_ms(job.timeout_secs);
        let send_result = tokio::time::timeout(
            std::time::Duration::from_millis(timeout_ms),
            bus.publish_inbound(inbound),
        )
        .await;
        let ended_at = now_ms();
        match send_result {
            Ok(Ok(())) => results.push((job.id.clone(), true, None, started_at, ended_at)),
            Ok(Err(e)) => results.push((
                job.id.clone(),
                false,
                Some(e.to_string()),
                started_at,
                ended_at,
            )),
            Err(_) => results.push((
                job.id.clone(),
                false,
                Some("cron dispatch timed out".to_string()),
                started_at,
                ended_at,
            )),
        }
    }

    {
        let mut store_guard = store.write().await;
        for (job_id, ok, err, started_at, ended_at) in results {
            if let Some(job) = store_guard.jobs.iter_mut().find(|j| j.id == job_id) {
                job.state.last_run_at_ms = Some(started_at);
                job.state.last_duration_ms = Some((ended_at - started_at).max(0));
                job.state.last_status = Some(if ok { "ok" } else { "error" }.to_string());
                job.state.last_error = err;
                job.updated_at_ms = ended_at;
                if ok {
                    job.state.consecutive_errors = 0;
                } else {
                    job.state.consecutive_errors = job.state.consecutive_errors.saturating_add(1);
                }

                match job.schedule {
                    CronSchedule::At { .. } => {
                        job.enabled = false;
                        job.state.next_run_at_ms = None;
                    }
                    _ => {
                        if ok {
                            job.state.next_run_at_ms = next_run_at(&job.schedule, ended_at);
                        } else {
                            let base_next = next_run_at(&job.schedule, ended_at).unwrap_or(
                                ended_at + error_backoff_ms(job.state.consecutive_errors),
                            );
                            let backoff_next =
                                ended_at + error_backoff_ms(job.state.consecutive_errors);
                            job.state.next_run_at_ms = Some(base_next.max(backoff_next));
                        }
                    }
                }
            }
        }
        // Remove one-shot jobs marked for delete-after-run only after success.
        store_guard.jobs.retain(|job| {
            let should_remove = matches!(job.schedule, CronSchedule::At { .. })
                && job.delete_after_run
                && !job.enabled
                && job.state.last_status.as_deref() == Some("ok");
            !should_remove
        });
    }

    let json = {
        let store_guard = store.read().await;
        serde_json::to_string_pretty(&*store_guard)?
    };
    if let Some(parent) = store_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(store_path, json).await?;

    Ok(())
}

/// Parse ISO datetime string into unix milliseconds.
pub fn parse_at_datetime_ms(input: &str) -> Result<i64> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(input) {
        return Ok(dt.timestamp_millis());
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(input, "%Y-%m-%dT%H:%M:%S") {
        return Ok(naive.and_utc().timestamp_millis());
    }
    Err(ZeptoError::Tool(format!(
        "Invalid 'at' datetime '{}'. Use RFC3339 or YYYY-MM-DDTHH:MM:SS",
        input
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::MessageBus;
    use tempfile::tempdir;

    #[test]
    fn test_next_run_at_every() {
        let now = 1_000;
        let next = next_run_at(&CronSchedule::Every { every_ms: 500 }, now).unwrap();
        assert_eq!(next, 1_500);
    }

    #[test]
    fn test_parse_at_datetime_ms_rfc3339() {
        let ms = parse_at_datetime_ms("2026-02-12T12:34:56Z").unwrap();
        assert!(ms > 0);
    }

    #[tokio::test]
    async fn test_add_list_remove_job() {
        let temp = tempdir().unwrap();
        let service = CronService::new(temp.path().join("jobs.json"), Arc::new(MessageBus::new()));

        let job = service
            .add_job(
                "test".to_string(),
                CronSchedule::Every { every_ms: 1_000 },
                CronPayload {
                    message: "hello".to_string(),
                    channel: "cli".to_string(),
                    chat_id: "cli".to_string(),
                },
                false,
            )
            .await
            .unwrap();

        let jobs = service.list_jobs(true).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, job.id);

        let removed = service.remove_job(&job.id).await.unwrap();
        assert!(removed);
        assert!(service.list_jobs(true).await.is_empty());
    }

    #[test]
    fn test_jitter_delay_zero() {
        let d = jitter_delay(0);
        assert_eq!(d, std::time::Duration::ZERO);
    }

    #[test]
    fn test_jitter_delay_bounded() {
        let max_ms = 500;
        let d = jitter_delay(max_ms);
        assert!(d < std::time::Duration::from_millis(max_ms));
    }

    #[test]
    fn test_cron_service_with_jitter() {
        let temp = tempdir().unwrap();
        let service = CronService::with_jitter(
            temp.path().join("jobs.json"),
            Arc::new(MessageBus::new()),
            250,
        );
        assert_eq!(service.jitter_ms, 250);
    }

    #[test]
    fn test_on_miss_default_is_skip() {
        let policy = OnMiss::default();
        assert_eq!(policy, OnMiss::Skip);
    }

    #[test]
    fn test_on_miss_serde_roundtrip() {
        let skip_json = serde_json::to_string(&OnMiss::Skip).unwrap();
        assert_eq!(skip_json, r#""skip""#);

        let run_once_json = serde_json::to_string(&OnMiss::RunOnce).unwrap();
        assert_eq!(run_once_json, r#""run_once""#);

        let parsed: OnMiss = serde_json::from_str(r#""run_once""#).unwrap();
        assert_eq!(parsed, OnMiss::RunOnce);
    }

    #[tokio::test]
    async fn test_start_skip_missed_jobs() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store_path = temp.path().join("jobs.json");

        // Pre-seed store with a job whose next_run is in the past
        let json = serde_json::json!({
            "version": 1,
            "jobs": [{
                "id": "missed1",
                "name": "missed job",
                "enabled": true,
                "schedule": { "kind": "every", "every_ms": 60000 },
                "payload": { "message": "check", "channel": "cli", "chat_id": "cli" },
                "state": { "next_run_at_ms": 1 },
                "created_at_ms": 1,
                "updated_at_ms": 1,
                "delete_after_run": false
            }]
        });
        tokio::fs::write(&store_path, serde_json::to_string_pretty(&json).unwrap())
            .await
            .unwrap();

        let service = CronService::new(store_path, bus);
        service.start(&OnMiss::Skip).await.unwrap();
        service.stop().await;

        // After skip, job should have a future next_run_at_ms
        let jobs = service.list_jobs(true).await;
        assert_eq!(jobs.len(), 1);
        let next = jobs[0].state.next_run_at_ms.unwrap();
        assert!(
            next > now_ms() - 5000,
            "next_run should be in the future after skip"
        );
    }

    #[tokio::test]
    async fn test_start_run_once_missed_jobs() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store_path = temp.path().join("jobs.json");

        // Pre-seed store with a missed job
        let json = serde_json::json!({
            "version": 1,
            "jobs": [{
                "id": "missed2",
                "name": "missed run_once job",
                "enabled": true,
                "schedule": { "kind": "every", "every_ms": 60000 },
                "payload": { "message": "run_once_check", "channel": "cli", "chat_id": "cli" },
                "state": { "next_run_at_ms": 1 },
                "created_at_ms": 1,
                "updated_at_ms": 1,
                "delete_after_run": false
            }]
        });
        tokio::fs::write(&store_path, serde_json::to_string_pretty(&json).unwrap())
            .await
            .unwrap();

        let service = CronService::new(store_path, bus.clone());
        service.start(&OnMiss::RunOnce).await.unwrap();
        service.stop().await;

        // Verify the missed job was dispatched via the bus
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), bus.consume_inbound())
            .await
            .expect("should receive dispatched missed job within timeout")
            .expect("bus should have a message");
        assert_eq!(msg.content, "run_once_check");

        // Job should still be rescheduled to the future
        let jobs = service.list_jobs(true).await;
        assert_eq!(jobs.len(), 1);
        let next = jobs[0].state.next_run_at_ms.unwrap();
        assert!(
            next > now_ms() - 5000,
            "next_run should be in the future after run_once"
        );
    }

    #[test]
    fn test_error_backoff_schedule() {
        assert_eq!(error_backoff_ms(0), 0);
        assert_eq!(error_backoff_ms(1), 30_000);
        assert_eq!(error_backoff_ms(2), 60_000);
        assert_eq!(error_backoff_ms(3), 300_000);
        assert_eq!(error_backoff_ms(10), 3_600_000);
    }

    #[tokio::test]
    async fn test_tick_timeout_applies_error_backoff() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::with_buffer_size(1));
        let store = Arc::new(RwLock::new(CronStore {
            version: 1,
            jobs: vec![
                CronJob {
                    id: "fill".to_string(),
                    name: "fill queue".to_string(),
                    enabled: true,
                    schedule: CronSchedule::Every { every_ms: 1_000 },
                    payload: CronPayload {
                        message: "fill".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: false,
                    timeout_secs: None,
                },
                CronJob {
                    id: "timeout".to_string(),
                    name: "should timeout".to_string(),
                    enabled: true,
                    schedule: CronSchedule::Every { every_ms: 1_000 },
                    payload: CronPayload {
                        message: "timeout".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: false,
                    timeout_secs: None,
                },
            ],
        }));
        let store_path = temp.path().join("jobs.json");

        tick(&store, &store_path, &bus, 0).await.unwrap();

        let store_guard = store.read().await;
        let timed_out = store_guard
            .jobs
            .iter()
            .find(|j| j.id == "timeout")
            .expect("timeout job");
        assert_eq!(timed_out.state.last_status.as_deref(), Some("error"));
        assert_eq!(timed_out.state.consecutive_errors, 1);
        let last_run = timed_out.state.last_run_at_ms.expect("last_run_at_ms");
        let duration = timed_out.state.last_duration_ms.unwrap_or(0);
        let ended_at = last_run + duration;
        let next = timed_out
            .state
            .next_run_at_ms
            .expect("next_run_at_ms should be set");
        assert!(
            next >= ended_at + 29_000,
            "expected backoff >= ~30s, got next={} ended_at={}",
            next,
            ended_at
        );
    }

    #[tokio::test]
    async fn test_delete_after_run_at_job_not_removed_on_error() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::with_buffer_size(1));
        let store = Arc::new(RwLock::new(CronStore {
            version: 1,
            jobs: vec![
                CronJob {
                    id: "fill".to_string(),
                    name: "fill queue".to_string(),
                    enabled: true,
                    schedule: CronSchedule::Every { every_ms: 1_000 },
                    payload: CronPayload {
                        message: "fill".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: false,
                    timeout_secs: None,
                },
                CronJob {
                    id: "atdel".to_string(),
                    name: "one-shot delete".to_string(),
                    enabled: true,
                    schedule: CronSchedule::At {
                        at_ms: now_ms() - 1,
                    },
                    payload: CronPayload {
                        message: "one-shot".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: true,
                    timeout_secs: None,
                },
            ],
        }));
        let store_path = temp.path().join("jobs.json");

        tick(&store, &store_path, &bus, 0).await.unwrap();

        let store_guard = store.read().await;
        let job = store_guard.jobs.iter().find(|j| j.id == "atdel").unwrap();
        assert_eq!(job.state.last_status.as_deref(), Some("error"));
        assert!(!job.enabled, "one-shot should be disabled after run");
        assert!(
            job.state.next_run_at_ms.is_none(),
            "one-shot should not be rescheduled after error"
        );
    }
    #[tokio::test]
    async fn test_one_shot_job_removed_after_success() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store = Arc::new(RwLock::new(CronStore {
            version: 1,
            jobs: vec![CronJob {
                id: "oneshot-ok".to_string(),
                name: "one-shot success".to_string(),
                enabled: true,
                schedule: CronSchedule::At {
                    at_ms: now_ms() - 1,
                },
                payload: CronPayload {
                    message: "hello".to_string(),
                    channel: "cli".to_string(),
                    chat_id: "cli".to_string(),
                },
                state: CronJobState {
                    next_run_at_ms: Some(now_ms() - 1),
                    ..Default::default()
                },
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                delete_after_run: true,
                timeout_secs: None,
            }],
        }));
        let store_path = temp.path().join("jobs.json");

        // Confirm job exists before tick
        assert_eq!(store.read().await.jobs.len(), 1);

        tick(&store, &store_path, &bus, 0).await.unwrap();

        let store_guard = store.read().await;
        assert!(
            store_guard.jobs.is_empty(),
            "one-shot job with delete_after_run=true should be removed after successful dispatch"
        );
    }

    // --- Duplicate delivery guard (#252) ---

    #[tokio::test]
    async fn test_dedup_guard_skips_recently_dispatched_job() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store_path = temp.path().join("jobs.json");

        let next_run = 100_000;
        // Simulate crash: last_run_at is close to next_run (within DEDUP_WINDOW_MS)
        let json = serde_json::json!({
            "version": 1,
            "jobs": [{
                "id": "dedup1",
                "name": "dedup job",
                "enabled": true,
                "schedule": { "kind": "every", "every_ms": 60000 },
                "payload": { "message": "dedup_check", "channel": "cli", "chat_id": "cli" },
                "state": {
                    "next_run_at_ms": next_run,
                    "last_run_at_ms": next_run + 50,
                    "last_status": "ok"
                },
                "created_at_ms": 1,
                "updated_at_ms": 1,
                "delete_after_run": false
            }]
        });
        tokio::fs::write(&store_path, serde_json::to_string_pretty(&json).unwrap())
            .await
            .unwrap();

        let service = CronService::new(store_path, bus.clone());
        service.start(&OnMiss::RunOnce).await.unwrap();
        service.stop().await;

        // Should NOT have dispatched (dedup guard)
        let result =
            tokio::time::timeout(std::time::Duration::from_millis(200), bus.consume_inbound())
                .await;
        assert!(
            result.is_err(),
            "dedup guard should prevent dispatch when last_run is near next_run"
        );
    }

    #[tokio::test]
    async fn test_dedup_guard_allows_genuinely_missed_job() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store_path = temp.path().join("jobs.json");

        // last_run is far from next_run (genuinely missed)
        let json = serde_json::json!({
            "version": 1,
            "jobs": [{
                "id": "dedup2",
                "name": "genuine miss",
                "enabled": true,
                "schedule": { "kind": "every", "every_ms": 60000 },
                "payload": { "message": "genuine_check", "channel": "cli", "chat_id": "cli" },
                "state": {
                    "next_run_at_ms": 1,
                    "last_run_at_ms": null,
                    "last_status": null
                },
                "created_at_ms": 1,
                "updated_at_ms": 1,
                "delete_after_run": false
            }]
        });
        tokio::fs::write(&store_path, serde_json::to_string_pretty(&json).unwrap())
            .await
            .unwrap();

        let service = CronService::new(store_path, bus.clone());
        service.start(&OnMiss::RunOnce).await.unwrap();
        service.stop().await;

        // Should dispatch — no dedup match
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), bus.consume_inbound())
            .await
            .expect("genuinely missed job should be dispatched")
            .expect("bus should have a message");
        assert_eq!(msg.content, "genuine_check");
    }

    // --- Per-job timeout (#254) ---

    #[tokio::test]
    async fn test_per_job_timeout_overrides_default() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store = Arc::new(RwLock::new(CronStore {
            version: 1,
            jobs: vec![CronJob {
                id: "timed".to_string(),
                name: "timed job".to_string(),
                enabled: true,
                schedule: CronSchedule::Every { every_ms: 60_000 },
                payload: CronPayload {
                    message: "timed_check".to_string(),
                    channel: "cli".to_string(),
                    chat_id: "cli".to_string(),
                },
                state: CronJobState {
                    next_run_at_ms: Some(now_ms() - 1),
                    ..Default::default()
                },
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
                delete_after_run: false,
                timeout_secs: Some(10),
            }],
        }));
        let store_path = temp.path().join("jobs.json");

        tick(&store, &store_path, &bus, 0).await.unwrap();

        let store_guard = store.read().await;
        let job = store_guard.jobs.first().expect("job should exist");
        assert_eq!(job.state.last_status.as_deref(), Some("ok"));
        assert_eq!(job.timeout_secs, Some(10));
    }

    #[tokio::test]
    async fn test_add_job_with_timeout() {
        let temp = tempdir().unwrap();
        let service = CronService::new(temp.path().join("jobs.json"), Arc::new(MessageBus::new()));

        let job = service
            .add_job_with_timeout(
                "timeout test".to_string(),
                CronSchedule::Every { every_ms: 1_000 },
                CronPayload {
                    message: "hello".to_string(),
                    channel: "cli".to_string(),
                    chat_id: "cli".to_string(),
                },
                false,
                Some(30),
            )
            .await
            .unwrap();

        assert_eq!(job.timeout_secs, Some(30));

        // Verify it persists through serde
        let jobs = service.list_jobs(true).await;
        assert_eq!(jobs[0].timeout_secs, Some(30));
    }

    #[test]
    fn test_timeout_secs_serde_roundtrip() {
        let job = CronJob {
            id: "t1".to_string(),
            name: "test".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 1000 },
            payload: CronPayload {
                message: "hi".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState::default(),
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: Some(60),
        };

        let json = serde_json::to_string(&job).unwrap();
        assert!(json.contains("\"timeout_secs\":60"));

        let parsed: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.timeout_secs, Some(60));

        // None should be omitted
        let job_no_timeout = CronJob {
            timeout_secs: None,
            ..job.clone()
        };
        let json2 = serde_json::to_string(&job_no_timeout).unwrap();
        assert!(!json2.contains("timeout_secs"));

        // Missing field deserializes as None
        let json3 = json2.clone();
        let parsed2: CronJob = serde_json::from_str(&json3).unwrap();
        assert_eq!(parsed2.timeout_secs, None);
    }

    #[test]
    fn test_dedup_window_constant() {
        assert_eq!(DEDUP_WINDOW_MS, 60_000, "dedup window should be 60 seconds");
    }

    #[test]
    fn test_dispatch_timeout_ms_default() {
        assert_eq!(dispatch_timeout_ms(None), DEFAULT_DISPATCH_TIMEOUT_MS);
    }

    #[test]
    fn test_dispatch_timeout_ms_saturates_on_large_value() {
        assert_eq!(dispatch_timeout_ms(Some(u64::MAX)), u64::MAX);
    }

    #[test]
    fn test_should_skip_missed_dispatch_requires_last_run_after_next() {
        let job = CronJob {
            id: "dedup3".to_string(),
            name: "fast schedule".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 30_000 },
            payload: CronPayload {
                message: "x".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState {
                last_run_at_ms: Some(70_000),
                last_status: Some("ok".to_string()),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: None,
        };
        assert!(
            !should_skip_missed_dispatch(&job, 100_000),
            "dedup guard should not skip when last_run is before next_run"
        );
    }

    #[test]
    fn test_should_skip_missed_dispatch_near_next_with_success() {
        let job = CronJob {
            id: "dedup4".to_string(),
            name: "near next".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 60_000 },
            payload: CronPayload {
                message: "x".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState {
                last_run_at_ms: Some(100_010),
                last_status: Some("ok".to_string()),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: None,
        };
        assert!(should_skip_missed_dispatch(&job, 100_000));
    }

    #[test]
    fn test_should_skip_missed_dispatch_no_last_run() {
        let job = CronJob {
            id: "d5".to_string(),
            name: "never ran".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 60_000 },
            payload: CronPayload {
                message: "x".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState {
                last_run_at_ms: None,
                last_status: None,
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: None,
        };
        assert!(
            !should_skip_missed_dispatch(&job, 100_000),
            "should not skip when job has never run"
        );
    }

    #[test]
    fn test_should_skip_missed_dispatch_last_run_errored() {
        let job = CronJob {
            id: "d6".to_string(),
            name: "errored run".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 60_000 },
            payload: CronPayload {
                message: "x".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState {
                last_run_at_ms: Some(100_010),
                last_status: Some("error".to_string()),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: None,
        };
        assert!(
            !should_skip_missed_dispatch(&job, 100_000),
            "should not skip when last run was an error — must re-dispatch"
        );
    }

    #[test]
    fn test_should_skip_missed_dispatch_outside_window() {
        let job = CronJob {
            id: "d7".to_string(),
            name: "old run".to_string(),
            enabled: true,
            schedule: CronSchedule::Every { every_ms: 60_000 },
            payload: CronPayload {
                message: "x".to_string(),
                channel: "cli".to_string(),
                chat_id: "cli".to_string(),
            },
            state: CronJobState {
                // last_run is 120s after next_run — outside 60s window
                last_run_at_ms: Some(100_000 + 120_000),
                last_status: Some("ok".to_string()),
                ..Default::default()
            },
            created_at_ms: 0,
            updated_at_ms: 0,
            delete_after_run: false,
            timeout_secs: None,
        };
        assert!(
            !should_skip_missed_dispatch(&job, 100_000),
            "should not skip when last_run is outside the dedup window"
        );
    }

    #[test]
    fn test_dispatch_timeout_ms_normal_value() {
        assert_eq!(dispatch_timeout_ms(Some(10)), 10_000);
        assert_eq!(dispatch_timeout_ms(Some(1)), 1_000);
        assert_eq!(dispatch_timeout_ms(Some(0)), 0);
    }

    #[tokio::test]
    async fn test_dedup_guard_dispatches_when_last_run_errored() {
        let temp = tempdir().unwrap();
        let bus = Arc::new(MessageBus::new());
        let store_path = temp.path().join("jobs.json");

        // Simulate crash after a failed dispatch — should re-dispatch
        let json = serde_json::json!({
            "version": 1,
            "jobs": [{
                "id": "dedup-err",
                "name": "errored dedup",
                "enabled": true,
                "schedule": { "kind": "every", "every_ms": 60000 },
                "payload": { "message": "retry_me", "channel": "cli", "chat_id": "cli" },
                "state": {
                    "next_run_at_ms": 1,
                    "last_run_at_ms": 50,
                    "last_status": "error",
                    "last_error": "cron dispatch timed out"
                },
                "created_at_ms": 1,
                "updated_at_ms": 1,
                "delete_after_run": false
            }]
        });
        tokio::fs::write(&store_path, serde_json::to_string_pretty(&json).unwrap())
            .await
            .unwrap();

        let service = CronService::new(store_path, bus.clone());
        service.start(&OnMiss::RunOnce).await.unwrap();
        service.stop().await;

        // Should dispatch — last run was an error, dedup guard must not block
        let msg = tokio::time::timeout(std::time::Duration::from_secs(2), bus.consume_inbound())
            .await
            .expect("errored job should be re-dispatched")
            .expect("bus should have a message");
        assert_eq!(msg.content, "retry_me");
    }

    #[tokio::test]
    async fn test_per_job_timeout_causes_timeout_on_full_bus() {
        let temp = tempdir().unwrap();
        // Buffer size 1: first job fills the bus, second job should timeout
        let bus = Arc::new(MessageBus::with_buffer_size(1));
        let store = Arc::new(RwLock::new(CronStore {
            version: 1,
            jobs: vec![
                CronJob {
                    id: "fill2".to_string(),
                    name: "fill queue".to_string(),
                    enabled: true,
                    schedule: CronSchedule::Every { every_ms: 60_000 },
                    payload: CronPayload {
                        message: "fill".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: false,
                    timeout_secs: None,
                },
                CronJob {
                    id: "short-timeout".to_string(),
                    name: "short timeout job".to_string(),
                    enabled: true,
                    schedule: CronSchedule::Every { every_ms: 60_000 },
                    payload: CronPayload {
                        message: "should_timeout".to_string(),
                        channel: "cli".to_string(),
                        chat_id: "cli".to_string(),
                    },
                    state: CronJobState {
                        next_run_at_ms: Some(now_ms() - 1),
                        ..Default::default()
                    },
                    created_at_ms: now_ms(),
                    updated_at_ms: now_ms(),
                    delete_after_run: false,
                    // Very short timeout — in tests DEFAULT_DISPATCH_TIMEOUT_MS is
                    // already 50ms, but this proves the field is actually read.
                    timeout_secs: Some(0),
                },
            ],
        }));
        let store_path = temp.path().join("jobs.json");

        tick(&store, &store_path, &bus, 0).await.unwrap();

        let store_guard = store.read().await;
        let timed = store_guard
            .jobs
            .iter()
            .find(|j| j.id == "short-timeout")
            .expect("short-timeout job");
        assert_eq!(
            timed.state.last_status.as_deref(),
            Some("error"),
            "job with timeout_secs=0 on full bus should timeout"
        );
    }

    #[tokio::test]
    async fn test_timeout_secs_persists_through_store_reload() {
        let temp = tempdir().unwrap();
        let store_path = temp.path().join("jobs.json");
        let bus = Arc::new(MessageBus::new());

        // Create a job with timeout_secs via the service
        let service = CronService::new(store_path.clone(), bus.clone());
        service
            .add_job_with_timeout(
                "persist test".to_string(),
                CronSchedule::Every { every_ms: 60_000 },
                CronPayload {
                    message: "hi".to_string(),
                    channel: "cli".to_string(),
                    chat_id: "cli".to_string(),
                },
                false,
                Some(45),
            )
            .await
            .unwrap();
        drop(service);

        // Create a new service from the same file — simulates restart
        let service2 = CronService::new(store_path, bus);
        service2.start(&OnMiss::Skip).await.unwrap();
        service2.stop().await;

        let jobs = service2.list_jobs(true).await;
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].timeout_secs,
            Some(45),
            "timeout_secs should survive store reload"
        );
    }
}
