//! The strategy boundary and the experiment harness: scripted completions or a
//! local stand-in provider only, so no network and no paid inference.
//!
//! Tests that run the fixtures' test commands need Node.js 22+ and are ignored by
//! a plain `cargo test`; CI's agent-e2e job runs them with `--include-ignored`.

#[path = "../../agent-model/tests/support/fake_gateway.rs"]
mod fake_gateway;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use agent_core::generic::{coding_task, plan_coding, Baseline, CodingContext};
use agent_core::observe::observe;
use agent_model::{ModelConfig, Provider};
use appport_agent::coding::{
    classify, coding_eval, read_task, CodingArm, CodingEvalOptions, CodingEvalReport, Limits, Strategies,
};
use appport_agent::host::HostConfig;
use appport_agent::strategy::{
    context_strategy, ContextInput, ContextStrategyId, RecoveryPolicyId, DEFAULT_CONTEXT_BUDGET,
};
use fake_gateway::{FakeGateway, Seen};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn fixtures() -> PathBuf {
    root().join("fixtures/coding")
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "appport-agent-experiments-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn arm(id: &str, context: ContextStrategyId, recovery: RecoveryPolicyId) -> CodingArm {
    CodingArm { id: id.into(), strategies: Strategies { context, recovery, context_budget: DEFAULT_CONTEXT_BUDGET } }
}

fn options(tasks: &[&str], arms: Vec<CodingArm>, name: &str) -> CodingEvalOptions {
    let mut o = CodingEvalOptions::new(
        tasks.iter().map(|t| fixtures().join(t)).collect(),
        ModelConfig::new(Provider::Scripted),
        arms,
        scratch(name),
    );
    o.root = root();
    o.trials = 1;
    o.test_timeout = Duration::from_secs(60);
    o.quiet = true;
    o
}

/// A copy of a fixture task with its own scripted completions.
fn task_with(base: &str, completions: &[&str], name: &str) -> PathBuf {
    let dir = scratch(name).join(base);
    let source = fixtures().join(base);
    for part in ["repo", "hidden"] {
        copy(&source.join(part), &dir.join(part));
    }
    fs::copy(source.join("task.json"), dir.join("task.json")).unwrap();
    fs::create_dir_all(dir.join("scripted")).unwrap();
    for (i, text) in completions.iter().enumerate() {
        fs::write(dir.join(format!("scripted/{:02}.txt", i + 1)), text).unwrap();
    }
    dir
}

fn copy(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(entry.path().canonicalize().unwrap(), to.join(entry.file_name())).unwrap();
        } else if kind.is_dir() {
            copy(&entry.path(), &to.join(entry.file_name()));
        } else {
            fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

fn trial<'a>(report: &'a CodingEvalReport, task: &str, arm: &str) -> &'a appport_agent::coding::CodingTrial {
    report.trials.iter().find(|t| t.task == task && t.arm == arm).unwrap()
}

/// The baseline strategies render exactly the prompt the loop sent before strategies existed.
#[test]
fn the_baseline_strategies_reproduce_the_existing_prompts_byte_for_byte() {
    let dir = scratch("prompt").join("repo");
    copy(&fixtures().join("bugfix-invoice-discount/repo"), &dir);
    let task = read_task(&fixtures().join("bugfix-invoice-discount")).unwrap();
    let project = observe(&dir).unwrap();
    let baseline =
        Baseline { passed: false, output: "not ok 1 - test/invoice.test.js\n  expected 90, got 99.1\n".into() };
    let plan = plan_coding(&project, &task.intent, &task.test, &baseline).unwrap();
    let files: BTreeMap<String, String> =
        plan.context.iter().map(|f| (f.path.clone(), project.read(&f.path).unwrap())).collect();
    let feedback = ["`npm test` failed (exit Some(1)).".to_string()];
    for (old, new) in
        [(CodingContext::Observed, ContextStrategyId::Observed), (CodingContext::Minimal, ContextStrategyId::Minimal)]
    {
        let selection = context_strategy(new)
            .select(&ContextInput {
                project: &project,
                plan: &plan,
                baseline: &baseline,
                budget: DEFAULT_CONTEXT_BUDGET,
                host: &HostConfig::default(),
                requested: &[],
            })
            .unwrap();
        for fb in [&[][..], &feedback[..]] {
            let before = coding_task(old, &plan, &files, &baseline, fb);
            let after = selection.task(&plan, &baseline, fb);
            assert_eq!((before.system, before.prompt), (after.system, after.prompt), "{new}");
        }
        assert!(selection.unknowns.is_empty());
        assert!(selection.items.iter().all(|i| !i.reason.is_empty()), "every selection has a reason");
    }
}

