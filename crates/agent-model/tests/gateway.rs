//! The free-gateway preflight against a local stand-in for the Vercel AI
//! Gateway: every way it must stop, and that it sends nothing to the model
//! unless the model is shown to be free. No network, no inference.

mod support;

use std::time::Duration;

use agent_model::gateway::{preflight, GatewayPolicy, PreflightCategory, PreflightReport, DEFAULT_FREE_MODEL};
use agent_model::{ModelConfig, Provider};
use serde_json::json;
use support::fake_gateway::{catalog, completion, FakeGateway, Seen};

const FREE: &str = DEFAULT_FREE_MODEL;

/// A free-gateway configuration pointing at `base_url`, with its key in a variable unique to `test`.
fn config(base_url: &str, test: &str, key: Option<&str>) -> ModelConfig {
    let env = format!("AGENT_MODEL_TEST_GATEWAY_KEY_{}", test.to_uppercase());
    match key {
        Some(k) => std::env::set_var(&env, k),
        None => std::env::remove_var(&env),
    }
    let mut c = ModelConfig::new(Provider::OpenAiCompatible);
    c.model = Some(FREE.into());
    c.base_url = Some(base_url.into());
    c.api_key_env = Some(env);
    c
}

fn policy(gateway: &FakeGateway) -> GatewayPolicy {
    GatewayPolicy {
        base_urls: vec![gateway.base_url.clone()],
        timeout: Duration::from_secs(2),
        ..GatewayPolicy::default()
    }
}

/// A gateway answering the catalog and completions with fixed responses.
fn gateway(models: (u16, String), chat: (u16, String)) -> FakeGateway {
    FakeGateway::start(Box::new(move |seen: &Seen| {
        let (status, body) = if seen.path.ends_with("/models") { models.clone() } else { chat.clone() };
        (status, body, Duration::ZERO)
    }))
}

fn free_catalog() -> (u16, String) {
    (200, catalog(FREE, Some(json!({ "input": "0", "output": "0" }))))
}

fn assert_stopped(report: &PreflightReport, category: PreflightCategory) {
    assert!(!report.proceed, "{report:?}");
    assert_eq!(report.category, category, "{report:?}");
    assert!(!report.next_steps.is_empty(), "every failure says what to do next");
}

#[test]
fn a_free_model_passes_with_one_minimal_completion() {
    let gw = gateway(free_catalog(), (200, completion(FREE, "OK")));
    let report = preflight(&config(&gw.base_url, "pass", Some("vck_test_secret_pass")), &policy(&gw));
    assert!(report.proceed, "{report:?}");
    assert_eq!(report.category, PreflightCategory::Ok);
    assert_eq!(report.pricing, Some(json!({ "input": "0", "output": "0" })));
    let seen = gw.seen();
    assert_eq!(seen.len(), 2, "the catalog, then exactly one completion");
    assert_eq!((seen[0].method.as_str(), seen[0].path.as_str()), ("GET", "/v1/models"));
    let chat = &gw.completions()[0];
    assert_eq!(chat.body["model"], FREE, "the configured free model, nothing else");
    assert!(chat.body["max_tokens"].as_u64().unwrap() <= 256);
    assert_eq!(chat.authorization.as_deref(), Some("Bearer vck_test_secret_pass"));
    let written = serde_json::to_string(&report).unwrap();
    assert!(!written.contains("vck_test_secret_pass"), "the key never appears in the report");
}

#[test]
fn configuration_problems_stop_before_any_request() {
    let gw = gateway(free_catalog(), (200, completion(FREE, "OK")));

    let mut anthropic = config(&gw.base_url, "anthropic", Some("k-anthropic-123"));
    anthropic.provider = Provider::Anthropic;
    assert_stopped(&preflight(&anthropic, &policy(&gw)), PreflightCategory::Config);

    let elsewhere = config("https://api.anthropic.com/v1", "elsewhere", Some("k-elsewhere-123"));
    assert_stopped(&preflight(&elsewhere, &policy(&gw)), PreflightCategory::Config);

    let real_gateway_only = config(&gw.base_url, "default-policy", Some("k-default-123"));
    assert_stopped(&preflight(&real_gateway_only, &GatewayPolicy::default()), PreflightCategory::Config);

    let no_key = config(&gw.base_url, "nokey", None);
    let report = preflight(&no_key, &policy(&gw));
    assert_stopped(&report, PreflightCategory::Config);
    assert!(report.steps[0].detail.contains("is not set"));

    let mut no_model = config(&gw.base_url, "nomodel", Some("k-nomodel-123"));
    no_model.model = None;
    assert_stopped(&preflight(&no_model, &policy(&gw)), PreflightCategory::Config);

    let mut paid = config(&gw.base_url, "paid", Some("k-paid-12345"));
    paid.model = Some("anthropic/claude-opus-5.5".into());
    assert_stopped(&preflight(&paid, &policy(&gw)), PreflightCategory::NotAllowlisted);

    let mut suffix = config(&gw.base_url, "suffix", Some("k-suffix-1234"));
    suffix.model = Some("someone/unknown-model-free".into());
    assert_stopped(&preflight(&suffix, &policy(&gw)), PreflightCategory::NotAllowlisted);

    assert!(gw.seen().is_empty(), "nothing was sent anywhere");
}

