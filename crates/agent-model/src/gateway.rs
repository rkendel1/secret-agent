//! The free-model gateway policy and its preflight.
//!
//! The live evaluation may only reach a model through the Vercel AI Gateway,
//! and only a model that is both on [`FREE_GATEWAY_MODELS`] and published by
//! the gateway with zero input and output pricing. A successful request alone
//! never establishes that a model is free; the catalog entry does, and a
//! model whose pricing cannot be read is rejected.
//!
//! [`preflight`] checks all of this, then makes one minimal completion, before
//! any trial runs. Every failure has its own [`PreflightCategory`], so a
//! provider problem is never mistaken for an agent outcome. Secrets never
//! appear in a report: everything written passes through [`redact`].

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use ureq::Agent;

use crate::{ModelConfig, Provider};

/// Gateway model ids verified as free-priced (input and output). Being listed
/// is necessary, not sufficient: the live catalog must also say so.
pub const FREE_GATEWAY_MODELS: &[&str] = &["nvidia/nemotron-3.5-lightning-free"];

/// The default model of the free live evaluation.
pub const DEFAULT_FREE_MODEL: &str = FREE_GATEWAY_MODELS[0];

/// The only live endpoint the free evaluation may use.
pub const GATEWAY_BASE_URL: &str = "https://ai-gateway.vercel.sh/v1";

/// Where the free evaluation may send requests.
#[derive(Debug, Clone)]
pub struct GatewayPolicy {
    /// Accepted base URLs (tests point this at a local stand-in).
    pub base_urls: Vec<String>,
    /// Accepted model ids.
    pub allowlist: Vec<String>,
    pub timeout: Duration,
}

impl Default for GatewayPolicy {
    fn default() -> Self {
        GatewayPolicy {
            base_urls: vec![GATEWAY_BASE_URL.to_string()],
            allowlist: FREE_GATEWAY_MODELS.iter().map(|s| s.to_string()).collect(),
            timeout: Duration::from_secs(60),
        }
    }
}

/// Why a preflight stopped, or `Ok`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PreflightCategory {
    Ok,
    /// The configuration is not a free-gateway configuration (wrong provider or endpoint, no key, no model).
    Config,
    /// The model is not on the free allowlist.
    NotAllowlisted,
    /// The catalog does not publish pricing for the model, so it cannot be shown to be free.
    PricingUnverified,
    /// The catalog prices the model above zero.
    NotFree,
    /// The model is not in the catalog, or the gateway says it does not exist.
    Unavailable,
    /// The gateway cannot be reached.
    Unreachable,
    /// The key is missing or rejected (401).
    Auth,
    /// The key may not use the model (403), e.g. a free-tier account restriction.
    Forbidden,
    RateLimited,
    Timeout,
    /// The response is not a usable completion.
    Malformed,
    /// Any other provider failure (5xx and the like).
    ProviderError,
}

/// One preflight step.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightStep {
    pub step: String,
    pub ok: bool,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

/// The preflight result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    /// Whether the live evaluation may proceed.
    pub proceed: bool,
    pub category: PreflightCategory,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    /// The catalog's prices for the model, as published (per token).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pricing: Option<Value>,
    pub steps: Vec<PreflightStep>,
    pub next_steps: Vec<String>,
}

/// Replaces every occurrence of each secret with `[redacted]`.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for secret in secrets.iter().filter(|s| s.len() >= 8) {
        out = out.replace(secret.as_str(), "[redacted]");
    }
    out
}

/// The secrets a configuration can expose (the API key, when set).
pub fn secrets_of(config: &ModelConfig) -> Vec<String> {
    let default = if config.provider == Provider::Anthropic { "ANTHROPIC_API_KEY" } else { "OPENAI_API_KEY" };
    let name = config.api_key_env.as_deref().unwrap_or(default);
    std::env::var(name).ok().filter(|v| !v.is_empty()).into_iter().collect()
}

