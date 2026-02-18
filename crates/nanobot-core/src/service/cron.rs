use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{error, info, warn};
use uuid::Uuid;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Schedule definition for a cron job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CronSchedule {
    #[serde(rename_all = "camelCase")]
    At { at_ms: u64 },
    #[serde(rename_all = "camelCase")]
    Every { every_ms: u64 },
    #[serde(rename_all = "camelCase")]
    Cron {
        expr: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tz: Option<String>,
    },
}

impl CronSchedule {
    pub fn kind_str(&self) -> &str {
        match self {
            CronSchedule::At { .. } => "at",
            CronSchedule::Every { .. } => "every",
            CronSchedule::Cron { .. } => "cron",
        }
    }

    /// Compute next run time in ms.
    pub fn next_run(&self, now_ms: u64) -> Option<u64> {
        match self {
            CronSchedule::At { at_ms } => {
                if *at_ms > now_ms {
                    Some(*at_ms)
                } else {
                    None
                }
            }
            CronSchedule::Every { every_ms } => {
                if *every_ms > 0 {
                    Some(now_ms + every_ms)
                } else {
                    None
                }
            }
            CronSchedule::Cron { expr, .. } => {
                // Use the cron crate to compute next run
                use cron::Schedule;
                use std::str::FromStr;
                // cron crate expects 6 or 7 fields, standard crontab has 5
                // Prepend seconds field "0" if only 5 fields
                let cron_expr = if expr.split_whitespace().count() == 5 {
                    format!("0 {expr}")
                } else {
                    expr.clone()
                };
                match Schedule::from_str(&cron_expr) {
                    Ok(schedule) => {
                        schedule
                            .upcoming(chrono::Utc)
                            .next()
                            .map(|dt| dt.timestamp_millis() as u64)
                    }
                    Err(e) => {
                        warn!("Invalid cron expression '{}': {}", expr, e);
                        None
                    }
                }
            }
        }
    }
}

/// What to do when the job runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronPayload {
    #[serde(default = "default_payload_kind")]
    pub kind: String,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub deliver: bool,
    pub channel: Option<String>,
    pub to: Option<String>,
}

fn default_payload_kind() -> String {
    "agent_turn".to_string()
}

/// Runtime state of a job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[derive(Default)]
pub struct CronJobState {
    pub next_run_at_ms: Option<u64>,
    pub last_run_at_ms: Option<u64>,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
}


/// A scheduled job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CronJob {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub schedule: CronSchedule,
    pub payload: CronPayload,
    #[serde(default)]
    pub state: CronJobState,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
    #[serde(default)]
    pub delete_after_run: bool,
}

fn default_true() -> bool {
    true
}

/// Persistent store for cron jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CronStore {
    #[serde(default = "default_version")]
    version: u32,
    #[serde(default)]
    jobs: Vec<CronJob>,
}

fn default_version() -> u32 {
    1
}

/// Service for managing and executing scheduled jobs.
pub struct CronService {
    store_path: PathBuf,
    store: Option<CronStore>,
}

impl CronService {
    pub fn new(store_path: PathBuf) -> Self {
        Self {
            store_path,
            store: None,
        }
    }

    fn load_store(&mut self) -> &mut CronStore {
        if self.store.is_none() {
            let store = if self.store_path.exists() {
                match std::fs::read_to_string(&self.store_path) {
                    Ok(content) => serde_json::from_str(&content).unwrap_or(CronStore {
                        version: 1,
                        jobs: Vec::new(),
                    }),
                    Err(e) => {
                        warn!("Failed to load cron store: {}", e);
                        CronStore {
                            version: 1,
                            jobs: Vec::new(),
                        }
                    }
                }
            } else {
                CronStore {
                    version: 1,
                    jobs: Vec::new(),
                }
            };
            self.store = Some(store);
        }
        self.store.as_mut().unwrap()
    }

    fn save_store(&self) {
        if let Some(ref store) = self.store {
            if let Some(parent) = self.store_path.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            if let Ok(json) = serde_json::to_string_pretty(store) {
                if let Err(e) = std::fs::write(&self.store_path, json) {
                    error!("Failed to save cron store: {}", e);
                }
            }
        }
    }

    /// List all jobs.
    pub fn list_jobs(&self, include_disabled: bool) -> Vec<CronJob> {
        match &self.store {
            Some(store) => {
                let mut jobs: Vec<_> = if include_disabled {
                    store.jobs.clone()
                } else {
                    store.jobs.iter().filter(|j| j.enabled).cloned().collect()
                };
                jobs.sort_by_key(|j| j.state.next_run_at_ms.unwrap_or(u64::MAX));
                jobs
            }
            None => Vec::new(),
        }
    }