#[test]
fn trials_are_classified_from_execution_evidence_only() {
    let base = || {
        let mut t = appport_agent::coding::CodingTrial {
            task: "t".into(),
            arm: "a".into(),
            strategies: Strategies::default(),
            trial: 1,
            order: 1,
            outcome: "accepted".into(),
            category: String::new(),
            reason: None,
            error: None,
            patch_applied: true,
            tests_passed: true,
            hidden_passed: Some(true),
            success: false,
            attempts: 1,
            model_calls: 1,
            models_reported: vec![],
            input_tokens: 0,
            output_tokens: 0,
            tokens_estimated: false,
            provider_retries: 0,
            recovery_decisions: vec![],
            stopped_by: None,
            context_bytes: 0,
            test_runs: 2,
            rollbacks: 0,
            unusable_responses: 0,
            boundary_violations: 0,
            test_edit_attempts: 0,
            stayed_in_intended_files: None,
            cost_usd: None,
            started_at: String::new(),
            wall_ms: 0,
        };
        t.success = false;
        t
    };
    let c = |t: &appport_agent::coding::CodingTrial, hidden: bool| classify(t, hidden).0;
    assert_eq!(c(&base(), true), "verified-success");
    let mut t = base();
    t.hidden_passed = Some(false);
    assert_eq!(c(&t, true), "accepted-unverified", "visible tests are not enough");
    t.hidden_passed = None;
    assert_eq!(c(&t, true), "infrastructure-error", "required evidence missing is never a success");
    assert_eq!(c(&t, false), "verified-success", "a task without hidden tests is judged by its own tests");
    let mut t = base();
    t.tests_passed = false;
    t.hidden_passed = None;
    assert_eq!(c(&t, false), "infrastructure-error", "an accepted claim without a passing test run is not a success");
    for (outcome, expected) in [
        ("provider-error", "provider-error"),
        ("infrastructure-error", "infrastructure-error"),
        ("rollback-failed", "infrastructure-error"),
        ("error", "infrastructure-error"),
        ("refused", "limitation"),
        ("not-run", "not-run"),
    ] {
        let mut t = base();
        t.outcome = outcome.into();
        assert_eq!(c(&t, true), expected, "{outcome}");
    }
    let mut t = base();
    t.outcome = "rejected".into();
    t.patch_applied = false;
    assert_eq!(c(&t, true), "patch-failed");
    t.patch_applied = true;
    assert_eq!(c(&t, true), "rejected");
    t.stopped_by = Some("same failure".into());
    let (category, reason) = classify(&t, true);
    assert_eq!(category, "rejected");
    assert!(reason.unwrap().contains("stopped by recovery"));
}

