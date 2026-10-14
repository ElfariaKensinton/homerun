use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

/// Reason step-log retrieval failed, preserving local-state and upstream API errors.
#[derive(Debug)]
pub enum StepLogError {
    NotFound(String),
    Unauthorized(String),
    Upstream(String),
}

/// Status of a single job step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

/// Information about a single job step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepInfo {
    pub number: u16,
    pub name: String,
    pub status: StepStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// API response for step progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepsResponse {
    pub job_name: String,
    pub steps: Vec<StepInfo>,
    pub steps_discovered: usize,
}

/// Events parsed from Worker log lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepEvent {
    Discovered {
        name: String,
        timestamp: DateTime<Utc>,
    },
    Started {
        timestamp: DateTime<Utc>,
    },
    Completed {
        result: StepStatus,
        timestamp: DateTime<Utc>,
    },
}

/// Parse a Worker log timestamp in `YYYY-MM-DD HH:MM:SSZ` format.
fn parse_worker_timestamp(s: &str) -> Option<DateTime<Utc>> {
    // Expected: "2026-03-23 07:54:53Z" — strip trailing 'Z' for NaiveDateTime parse
    let trimmed = s.strip_suffix('Z')?;
    let naive = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(naive.and_utc())
}

/// Parse a single Worker log line into a [`StepEvent`], if it matches a known pattern.
///
/// Expected patterns:
/// - `[<ts> INFO StepsRunner] Processing step: DisplayName='<name>'` → Discovered
/// - `[<ts> INFO StepsRunner] Starting the step.` → Started
/// - `[<ts> INFO StepsRunner] … current step result '<Result>'.` → Completed
pub fn parse_step_event(line: &str) -> Option<StepEvent> {
    // Early return if not a StepsRunner line
    if !line.contains("StepsRunner]") {
        return None;
    }

    // Extract timestamp from the leading bracket: [YYYY-MM-DD HH:MM:SSZ ...]
    let ts_start = line.find('[')? + 1;
    let ts_end = ts_start + "2026-03-23 07:54:53Z".len();
    if ts_end > line.len() {
        return None;
    }
    let timestamp = parse_worker_timestamp(&line[ts_start..ts_end])?;

    // Match patterns
    if let Some(idx) = line.find("DisplayName='") {
        let name_start = idx + "DisplayName='".len();
        let name_end = line[name_start..].find('\'')?;
        let name = line[name_start..name_start + name_end].to_string();
        return Some(StepEvent::Discovered { name, timestamp });
    }

    if line.contains("Starting the step.") {
        return Some(StepEvent::Started { timestamp });
    }

    if let Some(idx) = line.find("current step result '") {
        let result_start = idx + "current step result '".len();
        let result_end = line[result_start..].find('\'')?;
        let result_str = &line[result_start..result_start + result_end];
        let result = match result_str {
            "Succeeded" => StepStatus::Succeeded,
            "Failed" => StepStatus::Failed,
            "Skipped" => StepStatus::Skipped,
            "Cancelled" => StepStatus::Cancelled,
            _ => return None,
        };
        return Some(StepEvent::Completed { result, timestamp });
    }

    None
}

/// Internal state for tracking a single runner's step progress.
struct RunnerStepState {
    job_name: String,
    steps: Vec<StepInfo>,
    step_logs: HashMap<u16, Vec<String>>,
    step_log_bytes: HashMap<u16, usize>,
    file_offset: u64,
    log_path: Option<PathBuf>,
    work_dir: PathBuf,
}

/// Watches Worker log files to track step progress for runners.
///
/// Designed to be shared across async tasks via `Arc<RwLock<>>` interior.
#[derive(Clone)]
pub struct WorkerLogWatcher {
    step_state: Arc<RwLock<HashMap<String, RunnerStepState>>>,
}

