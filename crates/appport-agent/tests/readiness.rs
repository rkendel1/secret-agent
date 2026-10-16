//! Production-readiness regression tests for `appport-agent run`, the path that
//! changes a user's project in place. Each test names the readiness finding it
//! guards (see docs/production-readiness.md). Scripted completions or a local
//! stand-in provider only: no network, no paid inference.
//!
//! Most need Node.js and `npm ci` in examples/customers-app, so they are ignored
//! by a plain `cargo test`; CI's agent-e2e job runs them with `--include-ignored`.

#[path = "../../agent-model/tests/support/fake_gateway.rs"]
mod fake_gateway;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_model::{ModelConfig, Provider};
use appport_agent::host::HostConfig;
use appport_agent::run::ProjectLock;
use appport_agent::{run, RunOptions};
use fake_gateway::{completion, FakeGateway, Seen};
use serde_json::{json, Value};

const LOOKUP_INTENT: &str =
    "replace customers.lookup@1: read customers from the application's customer store instead of the legacy CRM export";
const LOOKUP: &str = "src/handlers/customers.lookup.mjs";
const BILLING: &str = "src/handlers/billing.contact.mjs";

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "appport-agent-readiness-{}-{name}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_symlink() || entry.file_name() == "node_modules" {
            continue;
        } else if kind.is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A private copy of a project (relative to the repository), sharing the installed packages.
fn project(relative: &str, name: &str) -> PathBuf {
    let modules = repo().join("examples/customers-app/node_modules");
    assert!(modules.join("@appport/server").exists(), "run `npm ci` in examples/customers-app");
    let dir = scratch(name).join("project");
    copy_dir(&repo().join(relative), &dir);
    #[cfg(unix)]
    std::os::unix::fs::symlink(modules, dir.join("node_modules")).unwrap();
    dir
}

/// A directory of scripted completions.
fn responses(name: &str, completions: &[String]) -> PathBuf {
    let dir = scratch(name).join("responses");
    fs::create_dir_all(&dir).unwrap();
    for (i, text) in completions.iter().enumerate() {
        fs::write(dir.join(format!("{:02}.txt", i + 1)), text).unwrap();
    }
    dir
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn scripted(path: &str) -> String {
    read(&repo().join(path))
}

/// Runs the CLI; returns its exit code and the report.
fn agent(args: &[&str], env: &[(&str, &str)]) -> (i32, Value) {
    let report = scratch("report").join("report.json");
    let mut command = Command::new(env!("CARGO_BIN_EXE_appport-agent"));
    command.args(args).args(["--report", report.to_str().unwrap(), "--quiet"]);
    for (k, v) in env {
        command.env(k, v);
    }
    let output = command.output().unwrap();
    let code = output.status.code().unwrap_or(-1);
    let report = fs::read_to_string(&report)
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap_or_else(|_| json!({ "stderr": String::from_utf8_lossy(&output.stderr) }));
    (code, report)
}

fn billing_goal() -> String {
    repo().join("fixtures/appport-tasks/billing-migration/task.json").to_string_lossy().into()
}

fn billing_intent() -> String {
    let task: Value =
        serde_json::from_str(&read(&repo().join("fixtures/appport-tasks/billing-migration/task.json"))).unwrap();
    format!("replace billing.contact@1: {}", task["goal"].as_str().unwrap())
}

// 1. A verified change, with its goal evidence, and the rejected attempt undone and checked.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra03_a_verified_change_records_its_goal_evidence() {
    let dir = project("examples/customers-app", "verified");
    let original = read(&dir.join(LOOKUP));
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "scripted",
            "--responses",
            repo().join("examples/scripted/replace-customer-lookup").to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["outcome"], "accepted");
    assert_eq!(report["projectState"], "changed");
    assert_eq!(
        report["goal"]["method"], "declared-verification",
        "a declared verification failed before and passes after"
    );
    assert!(report["inFlight"].is_null(), "no change is left in flight");
    let first = &report["attempts"][0];
    assert_eq!(first["rolledBack"], true);
    assert_eq!(first["rollback"]["restored"], json!([LOOKUP]));
    assert_ne!(read(&dir.join(LOOKUP)), original, "the accepted change is kept");
}

