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
                                    info!(job_id = %job.id, job_name = %job.name, "Queueing missed schedule for immediate run");
                                    missed.push(job.payload.clone());
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

        let handle = tokio::spawn(async move {
            info!("Cron service started");
            while running.load(Ordering::SeqCst) {
                if let Err(err) = tick(&store, &store_path, &bus, jitter_ms).await {
                    error!("Cron tick failed: {}", err);
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
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
        let now = now_ms();
        let job = CronJob {
            id: Uuid::new_v4().to_string()[..8].to_string(),
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

    let mut results: Vec<(String, bool, Option<String>)> = Vec::new();
    for job in &due_jobs {
        let inbound = InboundMessage::new(
            &job.payload.channel,
            "cron",
            &job.payload.chat_id,
            &job.payload.message,
        );
        if jitter_ms > 0 {
            tokio::time::sleep(jitter_delay(jitter_ms)).await;
        }
        match bus.publish_inbound(inbound).await {
            Ok(_) => results.push((job.id.clone(), true, None)),
            Err(e) => results.push((job.id.clone(), false, Some(e.to_string()))),
        }
    }

    {
        let mut store_guard = store.write().await;
        for (job_id, ok, err) in results {
            if let Some(job) = store_guard.jobs.iter_mut().find(|j| j.id == job_id) {
                job.state.last_run_at_ms = Some(now);
                job.state.last_status = Some(if ok { "ok" } else { "error" }.to_string());
                job.state.last_error = err;
                job.updated_at_ms = now;

                match job.schedule {
                    CronSchedule::At { .. } => {
                        if job.delete_after_run {
                            job.enabled = false;
                        } else {
                            job.enabled = false;
                            job.state.next_run_at_ms = None;
                        }
                    }
                    _ => {
                        job.state.next_run_at_ms = next_run_at(&job.schedule, now);
                    }
                }
            }
        }
        // Remove one-shot jobs marked for deletion.
        store_guard.jobs.retain(|job| {
            !(matches!(job.schedule, CronSchedule::At { .. })
                && job.delete_after_run
                && !job.enabled)
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
}