impl Default for WorkerLogWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerLogWatcher {
    /// Create a new empty watcher.
    pub fn new() -> Self {
        Self {
            step_state: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Begin watching a runner's Worker log for step progress.
    ///
    /// Stores the work directory; the actual log file is discovered lazily
    /// during the first `poll()` call, avoiding races when the Worker
    /// process hasn't spawned yet.
    pub async fn start_watching(&self, runner_id: &str, job_name: &str, work_dir: &Path) {
        let state = RunnerStepState {
            job_name: job_name.to_string(),
            steps: Vec::new(),
            step_logs: HashMap::new(),
            step_log_bytes: HashMap::new(),
            file_offset: 0,
            log_path: None,
            work_dir: work_dir.to_path_buf(),
        };
        self.step_state
            .write()
            .await
            .insert(runner_id.to_string(), state);
    }

    /// Stop watching a runner and remove its state.
    pub async fn stop_watching(&self, runner_id: &str) {
        self.step_state.write().await.remove(runner_id);
    }

    /// Poll a runner's Worker log for new step events.
    ///
    /// Returns `false` if the runner is not being watched (signal to stop polling).
    pub async fn poll(&self, runner_id: &str) -> bool {
        let mut map = self.step_state.write().await;
        let Some(state) = map.get_mut(runner_id) else {
            return false;
        };

        // If we don't have a log path yet, try to find one
        if state.log_path.is_none() {
            state.log_path = find_latest_worker_log(&state.work_dir);
            if state.log_path.is_none() {
                return true; // Keep polling, log not created yet
            }
        }

        {
            let log_path = state.log_path.as_ref().unwrap();
            let Ok(metadata) = std::fs::metadata(log_path) else {
                return true;
            };

            let file_len = metadata.len();
            if file_len <= state.file_offset {
                // No new bytes — check if a newer log file appeared
                if let Some(newer) = find_latest_worker_log(&state.work_dir) {
                    if newer != *log_path {
                        state.log_path = Some(newer);
                        state.file_offset = 0;
                        // Fall through to read the new file immediately
                    } else {
                        return true;
                    }
                } else {
                    return true;
                }
            }
        }

        let log_path = state.log_path.as_ref().unwrap();
        let Ok(content) = std::fs::read_to_string(log_path) else {
            return true;
        };

        // Consume only complete lines. If the runner is in the middle of
        // writing a line, leave the offset at the last newline and retry it next poll.
        let all_bytes = content.as_bytes();
        let start = (state.file_offset as usize).min(all_bytes.len());
        let new_bytes = &all_bytes[start..];
        let complete_len = new_bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let new_text = String::from_utf8_lossy(&new_bytes[..complete_len]);

        for line in new_text.lines() {
            if is_job_start_line(line) {
                state.steps.clear();
                state.step_logs.clear();
                state.step_log_bytes.clear();
                continue;
            }

            if let Some(event) = parse_step_event(line) {
                apply_step_event(&mut state.steps, event);
                continue;
            }

            // Only accept lines explicitly mirrored by our patched runner.
            // Worker diagnostics otherwise contain internal traces unrelated to
            // command output and must not appear in a user's live step log.
            if let Some(output) = mirrored_step_output(line) {
                if let Some(step_number) = state
                    .steps
                    .iter()
                    .rev()
                    .find(|step| step.status == StepStatus::Running)
                    .map(|step| step.number)
                {
                    append_step_log(
                        &mut state.step_logs,
                        &mut state.step_log_bytes,
                        step_number,
                        output,
                    );
                }
            }
        }

        state.file_offset = (start + complete_len) as u64;
        true
    }

    /// Get the current step state for a runner.
    pub async fn get_steps(&self, runner_id: &str) -> Option<StepsResponse> {
        let map = self.step_state.read().await;
        let state = map.get(runner_id)?;
        Some(StepsResponse {
            job_name: state.job_name.clone(),
            steps: state.steps.clone(),
            steps_discovered: state.steps.len(),
        })
    }

    /// Return log lines captured so far for a step that is being watched.
    pub async fn get_step_logs(&self, runner_id: &str, step_number: u16) -> Option<Vec<String>> {
        let map = self.step_state.read().await;
        let state = map.get(runner_id)?;
        Some(state.step_logs.get(&step_number).cloned().unwrap_or_default())
    }

    /// Reconstruct step logs from the last on-disk Worker log. The runner keeps
    /// these files after the job finishes, so step output remains available
    /// after the live watcher has been stopped or the daemon has restarted.
    pub fn read_step_logs_from_disk(work_dir: &Path, step_number: u16) -> Vec<String> {
        // Worker diagnostics roll over into multiple Worker log pages when they grow
        // large. Replay every page in order and reset only at the start of a new job.
        let log_paths = list_worker_logs(work_dir);
        if log_paths.is_empty() {
            return Vec::new();
        }

        let mut steps: Vec<StepInfo> = Vec::new();
        let mut logs: HashMap<u16, Vec<String>> = HashMap::new();
        let mut log_bytes: HashMap<u16, usize> = HashMap::new();

        for log_path in log_paths {
            let Ok(content) = std::fs::read_to_string(log_path) else {
                continue;
            };

            for line in content.lines() {
                if is_job_start_line(line) {
                    steps.clear();
                    logs.clear();
                    log_bytes.clear();
                    continue;
                }

                if let Some(event) = parse_step_event(line) {
                    apply_step_event(&mut steps, event);
                    continue;
                }

                if let Some(output) = mirrored_step_output(line) {
                    if let Some(number) = steps
                        .iter()
                        .rev()
                        .find(|step| step.status == StepStatus::Running)
                        .map(|step| step.number)
                    {
                        append_step_log(&mut logs, &mut log_bytes, number, output);
                    }
                }
            }
        }

        logs.remove(&step_number).unwrap_or_default()
    }
}

const MIRRORED_STEP_OUTPUT_MARKER: &str = "HOMERUN_STEP_OUTPUT::";

/// Extract only command output deliberately mirrored by the patched runner.
/// Other Worker diagnostic lines are operational noise, not user-facing step logs.
fn mirrored_step_output(line: &str) -> Option<&str> {
    let marker_index = line.find(MIRRORED_STEP_OUTPUT_MARKER)?;
    let output_start = marker_index + MIRRORED_STEP_OUTPUT_MARKER.len();
    Some(line[output_start..].strip_prefix(' ').unwrap_or(&line[output_start..]))
}

const MAX_STEP_LOG_LINES: usize = 4_000;
const MAX_STEP_LOG_BYTES: usize = 1024 * 1024;

/// Strip the runner's timestamp/logger prefix so the view contains readable
/// execution output while retaining the original line when no prefix is found.
fn worker_log_message(line: &str) -> String {
    if line.starts_with('[') {
        if let Some(end) = line.find("] ") {
            return line[end + 2..].to_string();
        }
    }
    line.to_string()
}

fn append_step_log(
    logs: &mut HashMap<u16, Vec<String>>,
    byte_counts: &mut HashMap<u16, usize>,
    step_number: u16,
    line: &str,
) {
    let mut message = worker_log_message(line);
    if message.is_empty() {
        return;
    }

    // Prevent one noisy build step from growing daemon memory/history without bound.
    if message.len() > MAX_STEP_LOG_BYTES {
        message = message.chars().take(MAX_STEP_LOG_BYTES).collect();
    }

    let entries = logs.entry(step_number).or_default();
    let bytes = byte_counts.entry(step_number).or_default();
    *bytes += message.len();
    entries.push(message);

    while entries.len() > MAX_STEP_LOG_LINES || *bytes > MAX_STEP_LOG_BYTES {
        if entries.is_empty() {
            *bytes = 0;
            break;
        }
        *bytes = bytes.saturating_sub(entries.remove(0).len());
    }
}

/// Apply a step event to the steps list.
fn apply_step_event(steps: &mut Vec<StepInfo>, event: StepEvent) {
    match event {
        StepEvent::Discovered { name, timestamp: _ } => {
            let number = steps.len() as u16 + 1;
            steps.push(StepInfo {
                number,
                name,
                status: StepStatus::Pending,
                started_at: None,
                completed_at: None,
            });
        }
        StepEvent::Started { timestamp } => {
            if let Some(step) = steps
                .iter_mut()
                .rev()
                .find(|s| s.status == StepStatus::Pending)
            {
                step.status = StepStatus::Running;
                step.started_at = Some(timestamp);
            }
        }
        StepEvent::Completed { result, timestamp } => {
            if let Some(step) = steps
                .iter_mut()
                .rev()
                .find(|s| s.status == StepStatus::Running)
            {
                step.status = result;
                step.completed_at = Some(timestamp);
            }
        }
    }
}

/// True when a diagnostic line marks the beginning of a new job in the Worker process.
/// Log page rollover does not repeat this marker, so step state can safely span pages.
fn is_job_start_line(line: &str) -> bool {
    line.contains(" INFO JobRunner] Starting the job execution context.")
}

/// List Worker diagnostic pages in filename/time order for replay across page rollovers.
fn list_worker_logs(work_dir: &Path) -> Vec<PathBuf> {
    let diag_dir = work_dir.join("_diag");
    let read_dir = match std::fs::read_dir(&diag_dir) {
        Ok(rd) => rd,
        Err(e) => {
            warn!(
                "Failed to read _diag directory at {}: {}",
                diag_dir.display(),
                e
            );
            return Vec::new();
        }
    };

    let mut paths = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if file_name.starts_with("Worker_") && file_name.ends_with(".log") {
            paths.push(path);
        }
    }
    paths.sort();
    paths
}

/// Find the newest Worker log page in the _diag directory.
fn find_latest_worker_log(work_dir: &Path) -> Option<PathBuf> {
    list_worker_logs(work_dir).into_iter().max_by_key(|path| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn test_parse_step_discovered() {
        let line =
            "[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Run actions/checkout@v6'";
        let event = parse_step_event(line);
        assert!(event.is_some());
        match event.unwrap() {
            StepEvent::Discovered { name, .. } => {
                assert_eq!(name, "Run actions/checkout@v6");
            }
            other => panic!("Expected Discovered, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_step_started() {
        let line = "[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.";
        let event = parse_step_event(line);
        assert!(event.is_some());
        assert!(matches!(event.unwrap(), StepEvent::Started { .. }));
    }

    #[test]
    fn test_parse_step_succeeded() {
        let line = "[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.";
        let event = parse_step_event(line);
        assert!(event.is_some());
        match event.unwrap() {
            StepEvent::Completed { result, .. } => assert_eq!(result, StepStatus::Succeeded),
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_step_failed() {
        let line = "[2026-03-23 07:54:55Z INFO StepsRunner] Updating job result with current step result 'Failed'.";
        let event = parse_step_event(line);
        assert!(event.is_some());
        match event.unwrap() {
            StepEvent::Completed { result, .. } => assert_eq!(result, StepStatus::Failed),
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_step_skipped() {
        let line = "[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Skipped'.";
        let event = parse_step_event(line);
        assert!(event.is_some());
        match event.unwrap() {
            StepEvent::Completed { result, .. } => assert_eq!(result, StepStatus::Skipped),
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_unrelated_line_returns_none() {
        let line = "[2026-03-23 07:54:53Z INFO JobRunner] Starting job execution";
        assert!(parse_step_event(line).is_none());
    }

    #[test]
    fn test_parse_timestamp_extracted_correctly() {
        let line = "[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Build'";
        let event = parse_step_event(line).unwrap();
        let expected = Utc.with_ymd_and_hms(2026, 3, 23, 7, 54, 53).unwrap();
        match event {
            StepEvent::Discovered { timestamp, .. } => assert_eq!(timestamp, expected),
            other => panic!("Expected Discovered, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_watcher_processes_step_events() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();

        let log_path = diag.join("Worker_20260323-075453-utc.log");
        let log_content = "\
[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Checkout'\n\
[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.\n\
[2026-03-23 07:54:54Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: checkout output line\n\
[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.\n\
[2026-03-23 07:54:55Z INFO StepsRunner] Processing step: DisplayName='Build'\n\
[2026-03-23 07:54:55Z INFO StepsRunner] Starting the step.\n";
        std::fs::write(&log_path, log_content).unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-1", "test-job", dir.path())
            .await;
        let result = watcher.poll("runner-1").await;
        assert!(result);

        let resp = watcher.get_steps("runner-1").await.unwrap();
        assert_eq!(resp.job_name, "test-job");
        assert_eq!(resp.steps.len(), 2);
        assert_eq!(resp.steps[0].name, "Checkout");
        assert_eq!(resp.steps[0].status, StepStatus::Succeeded);
        assert!(resp.steps[0].completed_at.is_some());
        assert_eq!(
            watcher.get_step_logs("runner-1", 1).await.unwrap(),
            vec!["checkout output line".to_string()]
        );
        assert_eq!(resp.steps[1].name, "Build");
        assert_eq!(resp.steps[1].status, StepStatus::Running);
        assert!(resp.steps[1].started_at.is_some());
    }

    #[test]
    fn test_read_step_logs_from_disk_after_job_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();
        let log_path = diag.join("Worker_20260323-075453-utc.log");
        let content = "\
[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Checkout'\n\
[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.\n\
[2026-03-23 07:54:54Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: checkout output line\n\
[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.\n";
        std::fs::write(&log_path, content).unwrap();

        assert_eq!(
            WorkerLogWatcher::read_step_logs_from_disk(dir.path(), 1),
            vec!["checkout output line".to_string()]
        );
    }

    #[tokio::test]
    async fn test_watcher_incremental_reads() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();

        let log_path = diag.join("Worker_20260323-075453-utc.log");
        let initial =
            "[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Setup'\n\
[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.\n";
        std::fs::write(&log_path, initial).unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-2", "inc-job", dir.path())
            .await;
        watcher.poll("runner-2").await;

        let resp = watcher.get_steps("runner-2").await.unwrap();
        assert_eq!(resp.steps.len(), 1);
        assert_eq!(resp.steps[0].status, StepStatus::Running);

        // Append more content
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        writeln!(
            file,
            "[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'."
        )
        .unwrap();

        watcher.poll("runner-2").await;

        let resp = watcher.get_steps("runner-2").await.unwrap();
        assert_eq!(resp.steps.len(), 1);
        assert_eq!(resp.steps[0].status, StepStatus::Succeeded);
        assert!(resp.steps[0].completed_at.is_some());
    }

    #[tokio::test]
    async fn test_watcher_stop_clears_state() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();
        std::fs::write(diag.join("Worker_20260323-075453-utc.log"), "").unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-3", "stop-job", dir.path())
            .await;
        assert!(watcher.get_steps("runner-3").await.is_some());

        watcher.stop_watching("runner-3").await;
        assert!(watcher.get_steps("runner-3").await.is_none());
    }

    #[test]
    fn test_parse_step_cancelled() {
        let line = "[2026-03-23 07:54:55Z INFO StepsRunner] Updating job result with current step result 'Cancelled'.";
        let event = parse_step_event(line);
        assert!(event.is_some());
        match event.unwrap() {
            StepEvent::Completed { result, .. } => assert_eq!(result, StepStatus::Cancelled),
            other => panic!("Expected Completed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_poll_retries_log_discovery() {
        let dir = tempfile::tempdir().unwrap();
        // No _diag directory — simulates Worker not spawned yet

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-retry", "retry-job", dir.path())
            .await;

        // First poll: no log file yet, returns true (keep polling), no steps
        assert!(watcher.poll("runner-retry").await);
        let resp = watcher.get_steps("runner-retry").await.unwrap();
        assert_eq!(resp.steps.len(), 0);

        // Now create the _diag dir and Worker log (simulates Worker spawning)
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();
        let log_content = "\
[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Checkout'\n\
[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.\n\
[2026-03-23 07:54:55Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.\n";
        std::fs::write(diag.join("Worker_20260323-080000-utc.log"), log_content).unwrap();

        // Second poll: should discover the log and parse steps
        assert!(watcher.poll("runner-retry").await);
        let resp = watcher.get_steps("runner-retry").await.unwrap();
        assert_eq!(resp.steps.len(), 1);
        assert_eq!(resp.steps[0].name, "Checkout");
        assert_eq!(resp.steps[0].status, StepStatus::Succeeded);
    }

    #[tokio::test]
    async fn test_watcher_no_diag_dir_returns_empty_steps() {
        let dir = tempfile::tempdir().unwrap();
        // No _diag directory created

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-4", "no-diag-job", dir.path())
            .await;
        watcher.poll("runner-4").await;

        let resp = watcher.get_steps("runner-4").await.unwrap();
        assert_eq!(resp.steps.len(), 0);
        assert_eq!(resp.job_name, "no-diag-job");
    }

    #[tokio::test]
    async fn test_poll_detects_newer_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();

        // Create an "old" Worker log with one step
        let old_log = diag.join("Worker_20260323-070000-utc.log");
        let old_content = [
            "[2026-03-23 07:00:00Z INFO JobRunner] Starting the job execution context.",
            "[2026-03-23 07:00:00Z INFO StepsRunner] Processing step: DisplayName='OldStep'",
            "[2026-03-23 07:00:00Z INFO StepsRunner] Starting the step.",
            "[2026-03-23 07:00:01Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: old job output",
            "[2026-03-23 07:00:01Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.",
        ].join("\n") + "\n";
        std::fs::write(&old_log, old_content).unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-stale", "stale-job", dir.path())
            .await;

        // First poll reads old log
        assert!(watcher.poll("runner-stale").await);
        let resp = watcher.get_steps("runner-stale").await.unwrap();
        assert_eq!(resp.steps.len(), 1);
        assert_eq!(resp.steps[0].name, "OldStep");

        // Create a newer Worker log (simulates new job's Worker process)
        // Sleep briefly so the new file has a strictly newer mtime
        std::thread::sleep(std::time::Duration::from_millis(50));
        let new_log = diag.join("Worker_20260323-080000-utc.log");
        let new_content = [
            "[2026-03-23 08:00:00Z INFO JobRunner] Starting the job execution context.",
            "[2026-03-23 08:00:00Z INFO StepsRunner] Processing step: DisplayName='NewStep'",
            "[2026-03-23 08:00:00Z INFO StepsRunner] Starting the step.",
        ].join("\n") + "\n";
        std::fs::write(&new_log, new_content).unwrap();

        // Second poll: no new bytes on old file → checks for newer log → switches
        assert!(watcher.poll("runner-stale").await);
        let resp = watcher.get_steps("runner-stale").await.unwrap();
        assert_eq!(resp.steps.len(), 1);
        assert_eq!(resp.steps[0].name, "NewStep");
        assert_eq!(resp.steps[0].status, StepStatus::Running);
    }

    #[tokio::test]
    async fn test_watcher_keeps_running_step_logs_across_worker_log_page_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();

        let first_page = diag.join("Worker_20260323-075453-utc.log");
        let first_content = [
            "[2026-03-23 07:54:53Z INFO JobRunner] Starting the job execution context.",
            "[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Build'",
            "[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.",
            "[2026-03-23 07:54:54Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: output before page rotation",
        ].join("\n") + "\n";
        std::fs::write(&first_page, first_content).unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-rotation", "rotation-job", dir.path())
            .await;
        assert!(watcher.poll("runner-rotation").await);
        assert_eq!(
            watcher.get_step_logs("runner-rotation", 1).await.unwrap(),
            vec!["output before page rotation".to_string()]
        );

        std::thread::sleep(std::time::Duration::from_millis(50));
        let second_page = diag.join("Worker_20260323-075454-utc.log");
        let second_content = [
            "[2026-03-23 07:54:55Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: output after page rotation",
            "[2026-03-23 07:54:56Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.",
        ].join("\n") + "\n";
        std::fs::write(&second_page, second_content).unwrap();

        assert!(watcher.poll("runner-rotation").await);
        let response = watcher.get_steps("runner-rotation").await.unwrap();
        assert_eq!(response.steps.len(), 1);
        assert_eq!(response.steps[0].status, StepStatus::Succeeded);
        assert_eq!(
            watcher.get_step_logs("runner-rotation", 1).await.unwrap(),
            vec![
                "output before page rotation".to_string(),
                "output after page rotation".to_string()
            ]
        );
    }

    #[test]
    fn test_read_step_logs_from_disk_across_worker_pages_and_job_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();

        std::fs::write(
            diag.join("Worker_20260323-070000-utc.log"),
            [
                "[2026-03-23 07:00:00Z INFO JobRunner] Starting the job execution context.",
                "[2026-03-23 07:00:00Z INFO StepsRunner] Processing step: DisplayName='OldStep'",
                "[2026-03-23 07:00:00Z INFO StepsRunner] Starting the step.",
                "[2026-03-23 07:00:01Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: old job output",
                "[2026-03-23 07:00:02Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.",
            ].join("\n") + "\n"
        ).unwrap();

        std::fs::write(
            diag.join("Worker_20260323-075453-utc.log"),
            [
                "[2026-03-23 07:54:53Z INFO JobRunner] Starting the job execution context.",
                "[2026-03-23 07:54:53Z INFO StepsRunner] Processing step: DisplayName='Checkout'",
                "[2026-03-23 07:54:53Z INFO StepsRunner] Starting the step.",
                "[2026-03-23 07:54:54Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: checkout output before rotation",
            ].join("\n") + "\n"
        ).unwrap();

        std::fs::write(
            diag.join("Worker_20260323-075454-utc.log"),
            [
                "[2026-03-23 07:54:55Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: checkout output after rotation",
                "[2026-03-23 07:54:56Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.",
            ].join("\n") + "\n"
        ).unwrap();

        assert_eq!(
            WorkerLogWatcher::read_step_logs_from_disk(dir.path(), 1),
            vec![
                "checkout output before rotation".to_string(),
                "checkout output after rotation".to_string()
            ]
        );
        assert!(WorkerLogWatcher::read_step_logs_from_disk(dir.path(), 2).is_empty());
    }

    #[tokio::test]
    async fn test_watcher_filters_internal_worker_diagnostics_from_live_output() {
        let dir = tempfile::tempdir().unwrap();
        let diag = dir.path().join("_diag");
        std::fs::create_dir_all(&diag).unwrap();
        let log_path = diag.join("Worker_20261014-000000-utc.log");
        let log_content = [
            "[2026-10-14 00:00:00Z INFO StepsRunner] Processing step: DisplayName='Build'",
            "[2026-10-14 00:00:00Z INFO StepsRunner] Starting the step.",
            "[2026-10-14 00:00:01Z INFO Worker] internal worker diagnostic",
            "[2026-10-14 00:00:01Z INFO ExecutionContext] ordinary execution trace",
            "[2026-10-14 00:00:02Z INFO ExecutionContext] HOMERUN_STEP_OUTPUT:: make all",
            "[2026-10-14 00:00:03Z INFO StepsRunner] No need for updating job result with current step result 'Succeeded'.",
        ].join("\n") + "\n";
        std::fs::write(&log_path, log_content).unwrap();

        let watcher = WorkerLogWatcher::new();
        watcher
            .start_watching("runner-filter", "filter-job", dir.path())
            .await;
        assert!(watcher.poll("runner-filter").await);
        assert_eq!(
            watcher.get_step_logs("runner-filter", 1).await.unwrap(),
            vec!["make all".to_string()]
        );
    }

}