#[test]
fn pricing_must_be_published_and_zero() {
    for (name, pricing, category) in [
        ("missing", None, PreflightCategory::PricingUnverified),
        ("partial", Some(json!({ "input": "0" })), PreflightCategory::PricingUnverified),
        ("unreadable", Some(json!({ "input": "free", "output": "free" })), PreflightCategory::PricingUnverified),
        ("input", Some(json!({ "input": "0.0000001", "output": "0" })), PreflightCategory::NotFree),
        ("output", Some(json!({ "input": 0, "output": 0.000002 })), PreflightCategory::NotFree),
    ] {
        let gw = gateway((200, catalog(FREE, pricing)), (200, completion(FREE, "OK")));
        let report = preflight(&config(&gw.base_url, &format!("pricing_{name}"), Some("k-pricing-123")), &policy(&gw));
        assert_stopped(&report, category);
        assert!(gw.completions().is_empty(), "{name}: no completion is requested from a model not shown to be free");
    }
}

#[test]
fn a_model_missing_from_the_catalog_is_unavailable() {
    let gw = gateway(
        (200, catalog("someone/other-model", Some(json!({ "input": "0", "output": "0" })))),
        (200, completion(FREE, "OK")),
    );
    assert_stopped(
        &preflight(&config(&gw.base_url, "missing", Some("k-missing-123")), &policy(&gw)),
        PreflightCategory::Unavailable,
    );
    assert!(gw.completions().is_empty());

    let gw = gateway((200, "<html>not json</html>".into()), (200, completion(FREE, "OK")));
    assert_stopped(
        &preflight(&config(&gw.base_url, "notjson", Some("k-notjson-123")), &policy(&gw)),
        PreflightCategory::Malformed,
    );
}

#[test]
fn provider_statuses_have_their_own_categories() {
    let free_tier = r#"{"error":{"message":"Free tier users do not have access to this model","type":"forbidden"}}"#;
    for (name, models, chat, category) in [
        ("catalog403", (403, free_tier.to_string()), (200, completion(FREE, "OK")), PreflightCategory::Forbidden),
        ("catalog401", (401, "{}".to_string()), (200, completion(FREE, "OK")), PreflightCategory::Auth),
        ("chat403", free_catalog(), (403, free_tier.to_string()), PreflightCategory::Forbidden),
        ("chat429", free_catalog(), (429, r#"{"error":"rate limited"}"#.to_string()), PreflightCategory::RateLimited),
        ("chat404", free_catalog(), (404, r#"{"error":"no such model"}"#.to_string()), PreflightCategory::Unavailable),
        ("chat500", free_catalog(), (500, "{}".to_string()), PreflightCategory::ProviderError),
    ] {
        let gw = gateway(models, chat);
        let report = preflight(&config(&gw.base_url, name, Some("k-status-123")), &policy(&gw));
        assert_stopped(&report, category);
        assert!(report.steps.last().unwrap().http_status.is_some(), "{name}: the status is recorded");
    }
}

#[test]
fn unusable_completions_are_malformed() {
    for (name, body) in [
        ("empty", completion(FREE, "  ")),
        ("nochoices", json!({ "choices": [] }).to_string()),
        ("notjson", "not json".to_string()),
    ] {
        let gw = gateway(free_catalog(), (200, body));
        assert_stopped(
            &preflight(&config(&gw.base_url, name, Some("k-malformed-1")), &policy(&gw)),
            PreflightCategory::Malformed,
        );
    }
}

#[test]
fn a_slow_gateway_times_out() {
    let gw = FakeGateway::start(Box::new(|seen: &Seen| {
        if seen.path.ends_with("/models") {
            (200, catalog(FREE, Some(json!({ "input": "0", "output": "0" }))), Duration::ZERO)
        } else {
            (200, completion(FREE, "OK"), Duration::from_secs(5))
        }
    }));
    let mut p = policy(&gw);
    p.timeout = Duration::from_millis(500);
    assert_stopped(&preflight(&config(&gw.base_url, "slow", Some("k-slow-12345")), &p), PreflightCategory::Timeout);
}

#[test]
fn an_unreachable_gateway_is_reported_as_such() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/v1", listener.local_addr().unwrap());
    drop(listener);
    let p = GatewayPolicy { base_urls: vec![url.clone()], timeout: Duration::from_secs(2), ..GatewayPolicy::default() };
    assert_stopped(&preflight(&config(&url, "unreachable", Some("k-unreach-123")), &p), PreflightCategory::Unreachable);
}

#[test]
fn a_key_echoed_by_the_provider_is_redacted() {
    let key = "vck_echoed_secret_value_42";
    let gw =
        gateway((403, format!(r#"{{"error":"key {key} may not use this model"}}"#)), (200, completion(FREE, "OK")));
    let report = preflight(&config(&gw.base_url, "echo", Some(key)), &policy(&gw));
    assert_stopped(&report, PreflightCategory::Forbidden);
    let written = serde_json::to_string(&report).unwrap() + &agent_model::gateway::preflight_text(&report);
    assert!(!written.contains(key), "{written}");
    assert!(written.contains("[redacted]"));
}
