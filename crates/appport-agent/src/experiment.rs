//! Experiment specifications: a versioned, serializable description of a
//! comparison, run by the existing coding evaluation ([`crate::coding::coding_eval`]).
//!
//! A specification names:
//! - the task suite;
//! - the model, through the existing model configuration files;
//! - the arms (a named set of strategies each), and which arm is the baseline;
//! - trials, limits and the output directory.
//!
//! An `ablation` derives arms from one base arm by varying one component, so a
//! difference can be traced to that component alone.
//!
//! ```json
//! {
//!   "schema": "secret-agent.experiment/1",
//!   "id": "context-ablation",
//!   "suite": "fixtures/coding",
//!   "model": { "config": "examples/models/vercel-gateway.json" },
//!   "arms": [{ "id": "baseline", "context": "observed", "recovery": "feedback" }],
//!   "ablation": { "from": "baseline", "vary": "context", "values": ["minimal", "dependency-aware", "appport-aware"] },
//!   "trials": 3
//! }
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_model::{ModelConfig, Provider};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::coding::{CodingArm, CodingEvalOptions, Limits};
use crate::host::HostConfig;
use crate::strategy::{ContextStrategyId, RecoveryPolicyId};

/// The specification format version this build reads.
pub const EXPERIMENT_SCHEMA: &str = "secret-agent.experiment/1";

/// An experiment specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExperimentSpec {
    pub schema: String,
    pub id: String,
    /// A directory of coding tasks (each with `task.json`), relative to the root.
    pub suite: PathBuf,
    /// Task ids to run; all tasks in the suite when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
    pub model: ModelSpec,
    #[serde(default)]
    pub arms: Vec<CodingArm>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ablation: Option<Ablation>,
    /// The arm the others are compared with; the first arm when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<String>,
    #[serde(default = "one")]
    pub trials: u32,
    #[serde(default = "three")]
    pub max_attempts: u32,
    #[serde(default = "two_minutes")]
    pub test_timeout_secs: u64,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default = "yes")]
    pub balanced_order: bool,
    /// Require the free-gateway preflight. Always applied when the model is at the gateway.
    #[serde(default)]
    pub free_gateway: bool,
    /// Where results go, relative to the root; `target/experiments/<id>` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PathBuf>,
}

fn one() -> u32 {
    1
}
fn three() -> u32 {
    3
}
fn two_minutes() -> u64 {
    120
}
fn yes() -> bool {
    true
}

/// The model: an existing configuration file, with optional overrides of its
/// fields (the same names, checked the same way). No second configuration system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<serde_json::Map<String, Value>>,
}

/// Arms derived from one base arm by varying one component.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Ablation {
    /// The id of the base arm.
    pub from: String,
    /// `context`, `recovery` or `contextBudget`.
    pub vary: String,
    pub values: Vec<Value>,
}

/// A specification, checked and resolved into evaluation options.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub options: CodingEvalOptions,
    /// The specification with arms expanded and the model configuration filled
    /// in, as recorded in the report.
    pub record: Value,
}

/// Reads a specification file.
pub fn read_spec(path: &Path) -> Result<ExperimentSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn model_config(spec: &ModelSpec, root: &Path) -> Result<ModelConfig, String> {
    let mut value = match &spec.config {
        Some(path) => {
            let path = root.join(path);
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            serde_json::from_str::<Value>(&text).map_err(|e| format!("{}: {e}", path.display()))?
        }
        None => Value::Object(Default::default()),
    };
    if let (Value::Object(base), Some(overrides)) = (&mut value, &spec.overrides) {
        base.extend(overrides.clone());
    }
    serde_json::from_value(value).map_err(|e| format!("model: {e}"))
}