fn next_steps(category: PreflightCategory, model: &str, key_env: &str) -> Vec<String> {
    let s = |t: &str| t.to_string();
    match category {
        PreflightCategory::Ok => vec![],
        PreflightCategory::Config => vec![
            format!("set the {key_env} secret (a Vercel AI Gateway key) and use the openai-compatible provider at {GATEWAY_BASE_URL}"),
            s("the free evaluation never calls Anthropic's API or any other endpoint directly"),
        ],
        PreflightCategory::NotAllowlisted => vec![
            format!("{model} is not on the free allowlist ({})", FREE_GATEWAY_MODELS.join(", ")),
            s("to add a model, confirm in the gateway catalog that its input and output prices are zero, then add it to FREE_GATEWAY_MODELS in crates/agent-model/src/gateway.rs"),
        ],
        PreflightCategory::PricingUnverified => vec![
            s("the gateway catalog did not publish pricing for this model, so it cannot be shown to be free; nothing was sent to the model"),
        ],
        PreflightCategory::NotFree => vec![format!("the gateway prices {model} above zero; choose a free model")],
        PreflightCategory::Unavailable => vec![format!("{model} is not offered by the gateway now; choose another allowlisted free model")],
        PreflightCategory::Unreachable => vec![s("the gateway could not be reached; check network access and retry")],
        PreflightCategory::Auth => vec![format!("the gateway rejected the key in {key_env}; check the secret")],
        PreflightCategory::Forbidden => vec![
            s("the gateway account may not use this model (HTTP 403); check the account's plan and model access in the Vercel dashboard"),
            s("no paid model is substituted"),
        ],
        PreflightCategory::RateLimited => vec![s("the gateway rate-limited the request; retry later")],
        PreflightCategory::Timeout => vec![s("the gateway or model did not answer in time; retry later")],
        PreflightCategory::Malformed => vec![s("the model answered with no usable completion; it may not be suitable for this evaluation")],
        PreflightCategory::ProviderError => vec![s("the provider failed; retry later")],
    }
}

fn status_category(status: u16) -> PreflightCategory {
    match status {
        401 => PreflightCategory::Auth,
        403 => PreflightCategory::Forbidden,
        404 => PreflightCategory::Unavailable,
        408 => PreflightCategory::Timeout,
        429 => PreflightCategory::RateLimited,
        _ => PreflightCategory::ProviderError,
    }
}

