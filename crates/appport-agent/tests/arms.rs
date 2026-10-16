//! The three-arm harness end to end with scripted completions (no paid
//! inference). Needs Node.js and `npm ci` in examples/customers-app, so it is
//! ignored by a plain `cargo test`; CI's agent-e2e job runs it with `--ignored`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_model::{ModelConfig, Provider};
use appport_agent::arms::{arm_eval, oracle, read_arm_task, Arm, ArmEvalOptions};
use appport_agent::host::HostConfig;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn every_arm_is_scored_by_the_independent_oracle() {
    let tasks_dir = root().join("fixtures/appport-tasks");
    let mut tasks: Vec<PathBuf> = std::fs::read_dir(&tasks_dir).unwrap().map(|e| e.unwrap().path()).collect();
    tasks.sort();
    let out = std::env::temp_dir().join(format!("appport-agent-arms-{}", std::process::id()));
    let report = arm_eval(&ArmEvalOptions {
        root: root(),
        tasks,
        model: ModelConfig::new(Provider::Scripted),
        arms: vec![Arm::Bare, Arm::Context, Arm::Native],
        trials: 1,
        max_attempts: 3,
        max_turns: 8,
        host: HostConfig { node: "node".into(), timeout: Duration::from_secs(20), ..HostConfig::default() },
        out: out.clone(),
        quiet: true,
        budget_usd: None,
        max_model_calls: None,
        max_minutes: None,
        free_gateway: None,
    })
    .unwrap();

    let category = |task: &str, arm: Arm| {
        report.trials.iter().find(|t| t.task == task && t.arm == arm.to_string()).map(|t| t.category.clone()).unwrap()
    };
    for arm in [Arm::Bare, Arm::Context, Arm::Native] {
        assert_eq!(category("replace-lookup", arm), "verified-success");
        assert_eq!(category("missing-capability", arm), "correct-limitation");
        assert_eq!(category("unverifiable-contract", arm), "correct-limitation");
    }
    // A and B need a named target; discovery is beyond them by design.
    assert_eq!(category("discover-readback", Arm::Bare), "wrong-limitation");
    assert_eq!(category("discover-readback", Arm::Context), "wrong-limitation");
    assert_eq!(category("discover-readback", Arm::Native), "verified-success");
    // Billing: B and C read the store and achieve the goal. A's rewrite still goes
    // through the legacy lookup: it preserves behaviour, so the in-loop judge
    // accepts it, but the oracle's goal check does not.
    assert_eq!(category("billing-migration", Arm::Context), "verified-success");
    assert_eq!(category("billing-migration", Arm::Native), "verified-success");
    assert_eq!(category("billing-migration", Arm::Bare), "preserved-only");
    let a_billing = report.trials.iter().find(|t| t.task == "billing-migration" && t.arm == "A-bare").unwrap();
    let o = a_billing.oracle.as_ref().unwrap();
    assert_eq!((o.preserved, o.goal_achieved, o.verified), (Some(true), Some(false), Some(false)));
    assert_eq!(report.arm(Arm::Bare).unwrap().false_successes, 1, "preserved-only counts as a false success");
    // A contract-changing request: B's proposals are rejected (fail-closed); C reports it.
    assert_eq!(category("contract-change", Arm::Context), "fail-closed");
    assert_eq!(category("contract-change", Arm::Native), "correct-limitation");

    // Nothing unverified is credited, and every trial has an auditable record.
    assert!(report.trials.iter().all(|t| t.category != "false-success" && t.category != "harness-error"));
    assert!(report.trials.iter().all(|t| t.provider == "scripted"));
    assert_eq!(report.status, "complete");
    assert_eq!((report.counts.planned, report.counts.attempted, report.counts.not_run), (18, 18, 0));
    assert_eq!(report.provenance.fixtures.len(), 6);
    assert!(!out.join("arm-eval.partial.json").exists(), "the partial summary is replaced by the final one");
    for t in &report.trials {
        assert!(out.join(&t.task).join(format!("{}-01/trial.json", t.arm)).exists());
    }
    // Arm order rotates across tasks.
    let first: std::collections::BTreeSet<&str> =
        report.trials.iter().filter(|t| t.order == 1).map(|t| t.arm.as_str()).collect();
    assert_eq!(first.len(), 3, "each arm runs first on some task");

    // The native arm's guards: a completion before verification is refused, and a
    // consequential capability cannot be invoked directly.
    let native = |task: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(out.join(task).join("C-native-01/report.json")).unwrap()).unwrap()
    };
    assert_eq!(native("replace-lookup")["refusedCompletions"], 1);
    let log = std::fs::read_to_string(out.join("discover-readback/C-native-01/run.jsonl")).unwrap();
    assert!(log.contains("is not a declared observation"));
    let _ = std::fs::remove_dir_all(out);
}

/// The goal check is what makes billing a measurement: an unchanged legacy
/// lookup preserves every answer, so without it the oracle would pass it.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn an_unchanged_project_never_achieves_a_goal() {
    let host = HostConfig { node: "node".into(), timeout: Duration::from_secs(20), ..HostConfig::default() };
    for id in ["billing-migration", "replace-lookup", "discover-readback"] {
        let task = read_arm_task(&root().join("fixtures/appport-tasks").join(id)).unwrap();
        assert!(task.goal_check.is_some(), "{id} has a goal check");
        let template = root().join(&task.project);
        let scratch = std::env::temp_dir().join(format!("appport-agent-goal-{id}-{}", std::process::id()));
        let project = scratch.join("project");
        appport_agent::eval::copy_project(&template, &project).unwrap();
        let result = oracle(&task, &template, &project, &scratch.join("oracle"), &host);
        assert_eq!(result.error, None, "{id}");
        assert!(result.changed_files.is_empty());
        if id == "billing-migration" {
            // The legacy implementation satisfies every behaviour check (it fails only
            // "no change was made"): without the goal check, nothing would separate it
            // from a store-backed one. That is the flaw the retired billing-from-store had.
            assert_eq!(result.failed_checks, ["edit.boundary", "goal.not-achieved"]);
        }
        assert_eq!(result.goal_achieved, Some(false), "{id}: but the goal is not achieved");
        assert_eq!(result.verified, Some(false), "{id}");
        assert!(result.failed_checks.contains(&"goal.not-achieved".to_string()));
        let _ = std::fs::remove_dir_all(scratch);
    }
}
