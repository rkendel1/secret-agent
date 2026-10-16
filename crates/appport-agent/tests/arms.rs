//! The three-arm harness end to end with scripted completions (no paid
//! inference). Needs Node.js and `npm ci` in examples/customers-app, so it is
//! ignored by a plain `cargo test`; CI's agent-e2e job runs it with `--ignored`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_model::{ModelConfig, Provider};
use appport_agent::arms::{arm_eval, Arm, ArmEvalOptions};
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
        host: HostConfig { node: "node".into(), timeout: Duration::from_secs(20) },
        out: out.clone(),
        quiet: true,
        budget_usd: None,
    })
    .unwrap();

    let category = |task: &str, arm: Arm| {
        report.trials.iter().find(|t| t.task == task && t.arm == arm.to_string()).map(|t| t.category.clone()).unwrap()
    };
    for arm in [Arm::Bare, Arm::Context, Arm::Native] {
        assert_eq!(category("replace-lookup", arm), "verified-success");
        assert_eq!(category("billing-from-store", arm), "verified-success");
        assert_eq!(category("missing-capability", arm), "correct-limitation");
        assert_eq!(category("unverifiable-contract", arm), "correct-limitation");
    }
    // A and B need a named target; discovery is beyond them by design.
    assert_eq!(category("discover-readback", Arm::Bare), "wrong-limitation");
    assert_eq!(category("discover-readback", Arm::Context), "wrong-limitation");
    assert_eq!(category("discover-readback", Arm::Native), "verified-success");
    // A contract-changing request: B's proposals are rejected (fail-closed); C reports it.
    assert_eq!(category("contract-change", Arm::Context), "fail-closed");
    assert_eq!(category("contract-change", Arm::Native), "correct-limitation");

    // Nothing unverified is credited, and every trial has an auditable record.
    assert!(report.trials.iter().all(|t| t.category != "false-success" && t.category != "harness-error"));
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