// 2 + 3. Builds, preserves every answer, passes every preservation check, and does not
// achieve the goal: rejected, and undone. The retired billing-from-store failure.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra03_preserving_behaviour_without_achieving_the_goal_is_rejected() {
    let dir = project("fixtures/appport-projects/customers-billing", "goal");
    let original = read(&dir.join(BILLING));
    let legacy = responses("legacy", &[scripted("fixtures/appport-tasks/billing-migration/scripted/A/01.txt")]);
    let intent = billing_intent();
    let goal = billing_goal();
    let base = ["run", "--project", dir.to_str().unwrap(), "--intent", &intent, "--provider", "scripted"];

    // Without a goal check nothing can show the goal was met: declined before any model call.
    let (code, report) = agent(&[&base[..], &["--responses", legacy.to_str().unwrap()]].concat(), &[]);
    assert_eq!(code, 3, "{report}");
    assert_eq!(report["outcome"], "limitation");
    assert!(report["reason"].as_str().unwrap().contains("nothing observable"));
    assert_eq!(report["totals"]["modelCalls"], 0);

    // With the goal check: every preservation check passes, the goal check fails, nothing is kept.
    let (code, report) = agent(
        &[&base[..], &["--responses", legacy.to_str().unwrap(), "--goal-check", &goal, "--max-attempts", "1"]].concat(),
        &[],
    );
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["outcome"], "rejected");
    let attempt = &report["attempts"][0];
    assert_eq!(attempt["verdict"]["accepted"], true, "the deterministic judge alone would have accepted it");
    assert_eq!(attempt["goalCheck"]["passed"], false);
    assert_eq!(attempt["rolledBack"], true);
    assert_eq!(report["projectState"], "unchanged");
    assert_eq!(read(&dir.join(BILLING)), original, "the project is exactly as it was");

    // A store-backed change achieves it.
    let store = responses("store", &[scripted("fixtures/appport-tasks/billing-migration/scripted/B/01.txt")]);
    let (code, report) =
        agent(&[&base[..], &["--responses", store.to_str().unwrap(), "--goal-check", &goal]].concat(), &[]);
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["goal"]["method"], "goal-check");
    assert_eq!(report["attempts"][0]["goalCheck"]["passed"], true);
}

// The explicit opt-out is visible in the report.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra03_accepting_without_goal_evidence_must_be_asked_for_and_is_recorded() {
    let dir = project("fixtures/appport-projects/customers-billing", "preserving");
    let legacy = responses("legacy", &[scripted("fixtures/appport-tasks/billing-migration/scripted/A/01.txt")]);
    let intent = billing_intent();
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            &intent,
            "--provider",
            "scripted",
            "--responses",
            legacy.to_str().unwrap(),
            "--accept-preserving",
        ],
        &[],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(report["goal"]["required"], false);
    assert_eq!(report["goal"]["method"], "none");
}

// 4. A contract that declares no verification: a limitation, exit 3, no model call.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn an_unverifiable_contract_is_a_limitation() {
    let dir = project("fixtures/appport-projects/customers-undeclared", "undeclared");
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "scripted",
            "--responses",
            repo().join("examples/scripted/replace-customer-lookup").to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 3, "{report}");
    assert!(report["reason"].as_str().unwrap().contains("contract.unverifiable"), "{report}");
    assert_eq!(report["totals"]["modelCalls"], 0);
}

// 7. A source file that is a symbolic link out of the project is never read or written.
#[cfg(unix)]
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra01_a_symlink_out_of_the_project_is_never_followed() {
    let dir = project("examples/customers-app", "symlink");
    let outside = dir.parent().unwrap().join("outside");
    fs::create_dir_all(outside.join("handlers")).unwrap();
    std::os::unix::fs::symlink(repo().join("examples/customers-app/node_modules"), outside.join("node_modules"))
        .unwrap();
    std::os::unix::fs::symlink(dir.join("src/legacy"), outside.join("legacy")).unwrap();
    std::os::unix::fs::symlink(dir.join("src/store.mjs"), outside.join("store.mjs")).unwrap();
    let target = outside.join("handlers/customers.lookup.mjs");
    fs::rename(dir.join(LOOKUP), &target).unwrap();
    std::os::unix::fs::symlink(&target, dir.join(LOOKUP)).unwrap();
    let before = read(&target);
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "scripted",
            "--responses",
            repo().join("examples/scripted/replace-customer-lookup").to_str().unwrap(),
        ],
        &[],
    );
    assert_ne!(code, 0, "{report}");
    assert_ne!(report["outcome"], "accepted");
    assert_eq!(read(&target), before, "the file outside the project is untouched");
    assert_eq!(report["totals"]["modelCalls"], 0, "nothing behind the link was sent to a model: {report}");
}

