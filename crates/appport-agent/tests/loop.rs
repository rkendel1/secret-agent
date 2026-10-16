//! The whole loop against the sample application, run by the real AppPort runtime.
//!
//! Needs Node.js and `npm ci` in `examples/customers-app`, so these tests are
//! ignored by a plain `cargo test`; CI runs them with `--ignored`
//! (see `.github/workflows/ci.yml`, job `agent-e2e`).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use agent_model::{ModelConfig, Provider};
use appport_agent::host::HostConfig;
use appport_agent::report::RunReport;
use appport_agent::{run, RunOptions};

const INTENT: &str =
    "replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export";
const LOOKUP: &str = "src/handlers/customers.lookup.mjs";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A private copy of the sample project, sharing its installed packages.
fn workspace(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let source = repo().join("examples/customers-app");
    assert!(source.join("node_modules/@appport/server").exists(), "run `npm ci` in {}", source.display());
    let dir = std::env::temp_dir().join(format!(
        "appport-agent-loop-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for part in ["src", "data"] {
        copy_dir(&source.join(part), &dir.join(part));
    }
    fs::copy(source.join("package.json"), dir.join("package.json")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("node_modules"), dir.join("node_modules")).unwrap();
    dir
}

fn responses(dir: &Path, completions: &[&str]) -> PathBuf {
    let path = dir.with_extension("responses");
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    for (i, text) in completions.iter().enumerate() {
        fs::write(path.join(format!("{:02}.txt", i + 1)), text).unwrap();
    }
    path
}

fn agent(project: &Path, responses: PathBuf, max_attempts: u32) -> RunReport {
    let mut model = ModelConfig::new(Provider::Scripted);
    model.responses = Some(responses);
    run(&RunOptions {
        project: project.to_path_buf(),
        intent: INTENT.into(),
        model,
        max_attempts,
        host: HostConfig::default(),
        report: None,
        log: Some(project.with_extension("log.jsonl")),
        quiet: true,
        context: Default::default(),
        goal_check: None,
        require_goal_evidence: true,
        allow_dirty: false,
    })
    .unwrap()
}

fn lookup_file(body: &str) -> String {
    format!("=== FILE: {LOOKUP} ===\n{body}\n=== END FILE ===\n")
}

fn failed(report: &RunReport, attempt: usize) -> Vec<String> {
    report.attempts[attempt]
        .verdict
        .as_ref()
        .map(|v| v.failed().into_iter().map(str::to_string).collect())
        .unwrap_or_default()
}

const STORE_LOOKUP: &str = r#"import { errors } from "@appport/protocol";
import { getCustomer } from "../store.mjs";

export async function lookupCustomer(input) {
  const customer = getCustomer(input.customerId);
  if (!customer) throw errors.notFound(`no customer ${input.customerId}`);
  const { customerId, name, email, tier, since } = customer;
  return { customerId, name, email, tier, since };
}"#;

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn demo_completions_reject_then_accept() {
    let project = workspace("demo");
    let report = agent(&project, repo().join("examples/scripted/replace-customer-lookup"), 3);

    assert_eq!(report.outcome, "accepted");
    assert_eq!((report.totals.model_calls, report.totals.rejected_attempts), (2, 1));
    assert_eq!(failed(&report, 0), ["failures.declared", "behavior.regression"]);
    assert!(failed(&report, 1).is_empty());
    assert!(
        report
            .baseline_failures
            .iter()
            .any(|f| f.contains("customers.register@1#customer-readable unavailable (NOT_FOUND)")),
        "the baseline shows why the change is needed: {:?}",
        report.baseline_failures
    );
    assert_eq!(report.deterministic.model_calls_for_discovery + report.deterministic.model_calls_for_verification, 0);
    assert_eq!(report.deterministic.permissions_granted, ["customers.read", "customers.write"]);
    assert!(report.context.source_bytes_sent < report.context.project_source_bytes);
    let accepted = fs::read_to_string(project.join(LOOKUP)).unwrap();
    assert!(accepted.contains("getCustomer") && accepted.contains("errors.notFound"));
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn an_edit_outside_the_boundary_is_never_written() {
    let project = workspace("boundary");
    let contract = fs::read_to_string(project.join("src/contract.mjs")).unwrap();
    let widened = contract.replace(
        r#"failures: [{ code: "NOT_FOUND""#,
        r#"failures: [{ code: "INTERNAL_ERROR", retry: "safe", effect: "none" }, { code: "NOT_FOUND""#,
    );
    let completion =
        format!("{}=== FILE: src/contract.mjs ===\n{widened}\n=== END FILE ===\n", lookup_file(STORE_LOOKUP));
    let report = agent(&project, responses(&project, &[&completion]), 1);

    assert_eq!(report.outcome, "rejected");
    assert!(failed(&report, 0).contains(&"edit.boundary".to_string()));
    assert_eq!(fs::read_to_string(project.join("src/contract.mjs")).unwrap(), contract, "the contract is untouched");
    assert!(fs::read_to_string(project.join(LOOKUP)).unwrap().contains("readExport"), "nothing was applied");
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn a_contradicted_verification_is_rejected_and_rolled_back() {
    let project = workspace("contradicted");
    let original = fs::read_to_string(project.join(LOOKUP)).unwrap();
    // Valid against the output schema, declared failure, but every customer is "standard".
    let flattening = STORE_LOOKUP.replace(
        "return { customerId, name, email, tier, since };",
        r#"return { customerId, name, email, tier: "standard", since };"#,
    );
    let report = agent(&project, responses(&project, &[&lookup_file(&flattening)]), 1);

    assert_eq!(report.outcome, "rejected");
    let verdict = report.attempts[0].verdict.as_ref().unwrap();
    assert_eq!(failed(&report, 0), ["behavior.regression", "verification"]);
    let verification = verdict.checks.iter().find(|c| c.code == "verification").unwrap();
    assert!(
        verification.violations[0]
            .contains("customer-readable contradicted: at /tier expected \"gold\" observed \"standard\""),
        "{verification:?}"
    );
    assert_eq!(fs::read_to_string(project.join(LOOKUP)).unwrap(), original, "rolled back");
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn an_implementation_that_does_not_load_fails_every_runtime_check() {
    let project = workspace("broken");
    let report =
        agent(&project, responses(&project, &[&lookup_file("export async function lookupCustomer(input) {")]), 1);

    assert_eq!(report.outcome, "rejected");
    let verdict = report.attempts[0].verdict.as_ref().unwrap();
    let load = verdict.checks.iter().find(|c| c.code == "runtime.load").unwrap();
    assert!(load.violations[0].contains("the application did not run"), "{load:?}");
    assert_eq!(failed(&report, 0).len(), verdict.checks.len() - 1, "everything but the boundary fails");
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn an_unusable_response_is_fed_back_and_retried() {
    let project = workspace("unusable");
    let report = agent(&project, responses(&project, &["I would read from the store.", &lookup_file(STORE_LOOKUP)]), 2);

    assert_eq!(report.outcome, "accepted");
    assert!(report.attempts[0].unusable.as_ref().unwrap().contains("no `=== FILE:"));
    assert_eq!(report.totals.model_calls, 2);
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn the_published_manifest_matches_the_offline_fixture() {
    // agent-core's offline tests plan against a copy of this manifest; keep it honest.
    let project = workspace("manifest");
    let entry = agent_core::observe::observe(&project).unwrap().application.unwrap();
    let (manifest, _) = appport_agent::collect::discover(&HostConfig::default(), &project, &entry).unwrap();
    let fixture: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(repo().join("crates/agent-core/tests/fixtures/customers.manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest, fixture, "regenerate crates/agent-core/tests/fixtures/customers.manifest.json");
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn eval_runs_every_arm_on_fresh_copies_and_aggregates() {
    use agent_core::plan::ContextMode;
    use appport_agent::eval::{eval, EvalOptions};

    let template = workspace("eval-template");
    let original = fs::read_to_string(template.join(LOOKUP)).unwrap();
    let out = template.with_extension("eval");
    let mut model = ModelConfig::new(Provider::Scripted);
    model.responses = Some(repo().join("examples/scripted/replace-customer-lookup"));
    let report = eval(&EvalOptions {
        project: template.clone(),
        intent: INTENT.into(),
        model,
        arms: vec![ContextMode::Knowledge, ContextMode::Bare],
        trials: 2,
        max_attempts: 3,
        host: HostConfig::default(),
        out: out.clone(),
        quiet: true,
    })
    .unwrap();

    assert_eq!(fs::read_to_string(template.join(LOOKUP)).unwrap(), original, "the template is never changed");
    for arm in [ContextMode::Knowledge, ContextMode::Bare] {
        let a = report.arm(arm).unwrap();
        assert_eq!((a.trials, a.accepted, a.accepted_first_attempt, a.rollbacks), (2, 2, 0, 2), "{a:?}");
        assert_eq!(a.rejected_by.get("failures.declared"), Some(&2));
    }
    let knowledge = report.arm(ContextMode::Knowledge).unwrap().input_tokens;
    let bare = report.arm(ContextMode::Bare).unwrap().input_tokens;
    assert!(bare < knowledge, "the bare arm is told less: {bare} vs {knowledge}");
    assert!(out.join("eval.json").exists() && out.join("eval.md").exists());
    assert!(out.join("bare-02/report.json").exists());
}