/// A configuration at the gateway runs only after the free-model preflight, and
/// a failed preflight runs nothing.
#[test]
fn a_gateway_experiment_fails_closed_without_a_verified_free_model() {
    let dir = scratch("gateway");
    let spec = dir.join("spec.json");
    fs::write(
        &spec,
        serde_json::json!({
            "schema": "secret-agent.experiment/1",
            "id": "gateway-closed",
            "suite": "fixtures/coding",
            "model": { "config": "examples/models/vercel-gateway.json", "overrides": { "apiKeyEnv": "EXPERIMENTS_TEST_NO_SUCH_KEY" } },
            "arms": [{ "id": "baseline" }],
            "out": dir.join("out")
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_appport-agent"))
        .args(["experiment", "--spec", spec.to_str().unwrap(), "--root", root().to_str().unwrap(), "--quiet"])
        .env_remove("EXPERIMENTS_TEST_NO_SUCH_KEY")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", String::from_utf8_lossy(&output.stderr));
    let report: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("out/coding-eval.json")).unwrap()).unwrap();
    assert_eq!(report["status"], "preflight-failed");
    assert_eq!(report["trials"].as_array().unwrap().len(), 0, "no trial ran");
    assert_eq!(report["preflight"]["category"], "config");
    assert!(dir.join("out/preflight.json").exists());
    assert!(dir.join("out/experiment.json").exists(), "the resolved specification is recorded");
}

#[test]
fn validate_resolves_without_running_anything() {
    let output = Command::new(env!("CARGO_BIN_EXE_appport-agent"))
        .args(["experiment", "--spec", "examples/experiments/context-ablation.json", "--validate"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let resolved: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let ids: Vec<&str> = resolved["arms"].as_array().unwrap().iter().map(|a| a["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["baseline", "context=minimal", "context=dependency-aware", "context=appport-aware"]);
    let bad = Command::new(env!("CARGO_BIN_EXE_appport-agent"))
        .args(["experiment", "--spec", "Cargo.toml", "--validate"])
        .current_dir(root())
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2), "an unreadable specification is a usage error");
}

// Everything below runs the fixtures' tests.

/// The default `code-eval` path, without any experiment configuration, still
/// runs the baseline and judges it exactly as before.
#[test]
#[ignore = "needs Node.js 22+"]
fn the_default_path_runs_the_baseline_unchanged() {
    let o = CodingEvalOptions::new(
        vec![fixtures().join("bugfix-invoice-discount")],
        ModelConfig::new(Provider::Scripted),
        vec![CodingArm::of(CodingContext::Observed)],
        scratch("default"),
    );
    assert_eq!(o.arms[0].strategies, Strategies::default());
    let mut o = o;
    o.trials = 1;
    o.quiet = true;
    let report = coding_eval(&o).unwrap();
    let t = trial(&report, "bugfix-invoice-discount", "observed");
    assert_eq!((t.category.as_str(), t.attempts, t.test_edit_attempts), ("verified-success", 2, 1));
    assert_eq!(t.recovery_decisions, ["retry-with-feedback"], "the baseline recovery is a feedback retry");
}

/// Same specification, same scripted completions: the same results, the same
/// prompts, on fresh workspaces, with the fixtures left untouched.
#[test]
#[ignore = "needs Node.js 22+"]
fn scripted_experiments_replay_deterministically_on_isolated_workspaces() {
    let arms = || {
        vec![
            arm("observed", ContextStrategyId::Observed, RecoveryPolicyId::Feedback),
            arm("dependency-aware", ContextStrategyId::DependencyAware, RecoveryPolicyId::Feedback),
        ]
    };
    let tasks = ["bugfix-invoice-discount", "feature-slug-max-length"];
    let (a, b) = (options(&tasks, arms(), "replay-a"), options(&tasks, arms(), "replay-b"));
    let (out_a, out_b) = (a.out.clone(), b.out.clone());
    let first = coding_eval(&a).unwrap();
    let second = coding_eval(&b).unwrap();
    assert!(first.fixtures_unchanged && second.fixtures_unchanged);
    assert_eq!(first.provenance.fixtures, second.provenance.fixtures);
    let summary = |r: &CodingEvalReport| -> Vec<(String, String, String, usize, Vec<String>)> {
        let mut rows: Vec<_> = r
            .trials
            .iter()
            .map(|t| (t.task.clone(), t.arm.clone(), t.category.clone(), t.attempts, t.recovery_decisions.clone()))
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(summary(&first), summary(&second));
    let prompts = |out: &Path, task: &str, arm: &str| -> Vec<String> {
        fs::read_to_string(out.join(format!("{task}/{arm}-01/run.jsonl")))
            .unwrap()
            .lines()
            .filter(|l| l.contains("\"task\":{"))
            .map(|l| {
                // The test runner prints wall-clock durations; everything else must match.
                let task = serde_json::from_str::<serde_json::Value>(l).unwrap()["task"]["prompt"]
                    .as_str()
                    .unwrap()
                    .to_string();
                // Each run has its own directory, which appears in stack traces.
                let task = task.replace(&out.display().to_string(), "<out>");
                // ... and the absolute path of the trial's own workspace.
                let task = task.replace(out.to_string_lossy().as_ref(), "<out>");
                task.lines().filter(|l| !l.contains("duration_ms")).collect::<Vec<_>>().join("\n")
            })
            .collect()
    };
    for task in tasks {
        for a in ["observed", "dependency-aware"] {
            let (p1, p2) = (prompts(&out_a, task, a), prompts(&out_b, task, a));
            assert!(!p1.is_empty());
            assert_eq!(p1, p2, "{task} {a}: the same prompts");
        }
    }
    // Arms ran on separate copies, and the order rotated.
    let dirs: std::collections::BTreeSet<String> =
        first.trials.iter().map(|t| format!("{}/{}", t.task, t.arm)).collect();
    assert_eq!(dirs.len(), first.trials.len());
    assert!(first.trials.iter().any(|t| t.arm == "dependency-aware" && t.order == 1), "balanced order");
    // The richer context is reported, and the comparison names the one component that differs.
    let c = &first.comparisons[0];
    assert_eq!((c.arm.as_str(), c.differs_in.as_slice()), ("dependency-aware", &["context".to_string()][..]));
    assert!(c.mean_context_bytes_delta > 0.0);
}

/// Recovery decisions are recorded, stop early on a repeated failure, and never
/// widen the editable files.
#[test]
#[ignore = "needs Node.js 22+"]
fn recovery_policies_are_recorded_and_never_widen_the_boundary() {
    let wrong = "=== FILE: src/invoice.js ===\nexport function invoiceTotal() { return 0; }\n=== END FILE ===\n";
    let golden = format!(
        "=== FILE: src/invoice.js ===\n{}=== END FILE ===\n",
        fs::read_to_string(fixtures().join("bugfix-invoice-discount/golden/src/invoice.js")).unwrap()
    );
    // Stop on repeat: the same wrong change twice → stopped after two attempts.
    let task = task_with("bugfix-invoice-discount", &[wrong, wrong, &golden], "stop");
    let mut o =
        options(&[], vec![arm("stop", ContextStrategyId::Observed, RecoveryPolicyId::StopOnRepeat)], "stop-out");
    o.tasks = vec![task];
    let report = coding_eval(&o).unwrap();
    let t = trial(&report, "bugfix-invoice-discount", "stop");
    assert_eq!((t.category.as_str(), t.attempts), ("rejected", 2), "{t:?}");
    assert!(t.reason.as_deref().unwrap().contains("stopped by recovery"));
    assert_eq!(t.recovery_decisions, ["retry-with-feedback", "stop"]);

    // Feedback retry with the same script runs the third attempt and succeeds.
    let task = task_with("bugfix-invoice-discount", &[wrong, wrong, &golden], "feedback");
    let mut o =
        options(&[], vec![arm("feedback", ContextStrategyId::Observed, RecoveryPolicyId::Feedback)], "feedback-out");
    o.tasks = vec![task];
    let report = coding_eval(&o).unwrap();
    let t = trial(&report, "bugfix-invoice-discount", "feedback");
    assert_eq!((t.category.as_str(), t.attempts), ("verified-success", 3));

    // Expand context: read-only files may be added; the editable files never change.
    let task = task_with("bugfix-invoice-discount", &[wrong, &golden], "expand");
    let mut o =
        options(&[], vec![arm("expand", ContextStrategyId::Minimal, RecoveryPolicyId::ExpandContext)], "expand-out");
    o.tasks = vec![task];
    let out = o.out.clone();
    let report = coding_eval(&o).unwrap();
    let t = trial(&report, "bugfix-invoice-discount", "expand");
    assert_eq!(t.category, "verified-success");
    assert_eq!(t.recovery_decisions, ["retry-with-context"]);
    let run: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("bugfix-invoice-discount/expand-01/report.json")).unwrap())
            .unwrap();
    let files = |i: usize| -> Vec<String> {
        run["attempts"][i]["contextFiles"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect()
    };
    assert!(files(1).len() > files(0).len(), "the second attempt saw more: {:?} → {:?}", files(0), files(1));
    assert!(files(0).iter().all(|f| files(1).contains(f)));
    assert_eq!(run["editable"], serde_json::json!(["src/invoice.js"]), "the boundary is the planner's, unchanged");
    assert_eq!(t.boundary_violations, 0);
}

/// Passing the project's own tests is not enough when hidden acceptance fails.
#[test]
#[ignore = "needs Node.js 22+"]
fn a_change_that_only_satisfies_the_visible_tests_is_not_a_success() {
    let partial = "=== FILE: src/handlers/customers.lookup.mjs ===\nimport { getCustomer } from \"../store.mjs\";\nexport async function lookupCustomer(input) {\n  const c = getCustomer(input.customerId);\n  if (!c) throw new Error(\"missing\");\n  return { ...c };\n}\n=== END FILE ===\n";
    let task = task_with("appport-readback", &[partial], "partial");
    let mut o =
        options(&[], vec![arm("observed", ContextStrategyId::Observed, RecoveryPolicyId::Feedback)], "partial-out");
    o.tasks = vec![task];
    let report = coding_eval(&o).unwrap();
    let t = trial(&report, "appport-readback", "observed");
    assert_eq!((t.outcome.as_str(), t.tests_passed, t.hidden_passed), ("accepted", true, Some(false)));
    assert_eq!(t.category, "accepted-unverified");
    assert!(!t.success);
    assert_eq!(report.arms[0].false_accepts, 1);
}

/// Provider failures and broken tasks are counted apart from task outcomes, a
/// broken trial does not stop the rest, and secrets never reach the records.
#[test]
#[ignore = "needs Node.js 22+"]
fn provider_and_infrastructure_failures_are_not_task_outcomes() {
    let key = "vck_experiments_secret_0123456789";
    std::env::set_var("EXPERIMENTS_TEST_KEY", key);
    let body = format!(r#"{{"error":"key {key} is not allowed"}}"#);
    let gw = FakeGateway::start(Box::new(move |_: &Seen| (403, body.clone(), Duration::ZERO)));
    let broken = scratch("broken").join("broken-task");
    fs::create_dir_all(&broken).unwrap();
    fs::write(broken.join("task.json"), r#"{"id":"broken-task","intent":"x","test":["npm","test"]}"#).unwrap();
    let mut o = options(
        &["feature-slug-max-length"],
        vec![arm("observed", ContextStrategyId::Observed, RecoveryPolicyId::Feedback)],
        "failures",
    );
    o.tasks.push(broken);
    let mut model = ModelConfig::new(Provider::OpenAiCompatible);
    model.model = Some("m".into());
    model.base_url = Some(gw.base_url.clone());
    model.api_key_env = Some("EXPERIMENTS_TEST_KEY".into());
    o.model = model;
    let out = o.out.clone();
    let report = coding_eval(&o).unwrap();
    std::env::remove_var("EXPERIMENTS_TEST_KEY");
    assert_eq!(trial(&report, "feature-slug-max-length", "observed").category, "provider-error");
    assert_eq!(
        trial(&report, "broken-task", "observed").category,
        "infrastructure-error",
        "a missing repository is the harness's failure"
    );
    let cell = &report.arms[0];
    assert_eq!(
        (cell.scored, cell.errors, cell.success_rate),
        (0, 2, 0.0),
        "nothing scored, nothing counted against the strategy"
    );
    for entry in walk(&out) {
        let text = fs::read_to_string(&entry).unwrap_or_default();
        assert!(!text.contains(key), "{} holds the key", entry.display());
    }
    assert!(!out.join("coding-eval.partial.json").exists(), "the partial summary is replaced by the final one");
}

/// A model-call cap is never exceeded; skipped trials are listed, never dropped.
#[test]
#[ignore = "needs Node.js 22+"]
fn limits_skip_trials_and_say_so() {
    let mut o = options(
        &["bugfix-invoice-discount", "feature-slug-max-length", "appport-readback"],
        vec![arm("observed", ContextStrategyId::Observed, RecoveryPolicyId::Feedback)],
        "limits",
    );
    o.limits = Limits { max_model_calls: Some(4), ..Limits::default() };
    let report = coding_eval(&o).unwrap();
    let calls: usize = report.trials.iter().map(|t| t.model_calls).sum();
    assert!(calls <= 4);
    let skipped: Vec<_> = report.trials.iter().filter(|t| t.category == "not-run").collect();
    assert!(!skipped.is_empty() && skipped.iter().all(|t| t.reason.as_deref().unwrap().contains("cap of 4")));
    assert_eq!(report.trials.len(), 3);

    let mut o = options(
        &["feature-slug-max-length"],
        vec![arm("observed", ContextStrategyId::Observed, RecoveryPolicyId::Feedback)],
        "timeout",
    );
    o.test_timeout = Duration::from_millis(1);
    o.max_attempts = 1;
    let report = coding_eval(&o).unwrap();
    let t = &report.trials[0];
    assert!(
        matches!(t.category.as_str(), "timed-out" | "limitation"),
        "a test command that cannot finish is never a success: {t:?}"
    );
    assert!(!t.success);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() && !path.ends_with("node_modules") {
            out.extend(walk(&path));
        } else if path.is_file() {
            out.push(path);
        }
    }
    out
}