    /// Add a new job.
    pub fn add_job(
        &mut self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: Option<&str>,
        to: Option<&str>,
    ) -> CronJob {
        let now = now_ms();
        let job = CronJob {
            id: Uuid::new_v4().to_string()[..8].to_string(),
            name: name.to_string(),
            enabled: true,
            schedule: schedule.clone(),
            payload: CronPayload {
                kind: "agent_turn".to_string(),
                message: message.to_string(),
                deliver,
                channel: channel.map(|s| s.to_string()),
                to: to.map(|s| s.to_string()),
            },
            state: CronJobState {
                next_run_at_ms: schedule.next_run(now),
                ..Default::default()
            },
            created_at_ms: now,
            updated_at_ms: now,
            delete_after_run: false,
        };

        let store = self.load_store();
        store.jobs.push(job.clone());
        self.save_store();
        info!("Cron: added job '{}' ({})", name, job.id);
        job
    }

    /// Remove a job by ID.
    pub fn remove_job(&mut self, job_id: &str) -> bool {
        let store = self.load_store();
        let before = store.jobs.len();
        store.jobs.retain(|j| j.id != job_id);
        let removed = store.jobs.len() < before;
        if removed {
            self.save_store();
            info!("Cron: removed job {}", job_id);
        }
        removed
    }

    /// Enable or disable a job.
    pub fn enable_job(&mut self, job_id: &str, enabled: bool) -> Option<CronJob> {
        let store = self.load_store();
        let now = now_ms();
        for job in &mut store.jobs {
            if job.id == job_id {
                job.enabled = enabled;
                job.updated_at_ms = now;
                if enabled {
                    job.state.next_run_at_ms = job.schedule.next_run(now);
                } else {
                    job.state.next_run_at_ms = None;
                }
                let result = job.clone();
                self.save_store();
                return Some(result);
            }
        }
        None
    }

    /// Get due jobs (next_run_at_ms <= now).
    pub fn get_due_jobs(&mut self) -> Vec<CronJob> {
        let now = now_ms();
        let store = self.load_store();
        store
            .jobs
            .iter()
            .filter(|j| {
                j.enabled
                    && j.state
                        .next_run_at_ms
                        .map(|t| now >= t)
                        .unwrap_or(false)
            })
            .cloned()
            .collect()
    }

    /// Mark a job as executed and compute next run.
    pub fn mark_executed(&mut self, job_id: &str, status: &str, error: Option<&str>) {
        let now = now_ms();
        let store = self.load_store();
        for job in &mut store.jobs {
            if job.id == job_id {
                job.state.last_run_at_ms = Some(now);
                job.state.last_status = Some(status.to_string());
                job.state.last_error = error.map(|s| s.to_string());
                job.updated_at_ms = now;

                match &job.schedule {
                    CronSchedule::At { .. } => {
                        if job.delete_after_run {
                            // Will be removed
                        } else {
                            job.enabled = false;
                            job.state.next_run_at_ms = None;
                        }
                    }
                    _ => {
                        job.state.next_run_at_ms = job.schedule.next_run(now);
                    }
                }
                break;
            }
        }
        // Remove delete_after_run jobs
        store.jobs.retain(|j| {
            !(j.id == job_id && j.delete_after_run && matches!(j.schedule, CronSchedule::At { .. }))
        });
        self.save_store();
    }

    /// Get service status.
    pub fn status(&self) -> serde_json::Value {
        let jobs_count = self
            .store
            .as_ref()
            .map(|s| s.jobs.len())
            .unwrap_or(0);
        serde_json::json!({
            "jobs": jobs_count,
        })
    }