/// Reads a price (string or number) as a number.
fn price_of(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `(input, output)` prices from a catalog entry, if both are published.
pub fn catalog_pricing(entry: &Value) -> Option<(f64, f64)> {
    let pricing = entry.get("pricing")?;
    let input = ["input", "prompt", "input_tokens"].iter().find_map(|k| price_of(pricing.get(*k)))?;
    let output = ["output", "completion", "output_tokens"].iter().find_map(|k| price_of(pricing.get(*k)))?;
    Some((input, output))
}

enum Http {
    Status(u16, String),
    Timeout,
    Unreachable(String),
}

fn http(agent: &Agent, method: &str, url: &str, key: &str, body: Option<&Value>) -> (Http, u64) {
    let began = Instant::now();
    let auth = format!("Bearer {key}");
    let result = match body {
        Some(body) => agent.post(url).header("authorization", auth.as_str()).send_json(body),
        None => agent.get(url).header("authorization", auth.as_str()).call(),
    };
    let elapsed = began.elapsed().as_millis() as u64;
    let _ = method;
    match result {
        Ok(mut response) => {
            let status = response.status().as_u16();
            let text = response.body_mut().read_to_string().unwrap_or_default();
            (Http::Status(status, text), elapsed)
        }
        Err(ureq::Error::Timeout(_)) => (Http::Timeout, elapsed),
        Err(e) => {
            let text = e.to_string();
            if text.to_lowercase().contains("timed out") || text.to_lowercase().contains("timeout") {
                (Http::Timeout, elapsed)
            } else {
                (Http::Unreachable(text), elapsed)
            }
        }
    }
}

/// Checks the configuration against the policy, the catalog, and one minimal
/// completion. Sends nothing to the model unless the model is shown to be free.
pub fn preflight(config: &ModelConfig, policy: &GatewayPolicy) -> PreflightReport {
    let model = config.model.clone().unwrap_or_default();
    let base = config.base_url.clone().unwrap_or_default().trim_end_matches('/').to_string();
    let key_env = config.api_key_env.clone().unwrap_or_else(|| "AI_GATEWAY_API_KEY".into());
    let secrets = secrets_of(config);
    let mut report = PreflightReport {
        proceed: false,
        category: PreflightCategory::Ok,
        provider: "vercel-ai-gateway".into(),
        model: model.clone(),
        base_url: base.clone(),
        pricing: None,
        steps: Vec::new(),
        next_steps: Vec::new(),
    };
    let stop = |report: &mut PreflightReport, category: PreflightCategory, step: PreflightStep| {
        let mut step = step;
        step.detail = redact(&step.detail, &secrets);
        report.steps.push(step);
        report.category = category;
        report.next_steps = next_steps(category, &model, &key_env);
    };
    let step = |name: &str, ok: bool, detail: String| PreflightStep {
        step: name.into(),
        ok,
        detail,
        http_status: None,
        latency_ms: None,
    };

    // 1. Configuration, without printing secrets.
    let problem = if config.provider != Provider::OpenAiCompatible {
        Some(format!("provider must be openai-compatible (the gateway), not {:?}", config.provider))
    } else if !policy.base_urls.iter().any(|b| b.trim_end_matches('/') == base) {
        Some(format!("endpoint {base:?} is not the Vercel AI Gateway ({})", policy.base_urls.join(", ")))
    } else if model.is_empty() {
        Some("no model is configured".into())
    } else if secrets.is_empty() {
        Some(format!("{key_env} is not set"))
    } else {
        None
    };
    if let Some(problem) = problem {
        stop(&mut report, PreflightCategory::Config, step("configuration", false, problem));
        return report;
    }
    report.steps.push(step("configuration", true, format!("gateway {base}, key from {key_env} (not shown)")));

    // 2. The allowlist.
    if !policy.allowlist.iter().any(|m| m == &model) {
        stop(
            &mut report,
            PreflightCategory::NotAllowlisted,
            step("free allowlist", false, format!("{model} is not allowlisted")),
        );
        return report;
    }
    report.steps.push(step("free allowlist", true, format!("{model} is allowlisted")));

    let agent: Agent = Agent::config_builder()
        .timeout_global(Some(policy.timeout))
        .http_status_as_error(false)
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .build()
        .into();
    let key = secrets[0].clone();

    // 3. Reachability and the catalog's pricing for the model.
    let (catalog, latency) = http(&agent, "GET", &format!("{base}/models"), &key, None);
    let entry = match catalog {
        Http::Timeout => {
            stop(
                &mut report,
                PreflightCategory::Timeout,
                PreflightStep { latency_ms: Some(latency), ..step("catalog", false, "timed out".into()) },
            );
            return report;
        }
        Http::Unreachable(e) => {
            stop(
                &mut report,
                PreflightCategory::Unreachable,
                PreflightStep { latency_ms: Some(latency), ..step("catalog", false, e) },
            );
            return report;
        }
        Http::Status(status, text) if !(200..300).contains(&status) => {
            stop(
                &mut report,
                status_category(status),
                PreflightStep {
                    http_status: Some(status),
                    latency_ms: Some(latency),
                    ..step("catalog", false, text.chars().take(400).collect())
                },
            );
            return report;
        }
        Http::Status(status, text) => {
            let parsed: Option<Value> = serde_json::from_str(&text).ok();
            let entry = parsed.as_ref().and_then(|v| v.get("data")).and_then(Value::as_array).and_then(|data| {
                data.iter().find(|m| m.get("id").and_then(Value::as_str) == Some(model.as_str())).cloned()
            });
            match (parsed.is_some(), entry) {
                (false, _) => {
                    stop(
                        &mut report,
                        PreflightCategory::Malformed,
                        PreflightStep {
                            http_status: Some(status),
                            latency_ms: Some(latency),
                            ..step("catalog", false, "the catalog is not JSON".into())
                        },
                    );
                    return report;
                }
                (true, None) => {
                    stop(
                        &mut report,
                        PreflightCategory::Unavailable,
                        PreflightStep {
                            http_status: Some(status),
                            latency_ms: Some(latency),
                            ..step("catalog", false, format!("{model} is not in the catalog"))
                        },
                    );
                    return report;
                }
                (true, Some(entry)) => {
                    report.steps.push(PreflightStep {
                        http_status: Some(status),
                        latency_ms: Some(latency),
                        ..step("catalog", true, format!("{model} is in the catalog"))
                    });
                    entry
                }
            }
        }
    };
    report.pricing = entry.get("pricing").cloned();
    match catalog_pricing(&entry) {
        None => {
            stop(
                &mut report,
                PreflightCategory::PricingUnverified,
                step("free pricing", false, "the catalog publishes no input/output pricing for this model".into()),
            );
            return report;
        }
        Some((input, output)) if input != 0.0 || output != 0.0 => {
            stop(
                &mut report,
                PreflightCategory::NotFree,
                step("free pricing", false, format!("input {input}, output {output} per token")),
            );
            return report;
        }
        Some(_) => report.steps.push(step("free pricing", true, "input and output are priced at 0".into())),
    }

    // 4–5. One minimal, non-streaming completion.
    let body = json!({
        "model": model,
        "max_tokens": 256,
        "messages": [{ "role": "user", "content": "Reply with the single word OK." }]
    });
    let (answer, latency) = http(&agent, "POST", &format!("{base}/chat/completions"), &key, Some(&body));
    match answer {
        Http::Timeout => stop(
            &mut report,
            PreflightCategory::Timeout,
            PreflightStep { latency_ms: Some(latency), ..step("completion", false, "timed out".into()) },
        ),
        Http::Unreachable(e) => stop(
            &mut report,
            PreflightCategory::Unreachable,
            PreflightStep { latency_ms: Some(latency), ..step("completion", false, e) },
        ),
        Http::Status(status, text) if !(200..300).contains(&status) => stop(
            &mut report,
            status_category(status),
            PreflightStep {
                http_status: Some(status),
                latency_ms: Some(latency),
                ..step("completion", false, text.chars().take(400).collect())
            },
        ),
        Http::Status(status, text) => {
            let content = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.pointer("/choices/0/message/content").and_then(Value::as_str).map(str::to_string));
            match content {
                Some(c) if !c.trim().is_empty() => {
                    report.steps.push(PreflightStep {
                        http_status: Some(status),
                        latency_ms: Some(latency),
                        ..step(
                            "completion",
                            true,
                            format!("answered {:?}", c.trim().chars().take(40).collect::<String>()),
                        )
                    });
                    report.proceed = true;
                }
                Some(_) => stop(
                    &mut report,
                    PreflightCategory::Malformed,
                    PreflightStep {
                        http_status: Some(status),
                        latency_ms: Some(latency),
                        ..step("completion", false, "the completion is empty".into())
                    },
                ),
                None => stop(
                    &mut report,
                    PreflightCategory::Malformed,
                    PreflightStep {
                        http_status: Some(status),
                        latency_ms: Some(latency),
                        ..step("completion", false, "no choices[0].message.content".into())
                    },
                ),
            }
        }
    }
    for s in &mut report.steps {
        s.detail = redact(&s.detail, &secrets);
    }
    report
}

