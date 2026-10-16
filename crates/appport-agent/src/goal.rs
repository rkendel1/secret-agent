//! Goal checks: evidence that a change achieved what was asked, not only that
//! it preserved behaviour.
//!
//! A goal check is a short scenario run against the application through its own
//! published capabilities (see [`GoalCheck`]). It is used two ways:
//!
//! - by `appport-agent run --goal-check <file>`: a change is accepted only if the
//!   check fails on the unchanged project and passes on the changed one;
//! - by the three-arm evaluation's oracle, which never shows it to an arm.

use std::path::Path;

use agent_core::appport::knowledge::Contract;
use serde::{Deserialize, Serialize};

use crate::host::HostConfig;

/// A check that the goal was achieved, not only that behaviour was preserved.
///
/// Each step invokes a capability on the running application (a fresh process,
/// granted exactly the permissions the steps' capabilities declare in the
/// fixture's own contract) and must answer `ok`. Inputs and expected values may
/// refer to an earlier step's input or output: `{"$step": 0, "at": "/customerId"}`
/// (output) or `{"$step": 0, "input": "/email"}`. The check must fail on the
/// unchanged fixture; if it passes there, it measures nothing and the oracle
/// reports an error instead of a verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalCheck {
    pub description: String,
    pub steps: Vec<GoalStep>,
}

/// One goal-check step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalStep {
    /// `name@version`.
    pub capability: String,
    pub input: serde_json::Value,
    #[serde(default)]
    pub expect: Vec<GoalExpect>,
}

/// An expected value in a step's output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalExpect {
    /// JSON pointer into the step's output.
    pub at: String,
    pub equals: serde_json::Value,
}

/// Resolves `{"$step": n, "at": ptr}` / `{"$step": n, "input": ptr}` references.
fn resolve_refs(
    value: &serde_json::Value,
    done: &[(serde_json::Value, serde_json::Value)],
) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    match value {
        Value::Object(map) if map.contains_key("$step") => {
            let n = map["$step"].as_u64().ok_or("$step must be a number")? as usize;
            let (input, output) = done.get(n).ok_or(format!("$step {n} has not run"))?;
            let (source, pointer) = match (map.get("at"), map.get("input")) {
                (Some(Value::String(p)), None) => (output, p),
                (None, Some(Value::String(p))) => (input, p),
                _ => return Err("a $step reference needs exactly one of \"at\" or \"input\"".into()),
            };
            source.pointer(pointer).cloned().ok_or(format!("step {n} has nothing at {pointer}"))
        }
        Value::Object(map) => Ok(Value::Object(
            map.iter().map(|(k, v)| Ok((k.clone(), resolve_refs(v, done)?))).collect::<Result<_, String>>()?,
        )),
        Value::Array(items) => Ok(Value::Array(items.iter().map(|v| resolve_refs(v, done)).collect::<Result<_, _>>()?)),
        other => Ok(other.clone()),
    }
}

/// Runs a goal check against the application at `root`, in a fresh process.
pub fn run_goal_check(
    check: &GoalCheck,
    contract: &Contract,
    host: &HostConfig,
    root: &Path,
    entry: &agent_core::observe::ApplicationEntry,
) -> Result<(), String> {
    use agent_core::appport::knowledge::CapabilityId;
    use agent_core::verify::Reply;
    let mut ids = Vec::new();
    let mut permissions = std::collections::BTreeSet::new();
    for step in &check.steps {
        let (name, version) = step.capability.rsplit_once('@').ok_or("goal step capability is not name@version")?;
        let version: u32 = version.parse().map_err(|_| "goal step version")?;
        let definition = contract
            .manifest()
            .capability_version(name, version)
            .ok_or(format!("the fixture's contract has no {}", step.capability))?;
        permissions.extend(definition.authorization.iter().cloned());
        ids.push(CapabilityId { name: name.into(), version });
    }
    let permissions: Vec<String> = permissions.into_iter().collect();
    let mut app = crate::host::Host::start(host, root, entry, &permissions)?;
    let mut done: Vec<(serde_json::Value, serde_json::Value)> = Vec::new();
    let outcome = (|| -> Result<(), String> {
        for (index, (step, id)) in check.steps.iter().zip(&ids).enumerate() {
            let input = resolve_refs(&step.input, &done)?;
            let output = match app.request(id, &input)?.0 {
                Reply::Ok { output } => output,
                Reply::Err { code, message } => {
                    return Err(format!("step {index} ({}) answered {code}: {message}", step.capability))
                }
            };
            for expect in &step.expect {
                let wanted = resolve_refs(&expect.equals, &done)?;
                let got = output.pointer(&expect.at);
                if got != Some(&wanted) {
                    return Err(format!(
                        "step {index} ({}): {} is {}, expected {wanted}",
                        step.capability,
                        expect.at,
                        got.map_or("missing".to_string(), |v| v.to_string())
                    ));
                }
            }
            done.push((input, output));
        }
        Ok(())
    })();
    app.close();
    outcome
}

/// Reads a goal check from a JSON file: either the check itself, or a task file
/// holding one under `goalCheck`.
pub fn read_goal_check(path: &Path) -> Result<GoalCheck, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let value = value.get("goalCheck").cloned().unwrap_or(value);
    let check: GoalCheck =
        serde_json::from_value(value).map_err(|e| format!("{}: not a goal check: {e}", path.display()))?;
    if check.steps.is_empty() {
        return Err(format!("{}: a goal check needs at least one step", path.display()));
    }
    Ok(check)
}