    /// Initialize the store (call on startup).
    pub fn init(&mut self) {
        self.load_store();
        // Recompute next runs
        let now = now_ms();
        if let Some(ref mut store) = self.store {
            for job in &mut store.jobs {
                if job.enabled {
                    job.state.next_run_at_ms = job.schedule.next_run(now);
                }
            }
        }
        self.save_store();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cron_service() -> (tempfile::TempDir, CronService) {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("cron").join("jobs.json");
        let mut svc = CronService::new(store_path);
        svc.init();
        (tmp, svc)
    }

    #[test]
    fn test_cron_schedule_every() {
        let schedule = CronSchedule::Every { every_ms: 60000 };
        assert_eq!(schedule.kind_str(), "every");
        let next = schedule.next_run(1000000);
        assert_eq!(next, Some(1060000));
    }

    #[test]
    fn test_cron_schedule_at_future() {
        let schedule = CronSchedule::At { at_ms: 2000000 };
        assert_eq!(schedule.kind_str(), "at");
        assert_eq!(schedule.next_run(1000000), Some(2000000));
    }

    #[test]
    fn test_cron_schedule_at_past() {
        let schedule = CronSchedule::At { at_ms: 500000 };
        assert_eq!(schedule.next_run(1000000), None);
    }

    #[test]
    fn test_cron_schedule_cron_expr() {
        let schedule = CronSchedule::Cron {
            expr: "*/5 * * * *".to_string(),
            tz: None,
        };
        assert_eq!(schedule.kind_str(), "cron");
        let next = schedule.next_run(0);
        assert!(next.is_some());
    }

    #[test]
    fn test_cron_schedule_invalid_expr() {
        let schedule = CronSchedule::Cron {
            expr: "not a cron".to_string(),
            tz: None,
        };
        assert_eq!(schedule.next_run(0), None);
    }

    #[test]
    fn test_add_and_list_jobs() {
        let (_tmp, mut svc) = temp_cron_service();

        assert!(svc.list_jobs(true).is_empty());

        svc.add_job(
            "test-job",
            CronSchedule::Every { every_ms: 60000 },
            "do something",
            false,
            None,
            None,
        );

        let jobs = svc.list_jobs(false);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].name, "test-job");
        assert!(jobs[0].enabled);
    }

    #[test]
    fn test_remove_job() {
        let (_tmp, mut svc) = temp_cron_service();

        let job = svc.add_job(
            "remove-me",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            false,
            None,
            None,
        );

        assert_eq!(svc.list_jobs(true).len(), 1);
        assert!(svc.remove_job(&job.id));
        assert!(svc.list_jobs(true).is_empty());
    }

    #[test]
    fn test_enable_disable_job() {
        let (_tmp, mut svc) = temp_cron_service();

        let job = svc.add_job(
            "toggle-job",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            false,
            None,
            None,
        );

        // Disable
        let updated = svc.enable_job(&job.id, false).unwrap();
        assert!(!updated.enabled);
        assert!(updated.state.next_run_at_ms.is_none());

        // Visible only with include_disabled
        assert!(svc.list_jobs(false).is_empty());
        assert_eq!(svc.list_jobs(true).len(), 1);

        // Re-enable
        let updated = svc.enable_job(&job.id, true).unwrap();
        assert!(updated.enabled);
        assert!(updated.state.next_run_at_ms.is_some());
    }

    #[test]
    fn test_get_due_jobs() {
        let (_tmp, mut svc) = temp_cron_service();

        // Add job with next_run in the past
        let _job = svc.add_job(
            "due-job",
            CronSchedule::Every { every_ms: 1 }, // 1ms interval
            "msg",
            false,
            None,
            None,
        );

        // Sleep briefly so job becomes due
        std::thread::sleep(std::time::Duration::from_millis(5));

        let due = svc.get_due_jobs();
        assert!(!due.is_empty());
        assert_eq!(due[0].name, "due-job");
    }

    #[test]
    fn test_mark_executed() {
        let (_tmp, mut svc) = temp_cron_service();

        let job = svc.add_job(
            "exec-job",
            CronSchedule::Every { every_ms: 60000 },
            "msg",
            false,
            None,
            None,
        );

        svc.mark_executed(&job.id, "ok", None);

        let jobs = svc.list_jobs(true);
        assert_eq!(jobs[0].state.last_status.as_deref(), Some("ok"));
        assert!(jobs[0].state.last_run_at_ms.is_some());
    }

    #[test]
    fn test_persistence() {
        let tmp = tempfile::tempdir().unwrap();
        let store_path = tmp.path().join("cron").join("jobs.json");

        // Create and save a job
        {
            let mut svc = CronService::new(store_path.clone());
            svc.init();
            svc.add_job("persistent", CronSchedule::Every { every_ms: 1000 }, "msg", false, None, None);
        }

        // Reload and verify
        {
            let mut svc = CronService::new(store_path);
            svc.init();
            let jobs = svc.list_jobs(true);
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].name, "persistent");
        }
    }

    #[test]
    fn test_cron_status() {
        let (_tmp, svc) = temp_cron_service();
        let status = svc.status();
        assert_eq!(status["jobs"], 0);
    }

    #[test]
    fn test_cron_schedule_serde() {
        let schedule = CronSchedule::Every { every_ms: 5000 };
        let json = serde_json::to_string(&schedule).unwrap();
        assert!(json.contains("every"));
        let parsed: CronSchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.kind_str(), "every");
    }
}