// 8. A provider failure is reported as one (exit 4), never as a rejection or a limitation.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn a_provider_failure_is_reported_as_such() {
    let dir = project("examples/customers-app", "provider");
    let original = read(&dir.join(LOOKUP));
    let gw = FakeGateway::start(Box::new(|_: &Seen| {
        (403, r#"{"error":"Free tier users do not have access to this model"}"#.to_string(), Duration::ZERO)
    }));
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "openai-compatible",
            "--model",
            "m",
            "--base-url",
            &gw.base_url,
            "--api-key-env",
            "READINESS_PROVIDER_KEY",
        ],
        &[("READINESS_PROVIDER_KEY", "vck_readiness_provider_key")],
    );
    assert_eq!(code, 4, "{report}");
    assert_eq!(report["outcome"], "provider-error");
    assert!(report["reason"].as_str().unwrap().contains("403"));
    assert_eq!(report["projectState"], "unchanged");
    assert_eq!(read(&dir.join(LOOKUP)), original);
    assert_eq!(gw.completions().len(), 1, "a 403 is not retried");
}

// 9. A rate limit is retried inside the provider; the change is applied once.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn a_rate_limit_retry_applies_the_change_once() {
    let dir = project("examples/customers-app", "retry");
    let answer = scripted("examples/scripted/replace-customer-lookup/02.txt");
    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let gw = FakeGateway::start_with_headers(Box::new(move |_: &Seen| {
        if seen.fetch_add(1, Ordering::SeqCst) == 0 {
            (429, "{}".into(), Duration::ZERO, vec![("retry-after".into(), "0".into())])
        } else {
            (200, completion("m", &answer), Duration::ZERO, Vec::new())
        }
    }));
    let log = scratch("retry-log").join("run.jsonl");
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "openai-compatible",
            "--model",
            "m",
            "--base-url",
            &gw.base_url,
            "--api-key-env",
            "READINESS_NO_KEY",
            "--log",
            log.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 0, "{report}");
    assert_eq!(gw.completions().len(), 2, "one rate-limited request, one retry");
    assert_eq!(report["attempts"].as_array().unwrap().len(), 1, "one model call as far as the loop is concerned");
    let writes = read(&log).lines().filter(|l| l.contains("\"kind\":\"apply\"") && l.contains("inFlight")).count();
    assert_eq!(writes, 1, "the change was written once");
}

// 12. A run that dies while a change is applied leaves a record of what it changed.
#[cfg(unix)]
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra10_an_interrupted_run_leaves_an_accurate_recovery_record() {
    use std::os::unix::fs::PermissionsExt;
    let dir = project("examples/customers-app", "interrupted");
    let original = read(&dir.join(LOOKUP));
    let work = scratch("interrupt");
    // A stand-in for node that runs it normally, until the changed application is
    // started (the third run: discovery, baseline, candidate). Then it kills the agent.
    let node = work.join("node");
    let count = work.join("count");
    fs::write(
        &node,
        format!(
            "#!/bin/sh\nn=$(cat {c} 2>/dev/null || echo 0); n=$((n+1)); echo $n > {c}\nif [ \"$n\" -ge 3 ]; then kill -9 $PPID; exit 1; fi\nexec node \"$@\"\n",
            c = count.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&node, fs::Permissions::from_mode(0o755)).unwrap();
    let lookup_responses = repo().join("examples/scripted/replace-customer-lookup");
    let args = [
        "run",
        "--project",
        dir.to_str().unwrap(),
        "--intent",
        LOOKUP_INTENT,
        "--provider",
        "scripted",
        "--responses",
        lookup_responses.to_str().unwrap(),
    ];
    let (code, report) = agent(&[&args[..], &["--node", node.to_str().unwrap()]].concat(), &[]);
    assert_eq!(code, -1, "the agent was killed: {report}");
    assert_eq!(report["outcome"], "interrupted", "the report does not claim anything it did not finish");
    let file = &report["inFlight"]["files"][0];
    assert_eq!(file["path"], LOOKUP);
    assert_eq!(file["original"], original.as_str(), "the original is recorded, to restore by hand");
    assert_ne!(read(&dir.join(LOOKUP)), original, "and the change is really still there");
    assert_eq!(
        file["sha256Written"],
        agent_core::appport::knowledge::rules::sha256_hex(&read(&dir.join(LOOKUP))),
        "the record matches what is on disk"
    );

    // The dead run's lock does not block the next run.
    fs::write(dir.join(LOOKUP), &original).unwrap();
    let (code, report) = agent(&args, &[]);
    assert_eq!(code, 0, "{report}");
}

// 13. Model-written code never sees a provider credential, and the log never holds one.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra02_secrets_never_reach_the_application_or_the_log() {
    let key = "vck_readiness_secret_value_1234";
    std::env::set_var("AI_GATEWAY_API_KEY", key);
    let dir = project("examples/customers-app", "secret");
    let leak = format!(
        "=== FILE: {LOOKUP} ===\nimport {{ errors }} from \"@appport/protocol\";\nexport async function lookupCustomer() {{\n  throw errors.notFound(`leak ${{process.env.AI_GATEWAY_API_KEY}}`);\n}}\n=== END FILE ===\n"
    );
    let log = scratch("secret-log").join("run.jsonl");
    let mut model = ModelConfig::new(Provider::Scripted);
    model.responses = Some(responses("leak", &[leak]));
    let report = run(&RunOptions {
        project: dir.clone(),
        intent: LOOKUP_INTENT.into(),
        model,
        max_attempts: 1,
        host: HostConfig::default(),
        report: None,
        log: Some(log.clone()),
        quiet: true,
        context: Default::default(),
        goal_check: None,
        require_goal_evidence: true,
        allow_dirty: false,
    })
    .unwrap();
    std::env::remove_var("AI_GATEWAY_API_KEY");
    assert_eq!(report.outcome, "rejected");
    let text = read(&log);
    assert!(!text.contains(key), "the key reached the log");
    assert!(text.contains("leak undefined"), "the application ran without the credential");
}

