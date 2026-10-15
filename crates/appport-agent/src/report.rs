//! Run logging and metrics.
//!
//! Every step writes one JSON line to the run log (when one is given) and a
//! short line to stderr. The report summarises what the deterministic core
//! established on its own and what the model was used for.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use agent_core::verify::Verdict;
use agent_model::Usage;
use serde::Serialize;
use serde_json::{json, Value};

/// The current UTC time as RFC 3339 with milliseconds.
pub fn now_rfc3339() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let (secs, millis) = (now.as_secs() as i64, now.subsec_millis());
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

/// The run log.
pub struct Log {
    start: Instant,
    file: Option<File>,
    quiet: bool,
}

impl Log {
    pub fn new(path: Option<&Path>, quiet: bool) -> std::io::Result<Log> {
        let file = match path {
            Some(path) => {
                if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    std::fs::create_dir_all(parent)?;
                }
                Some(File::create(path)?)
            }
            None => None,
        };
        Ok(Log { start: Instant::now(), file, quiet })
    }

    pub fn elapsed_ms(&self) -> u128 {
        self.start.elapsed().as_millis()
    }

    /// Records one step: `kind` and its data in the log, `human` on stderr.
    pub fn event(&mut self, kind: &str, data: Value, human: impl AsRef<str>) {
        if let Some(file) = &mut self.file {
            let mut record = json!({ "tMs": self.start.elapsed().as_millis() as u64, "kind": kind });
            if let (Value::Object(record), Value::Object(data)) = (&mut record, data) {
                record.extend(data);
            }
            let _ = writeln!(file, "{record}");
        }
        if !self.quiet && !human.as_ref().is_empty() {
            for line in human.as_ref().lines() {
                eprintln!("[{:>6} ms] {kind:<12} {line}", self.start.elapsed().as_millis());
            }
        }
    }
}

/// One model attempt.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attempt {
    pub attempt: u32,
    pub model: String,
    pub usage: Usage,
    pub prompt_bytes: usize,
    pub response_bytes: usize,
    pub files_changed: Vec<String>,
    /// Why the response could not be applied, if it could not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unusable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<Verdict>,
    pub duration_ms: u64,
}

/// What the agent did without the model.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Deterministic {
    pub source_files_observed: usize,
    pub capabilities_in_contract: usize,
    pub capabilities_located_in_source: usize,
    pub graph_nodes: usize,
    pub graph_edges: usize,
    pub dependents_of_target: usize,
    pub replacement_constraints: usize,
    pub acceptance_checks: usize,
    pub decisions: usize,
    pub permissions_granted: Vec<String>,
    pub application_runs: usize,
    pub envelopes_exchanged: usize,
    pub verification_methods_evaluated: usize,
    /// Always zero: discovery, planning and acceptance never call the model.
    pub model_calls_for_discovery: usize,
    pub model_calls_for_verification: usize,
}

/// What was sent to the model, against what exists.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Context {
    pub files_sent: usize,
    pub project_source_files: usize,
    pub source_bytes_sent: usize,
    pub project_source_bytes: usize,
    pub knowledge_bytes_sent: usize,
}

/// Totals for the whole run.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub model_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub tokens_estimated: bool,
    pub rejected_attempts: usize,
    pub wall_ms: u64,
}

/// The run report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReport {
    pub intent: String,
    pub project: String,
    pub application: String,
    pub model: String,
    /// `accepted`, or `rejected` when no attempt passed every check.
    pub outcome: String,
    /// Checks the unchanged application already failed: why the change was needed.
    pub baseline_failures: Vec<String>,
    pub attempts: Vec<Attempt>,
    pub totals: Totals,
    pub deterministic: Deterministic,
    pub context: Context,
    pub started_at: String,
}