/// A human-readable preflight summary.
pub fn preflight_text(report: &PreflightReport) -> String {
    let mut text = format!(
        "preflight: {} ({}) — {} via {}\n",
        if report.proceed { "PASS" } else { "FAIL" },
        serde_json::to_value(report.category).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default(),
        report.model,
        report.base_url
    );
    for s in &report.steps {
        text.push_str(&format!(
            "  {} {}: {}{}{}\n",
            if s.ok { "ok  " } else { "FAIL" },
            s.step,
            s.detail,
            s.http_status.map(|h| format!(" (HTTP {h})")).unwrap_or_default(),
            s.latency_ms.map(|l| format!(" [{l} ms]")).unwrap_or_default()
        ));
    }
    for n in &report.next_steps {
        text.push_str(&format!("  next: {n}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_published_pricing_in_either_shape() {
        assert_eq!(catalog_pricing(&json!({ "pricing": { "input": "0", "output": "0" } })), Some((0.0, 0.0)));
        assert_eq!(
            catalog_pricing(&json!({ "pricing": { "prompt": 0.000001, "completion": 0 } })),
            Some((0.000001, 0.0))
        );
        assert_eq!(catalog_pricing(&json!({ "pricing": { "input": "0" } })), None, "output missing");
        assert_eq!(catalog_pricing(&json!({ "id": "x" })), None);
    }

    #[test]
    fn redacts_secrets() {
        let key = "vck_secret_value_123".to_string();
        assert_eq!(redact("Bearer vck_secret_value_123 failed", &[key]), "Bearer [redacted] failed");
        assert_eq!(redact("short", &["abc".into()]), "short", "too-short strings are not treated as secrets");
    }
}