// 14. Uncommitted work in a file the change would rewrite is never taken over.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn ra05_uncommitted_changes_are_never_taken_over() {
    let dir = project("examples/customers-app", "dirty");
    let git = |args: &[&str]| {
        let ok = Command::new("git").arg("-C").arg(&dir).args(args).output().unwrap().status.success();
        assert!(ok, "git {args:?}");
    };
    fs::write(dir.join(".gitignore"), "node_modules\n").unwrap();
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-qm", "base"]);
    let mine = format!("{}// my unfinished work\n", read(&dir.join(LOOKUP)));
    fs::write(dir.join(LOOKUP), &mine).unwrap();
    let lookup_responses = repo().join("examples/scripted/replace-customer-lookup");
    let args = [
        "run",
        "--project",
        dir.to_str().unwrap(),
        "--intent",
        LOOKUP_INTENT,
        "--provider",
        "scripted",
        "--responses",
        lookup_responses.to_str().unwrap(),
    ];
    let (code, report) = agent(&args, &[]);
    assert_eq!(code, 3, "{report}");
    assert!(report["reason"].as_str().unwrap().contains("uncommitted changes"));
    assert_eq!(read(&dir.join(LOOKUP)), mine, "the user's work is untouched");
    assert_eq!(report["totals"]["modelCalls"], 0);
}

// 15 (rejection). No attempt passes: exit 1, and the project is checked to be as it was.
#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn a_rejected_run_exits_1_with_the_project_restored() {
    let dir = project("examples/customers-app", "rejected");
    let original = read(&dir.join(LOOKUP));
    let only_bad = responses("bad", &[scripted("examples/scripted/replace-customer-lookup/01.txt")]);
    let (code, report) = agent(
        &[
            "run",
            "--project",
            dir.to_str().unwrap(),
            "--intent",
            LOOKUP_INTENT,
            "--provider",
            "scripted",
            "--responses",
            only_bad.to_str().unwrap(),
            "--max-attempts",
            "1",
        ],
        &[],
    );
    assert_eq!(code, 1, "{report}");
    assert_eq!(report["outcome"], "rejected");
    assert_eq!(report["projectState"], "unchanged");
    assert_eq!(report["attempts"][0]["rollback"]["restored"], json!([LOOKUP]));
    assert_eq!(read(&dir.join(LOOKUP)), original);
}

// Concurrent runs on one project: the second is refused before it does anything.
#[test]
fn ra06_a_second_run_on_the_same_project_is_refused() {
    let dir = scratch("locked");
    let _held = ProjectLock::acquire(&dir).unwrap();
    let mut model = ModelConfig::new(Provider::Scripted);
    model.responses = Some(dir.clone());
    let error = run(&RunOptions {
        project: dir.clone(),
        intent: LOOKUP_INTENT.into(),
        model,
        max_attempts: 1,
        host: HostConfig::default(),
        report: None,
        log: None,
        quiet: true,
        context: Default::default(),
        goal_check: None,
        require_goal_evidence: true,
        allow_dirty: false,
    })
    .unwrap_err();
    assert!(error.contains("another appport-agent run"), "{error}");
    assert!(ProjectLock::acquire(&dir).is_err(), "still held by the first");
}