fn expand(arms: &mut Vec<CodingArm>, ablation: &Ablation) -> Result<(), String> {
    let base = arms
        .iter()
        .find(|a| a.id == ablation.from)
        .cloned()
        .ok_or_else(|| format!("ablation.from {:?} is not an arm", ablation.from))?;
    if ablation.values.is_empty() {
        return Err("ablation.values is empty".into());
    }
    for value in &ablation.values {
        let mut arm = base.clone();
        let text = value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string());
        match ablation.vary.as_str() {
            "context" => arm.strategies.context = text.parse::<ContextStrategyId>()?,
            "recovery" => arm.strategies.recovery = text.parse::<RecoveryPolicyId>()?,
            "contextBudget" => {
                arm.strategies.context_budget =
                    value.as_u64().ok_or_else(|| format!("contextBudget value {value} is not a whole number"))? as usize
            }
            other => return Err(format!("ablation.vary {other:?}: use context, recovery or contextBudget")),
        }
        if arm.strategies == base.strategies {
            continue;
        }
        arm.id = format!("{}={text}", ablation.vary);
        arms.push(arm);
    }
    Ok(())
}

/// Checks a specification and resolves it against `root` (the repository root).
pub fn resolve(spec: &ExperimentSpec, root: &Path) -> Result<Resolved, String> {
    if spec.schema != EXPERIMENT_SCHEMA {
        return Err(format!("schema {:?} is not supported (this build reads {EXPERIMENT_SCHEMA})", spec.schema));
    }
    if spec.id.is_empty() || !spec.id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        return Err(format!("id {:?} must be non-empty and use only letters, digits, '-', '_' and '.'", spec.id));
    }
    if spec.trials == 0 || spec.max_attempts == 0 {
        return Err("trials and maxAttempts must be at least 1".into());
    }
    let model = model_config(&spec.model, root)?;
    // Experiments run on free gateway models, local servers, or scripted completions.
    if model.provider == Provider::Anthropic {
        return Err("experiments do not use the anthropic provider: use a free Vercel AI Gateway model, a local OpenAI-compatible server, or scripted completions".into());
    }
    let mut arms = spec.arms.clone();
    if arms.is_empty() {
        return Err("at least one arm is needed".into());
    }
    if let Some(ablation) = &spec.ablation {
        expand(&mut arms, ablation)?;
    }
    let mut ids = std::collections::BTreeSet::new();
    for arm in &arms {
        if arm.id.is_empty() || !ids.insert(arm.id.clone()) {
            return Err(format!("arm id {:?} is empty or used twice", arm.id));
        }
    }
    let baseline = spec.baseline.clone().unwrap_or_else(|| arms[0].id.clone());
    if !ids.contains(&baseline) {
        return Err(format!("baseline {baseline:?} is not an arm"));
    }
    let suite = root.join(&spec.suite);
    let mut tasks: Vec<PathBuf> = std::fs::read_dir(&suite)
        .map_err(|e| format!("{}: {e}", suite.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("task.json").is_file())
        .collect();
    tasks.sort();
    if !spec.tasks.is_empty() {
        for id in &spec.tasks {
            if !tasks.iter().any(|t| t.file_name().is_some_and(|n| n == id.as_str())) {
                return Err(format!("task {id:?} is not in {}", suite.display()));
            }
        }
        tasks.retain(|t| t.file_name().is_some_and(|n| spec.tasks.iter().any(|id| n == id.as_str())));
    }
    if tasks.is_empty() {
        return Err(format!("{} holds no task.json", suite.display()));
    }
    let out = root.join(spec.out.clone().unwrap_or_else(|| PathBuf::from("target/experiments").join(&spec.id)));
    let mut record = serde_json::to_value(spec).expect("serializes");
    record["arms"] = serde_json::to_value(&arms).expect("serializes");
    record["baseline"] = Value::String(baseline.clone());
    record["resolvedModel"] = serde_json::to_value(&model).expect("serializes");
    let options = CodingEvalOptions {
        experiment: spec.id.clone(),
        root: root.to_path_buf(),
        tasks,
        model,
        arms,
        baseline: Some(baseline),
        trials: spec.trials,
        max_attempts: spec.max_attempts,
        test_timeout: Duration::from_secs(spec.test_timeout_secs),
        host: HostConfig::default(),
        limits: spec.limits.clone(),
        balanced_order: spec.balanced_order,
        free_gateway: spec.free_gateway.then(agent_model::gateway::GatewayPolicy::default),
        spec: Some(record.clone()),
        out,
        quiet: false,
    };
    Ok(Resolved { options, record })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
    }

    fn spec(json: Value) -> Result<ExperimentSpec, String> {
        serde_json::from_value(json).map_err(|e| e.to_string())
    }

    fn base() -> Value {
        serde_json::json!({
            "schema": EXPERIMENT_SCHEMA,
            "id": "t",
            "suite": "fixtures/coding",
            "model": { "overrides": { "provider": "scripted" } },
            "arms": [{ "id": "baseline" }]
        })
    }

    #[test]
    fn defaults_are_the_baseline() {
        let r = resolve(&spec(base()).unwrap(), &root()).unwrap();
        let arm = &r.options.arms[0];
        assert_eq!(
            (arm.strategies.context, arm.strategies.recovery),
            (ContextStrategyId::Observed, RecoveryPolicyId::Feedback)
        );
        assert_eq!(r.options.baseline.as_deref(), Some("baseline"));
        assert_eq!((r.options.trials, r.options.max_attempts), (1, 3));
        assert!(r.options.tasks.len() >= 2);
        assert!(r.options.out.ends_with("target/experiments/t"));
    }

    #[test]
    fn unknown_fields_strategies_and_schemas_are_rejected() {
        let mut v = base();
        v["arms"][0]["context"] = "repo-map".into();
        assert!(spec(v).unwrap_err().contains("unknown variant"));
        let mut v = base();
        v["arms"][0]["temperature"] = 0.into();
        assert!(spec(v).unwrap_err().contains("unknown field"));
        let mut v = base();
        v["surprise"] = true.into();
        assert!(spec(v).is_err());
        let mut v = base();
        v["schema"] = "secret-agent.experiment/9".into();
        assert!(resolve(&spec(v).unwrap(), &root()).unwrap_err().contains("not supported"));
        let mut v = base();
        v["model"]["overrides"]["nonsense"] = 1.into();
        assert!(resolve(&spec(v).unwrap(), &root()).unwrap_err().contains("unknown field"));
        let mut v = base();
        v["tasks"] = serde_json::json!(["no-such-task"]);
        assert!(resolve(&spec(v).unwrap(), &root()).unwrap_err().contains("not in"));
        let mut v = base();
        v["arms"] = serde_json::json!([{ "id": "a" }, { "id": "a" }]);
        assert!(resolve(&spec(v).unwrap(), &root()).unwrap_err().contains("used twice"));
        let mut v = base();
        v["baseline"] = "missing".into();
        assert!(resolve(&spec(v).unwrap(), &root()).is_err());
    }

    #[test]
    fn anthropic_is_refused_and_the_model_comes_from_existing_config_files() {
        let mut v = base();
        v["model"] = serde_json::json!({ "config": "examples/models/anthropic.json" });
        assert!(resolve(&spec(v).unwrap(), &root()).unwrap_err().contains("do not use the anthropic provider"));
        let mut v = base();
        v["model"] =
            serde_json::json!({ "config": "examples/models/vercel-gateway.json", "overrides": { "maxTokens": 1000 } });
        let r = resolve(&spec(v).unwrap(), &root()).unwrap();
        assert_eq!(r.options.model.model.as_deref(), Some(agent_model::gateway::DEFAULT_FREE_MODEL));
        assert_eq!(r.options.model.max_tokens, 1000);
        assert_eq!(
            r.record["resolvedModel"]["apiKeyEnv"], "AI_GATEWAY_API_KEY",
            "the variable's name, never its value"
        );
    }

    #[test]
    fn an_ablation_varies_one_component_of_the_base_arm() {
        let mut v = base();
        v["arms"] = serde_json::json!([{ "id": "baseline", "recovery": "stop-on-repeat" }]);
        v["ablation"] = serde_json::json!({ "from": "baseline", "vary": "context", "values": ["minimal", "observed", "dependency-aware"] });
        let r = resolve(&spec(v).unwrap(), &root()).unwrap();
        let ids: Vec<&str> = r.options.arms.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["baseline", "context=minimal", "context=dependency-aware"],
            "the value equal to the base is the base"
        );
        for arm in &r.options.arms[1..] {
            assert_eq!(arm.differs_from(&r.options.arms[0]), ["context"]);
            assert_eq!(arm.strategies.recovery, RecoveryPolicyId::StopOnRepeat);
        }
        let mut v = base();
        v["ablation"] = serde_json::json!({ "from": "baseline", "vary": "model", "values": ["x"] });
        assert!(resolve(&spec(v).unwrap(), &root()).is_err());
    }
}
