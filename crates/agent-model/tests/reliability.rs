//! Provider failure handling against a local stand-in server: bounded retries
//! that respect `Retry-After`, no retry of a timeout or an auth failure,
//! refusals and empty answers reported as provider failures, and secrets kept
//! out of errors. No network, no inference.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_model::{connect, ModelConfig, Provider, Request};
use serde_json::json;
use support::fake_gateway::{completion, FakeGateway, Seen};

fn config(gw: &FakeGateway) -> ModelConfig {
    let mut c = ModelConfig::new(Provider::OpenAiCompatible);
    c.model = Some("m".into());
    c.base_url = Some(gw.base_url.clone());
    c.api_key_env = Some("AGENT_MODEL_RELIABILITY_NO_KEY".into());
    c
}

fn request() -> Request {
    Request::new("system", "prompt", 64)
}

/// Answers `failures` times with `status` (and `headers`), then with a completion.
fn flaky(failures: usize, status: u16, headers: Vec<(&'static str, &'static str)>) -> FakeGateway {
    let count = Arc::new(AtomicUsize::new(0));
    FakeGateway::start_with_headers(Box::new(move |_: &Seen| {
        let n = count.fetch_add(1, Ordering::SeqCst);
        let headers: Vec<(String, String)> = headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        if n < failures {
            (status, json!({ "error": "busy" }).to_string(), Duration::ZERO, headers)
        } else {
            (200, completion("m", "the answer"), Duration::ZERO, Vec::new())
        }
    }))
}

#[test]
fn a_rate_limit_is_retried_after_the_requested_wait() {
    let gw = flaky(1, 429, vec![("retry-after", "1")]);
    let began = Instant::now();
    let response = connect(&config(&gw)).unwrap().complete(&request()).unwrap();
    assert_eq!((response.text.as_str(), response.retries), ("the answer", 1));
    assert_eq!(gw.completions().len(), 2, "one failed request, one retry");
    assert!(began.elapsed() >= Duration::from_secs(1), "Retry-After was respected");
}

#[test]
fn server_errors_are_retried_with_backoff_up_to_the_limit() {
    let gw = flaky(usize::MAX, 503, vec![]);
    let error = connect(&config(&gw)).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(error.contains("503") && error.contains("after 2 retries"), "{error}");
    assert_eq!(gw.completions().len(), 3, "the default: two retries, three requests in all");

    let gw = flaky(2, 502, vec![]);
    let response = connect(&config(&gw)).unwrap().complete(&request()).unwrap();
    assert_eq!(response.retries, 2);
}

#[test]
fn a_retry_after_beyond_the_limit_is_not_waited_for() {
    let gw = flaky(usize::MAX, 429, vec![("retry-after", "3600")]);
    let began = Instant::now();
    let error = connect(&config(&gw)).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(error.contains("429") && error.contains("Retry-After 3600s exceeds"), "{error}");
    assert_eq!(gw.completions().len(), 1);
    assert!(began.elapsed() < Duration::from_secs(5));
}

#[test]
fn auth_and_permission_failures_are_never_retried() {
    for status in [400, 401, 403, 404] {
        let gw = flaky(usize::MAX, status, vec![]);
        let error = connect(&config(&gw)).unwrap().complete(&request()).unwrap_err().to_string();
        assert!(error.contains(&status.to_string()), "{error}");
        assert_eq!(gw.completions().len(), 1, "{status} is not retried");
    }
}

#[test]
fn a_timeout_is_never_retried() {
    let gw = FakeGateway::start(Box::new(|_: &Seen| (200, completion("m", "late"), Duration::from_secs(3))));
    let mut c = config(&gw);
    c.timeout_secs = 1;
    let error = connect(&c).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(error.contains("timed out"), "{error}");
    assert_eq!(gw.completions().len(), 1, "the request may still be served; it is not sent again");
}

#[test]
fn refusals_and_empty_answers_are_provider_failures_not_proposals() {
    for (name, body) in [
        (
            "refusal",
            json!({ "choices": [{ "message": { "content": null, "refusal": "I can't help with that." }, "finish_reason": "stop" }] }),
        ),
        ("filter", json!({ "choices": [{ "message": { "content": "" }, "finish_reason": "content_filter" }] })),
    ] {
        let text = body.to_string();
        let gw = FakeGateway::start(Box::new(move |_: &Seen| (200, text.clone(), Duration::ZERO)));
        let error = connect(&config(&gw)).unwrap().complete(&request()).unwrap_err().to_string();
        assert!(error.contains("declined (stop_reason refusal"), "{name}: {error}");
    }
    let gw = FakeGateway::start(Box::new(|_: &Seen| (200, completion("m", "  \n"), Duration::ZERO)));
    let error = connect(&config(&gw)).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(error.contains("empty completion"), "{error}");
}

#[test]
fn a_key_echoed_in_an_error_body_is_redacted() {
    let key = "sk-reliability-echo-0123456789";
    std::env::set_var("AGENT_MODEL_RELIABILITY_ECHO_KEY", key);
    let body = format!(r#"{{"error":"invalid key {key}"}}"#);
    let gw = FakeGateway::start(Box::new(move |_: &Seen| (401, body.clone(), Duration::ZERO)));
    let mut c = config(&gw);
    c.api_key_env = Some("AGENT_MODEL_RELIABILITY_ECHO_KEY".into());
    let error = connect(&c).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(!error.contains(key) && error.contains("[redacted]"), "{error}");
}
