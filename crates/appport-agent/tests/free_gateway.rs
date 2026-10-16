//! `arm-eval --free-gateway` against a local stand-in for the Vercel AI
//! Gateway. No network and no inference: the stand-in answers every request.

#[path = "../../agent-model/tests/support/fake_gateway.rs"]
mod fake_gateway;

use std::path::{Path, PathBuf};
use std::time::Duration;

use agent_model::gateway::{GatewayPolicy, DEFAULT_FREE_MODEL};
use agent_model::{ModelConfig, Provider};
use appport_agent::arms::{arm_eval, run_problem, Arm, ArmEvalOptions, SMOKE_TASKS};
use appport_agent::host::HostConfig;
use fake_gateway::{catalog, completion, free_gateway, FakeGateway, Seen};
use serde_json::json;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

fn options(gw: &FakeGateway, test: &str, tasks: &[&str]) -> ArmEvalOptions {
    let env = format!("APPPORT_AGENT_TEST_GATEWAY_KEY_{}", test.to_uppercase());
    std::env::set_var(&env, format!("vck_test_key_{test}"));
    let mut model = ModelConfig::new(Provider::OpenAiCompatible);
    model.model = Some(DEFAULT_FREE_MODEL.into());
    model.base_url = Some(gw.base_url.clone());
    model.api_key_env = Some(env);
    model.timeout_secs = 10;
    ArmEvalOptions {
        root: root(),
        tasks: tasks.iter().map(|t| root().join("fixtures/appport-tasks").join(t)).collect(),
        model,
        arms: vec![Arm::Bare, Arm::Context, Arm::Native],
        trials: 1,
        max_attempts: 3,
        max_turns: 4,
        host: HostConfig { node: "node".into(), timeout: Duration::from_secs(20) },
        out: std::env::temp_dir().join(format!("appport-agent-free-{test}-{}", std::process::id())),
        quiet: true,
        budget_usd: Some(0.0),
        max_model_calls: None,
        max_minutes: None,
        free_gateway: Some(GatewayPolicy { base_urls: vec![gw.base_url.clone()], ..GatewayPolicy::default() }),
    }
}

#[test]
fn a_failed_preflight_runs_no_trial() {
    // The model is listed but priced: the preflight must stop before any completion.
    let gw = FakeGateway::start(Box::new(|seen: &Seen| {
        if seen.path.ends_with("/models") {
            (200, catalog(DEFAULT_FREE_MODEL, Some(json!({ "input": "0.000001", "output": "0" }))), Duration::ZERO)
        } else {
            (200, completion(DEFAULT_FREE_MODEL, "OK"), Duration::ZERO)
        }
    }));
    let mut o = options(&gw, "priced", SMOKE_TASKS);
    o.budget_usd = None;
    let report = arm_eval(&o).unwrap();
    assert_eq!(report.status, "preflight-failed");
    assert!(report.trials.is_empty(), "zero trials");
    assert_eq!((report.counts.planned, report.counts.attempted, report.counts.not_run), (6, 0, 6));
    assert!(gw.completions().is_empty(), "nothing was sent to the model");
    assert!(run_problem(&report).unwrap().contains("preflight failed (not-free)"));
    for f in ["preflight.json", "preflight.txt", "arm-eval.json", "arm-eval.md"] {
        assert!(o.out.join(f).exists(), "{f}");
    }
    let md = std::fs::read_to_string(o.out.join("arm-eval.md")).unwrap();
    assert!(md.contains("Preflight failed: no trial ran"));
    let _ = std::fs::remove_dir_all(&o.out);
}

#[test]
fn a_paid_or_direct_configuration_is_refused_without_a_request() {
    let gw = free_gateway(DEFAULT_FREE_MODEL, "OK");
    let mut o = options(&gw, "direct", SMOKE_TASKS);
    o.budget_usd = None;
    o.model.provider = Provider::Anthropic;
    let report = arm_eval(&o).unwrap();
    assert_eq!(report.status, "preflight-failed");
    assert!(report.trials.is_empty());

    let mut o = options(&gw, "paid", SMOKE_TASKS);
    o.budget_usd = None;
    o.model.model = Some("anthropic/claude-opus-5.5".into());
    let report = arm_eval(&o).unwrap();
    assert_eq!(report.status, "preflight-failed");
    assert!(gw.seen().is_empty(), "no request reached any endpoint");
    let _ = std::fs::remove_dir_all(&o.out);
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn the_free_model_is_the_only_model_any_arm_reaches() {
    // Every completion is unusable, so no arm can succeed; what matters is where the calls go.
    let gw = free_gateway(DEFAULT_FREE_MODEL, "I am not able to help with this.");
    let o = options(&gw, "propagates", SMOKE_TASKS);
    let report = arm_eval(&o).unwrap();
    assert_eq!(report.status, "complete");
    assert!(report.preflight.as_ref().unwrap().proceed);
    assert_eq!(report.trials.len(), 6);
    for t in &report.trials {
        assert_eq!((t.provider.as_str(), t.model.as_str()), ("openai-compatible", DEFAULT_FREE_MODEL));
        assert_eq!(t.cost_usd, Some(0.0), "free models are priced at zero");
        assert_ne!(t.category, "verified-success");
        assert!(!matches!(t.category.as_str(), "provider-error" | "provider-refusal"), "{t:?}");
    }
    let completions = gw.completions();
    let calls: usize = report.trials.iter().map(|t| t.model_calls).sum();
    assert_eq!(completions.len(), calls + 1, "the preflight's one completion, then exactly the trials' calls");
    assert!(completions.iter().all(|c| c.body["model"] == DEFAULT_FREE_MODEL), "no other model, no fallback");
    for arm in ["A-bare", "B-context", "C-native"] {
        assert!(report.trials.iter().any(|t| t.arm == arm && t.model_calls > 0), "{arm} reached the gateway");
    }
    // The key is never written to a record.
    let key = "vck_test_key_propagates";
    for entry in walk(&o.out) {
        let text = std::fs::read_to_string(&entry).unwrap_or_default();
        assert!(!text.contains(key), "{} holds the key", entry.display());
    }
    let _ = std::fs::remove_dir_all(&o.out);
}

#[test]
#[ignore = "needs Node.js and `npm ci` in examples/customers-app"]
fn the_model_call_cap_is_never_exceeded() {
    let gw = free_gateway(DEFAULT_FREE_MODEL, "I am not able to help with this.");
    let mut o = options(&gw, "capped", SMOKE_TASKS);
    o.max_model_calls = Some(7);
    let report = arm_eval(&o).unwrap();
    assert!(report.counts.model_calls <= 7, "{:?}", report.counts);
    assert_eq!(gw.completions().len(), report.counts.model_calls + 1);
    let skipped: Vec<_> = report.trials.iter().filter(|t| t.category == "not-run").collect();
    assert!(!skipped.is_empty());
    assert!(skipped.iter().all(|t| t.reason.as_deref().unwrap().contains("cap of 7 model calls")));
    assert_eq!(report.counts.not_run, skipped.len());
    assert_eq!(report.trials.len(), 6, "skipped trials are listed, never dropped");
    let _ = std::fs::remove_dir_all(&o.out);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}
