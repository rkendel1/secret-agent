use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use ureq::tls::{RootCerts, TlsConfig};
use ureq::Agent;

use crate::{Model, ModelConfig, ModelError, Provider, Request, Response, Role, Usage};

fn role(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn openai_messages(request: &Request) -> Value {
    let mut messages = vec![json!({ "role": "system", "content": request.system })];
    for turn in &request.history {
        messages.push(json!({ "role": role(turn.role), "content": turn.text }));
    }
    messages.push(json!({ "role": "user", "content": request.prompt }));
    Value::Array(messages)
}

/// Assistant turns replay their content blocks unchanged (thinking included).
fn anthropic_messages(request: &Request) -> Value {
    let mut messages: Vec<Value> = request
        .history
        .iter()
        .map(|turn| match (&turn.raw, turn.role) {
            (Some(raw), Role::Assistant) => json!({ "role": "assistant", "content": raw }),
            _ => json!({ "role": role(turn.role), "content": turn.text }),
        })
        .collect();
    messages.push(json!({ "role": "user", "content": request.prompt }));
    Value::Array(messages)
}

/// Builds the model a configuration describes.
pub fn connect(config: &ModelConfig) -> Result<Box<dyn Model>, ModelError> {
    Ok(match config.provider {
        Provider::OpenAiCompatible => Box::new(OpenAiCompatibleModel::new(config)?),
        Provider::Anthropic => Box::new(AnthropicModel::new(config)?),
        Provider::Command => Box::new(CommandModel::new(config)?),
        Provider::Scripted => Box::new(ScriptedModel::new(config)?),
    })
}

fn agent(config: &ModelConfig) -> Agent {
    Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(config.timeout_secs)))
        .http_status_as_error(false)
        // The operating system's trust store, so TLS-inspecting proxies the
        // machine already trusts work too, not only the bundled Mozilla roots.
        .tls_config(TlsConfig::builder().root_certs(RootCerts::PlatformVerifier).build())
        .build()
        .into()
}

fn key(config: &ModelConfig, default_env: &str, required: bool) -> Result<Option<String>, ModelError> {
    let name = config.api_key_env.as_deref().unwrap_or(default_env);
    match std::env::var(name) {
        Ok(value) if !value.is_empty() => Ok(Some(value)),
        _ if required => Err(ModelError::Config(format!("set {name} to the API key"))),
        _ => Ok(None),
    }
}

/// When and how often a request is retried.
#[derive(Debug, Clone, Copy)]
struct Retry {
    max_retries: u32,
    max_wait: Duration,
}

impl Retry {
    fn of(config: &ModelConfig) -> Retry {
        Retry { max_retries: config.max_retries, max_wait: Duration::from_secs(config.max_retry_wait_secs) }
    }
}

/// Statuses worth retrying: rate limits and overloaded or failing servers.
fn retryable(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 504 | 529)
}

/// Exponential backoff from 500 ms, with up to 250 ms of jitter, capped.
fn backoff(retry: u32, cap: Duration) -> Duration {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
    let jitter = Duration::from_millis(u64::from(nanos % 250));
    (Duration::from_millis(500u64.saturating_mul(1 << retry.min(10))) + jitter).min(cap)
}

/// Sends one model request, retrying within `retry`. A model request has no
/// effect beyond its answer, so repeating it is safe (a paid provider may bill
/// each attempt). Retries happen here only: the agent's loops never retry a
/// failed model call, so attempts do not multiply. Returns the answer and the
/// number of retries it took. Secrets are redacted from any error body.
fn post(
    agent: &Agent,
    url: &str,
    headers: &[(&str, String)],
    body: &Value,
    retry: Retry,
    secrets: &[String],
) -> Result<(Value, u32), ModelError> {
    let clean = |text: &str| crate::gateway::redact(text, secrets);
    let mut retries = 0;
    loop {
        let mut request = agent.post(url);
        for (name, value) in headers {
            request = request.header(*name, value.as_str());
        }
        let (status, retry_after, text) = match request.send_json(body) {
            Ok(mut response) => {
                let status = response.status().as_u16();
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .map(Duration::from_secs);
                let text = response.body_mut().read_to_string().map_err(|e| ModelError::Transport(e.to_string()))?;
                (status, retry_after, text)
            }
            // A timeout may mean the request is still being served: never repeated.
            Err(ureq::Error::Timeout(t)) => return Err(ModelError::Transport(format!("{url}: timed out ({t})"))),
            Err(e) if retries < retry.max_retries => {
                retries += 1;
                std::thread::sleep(backoff(retries, retry.max_wait));
                let _ = e;
                continue;
            }
            Err(e) => {
                return Err(ModelError::Transport(format!(
                    "{url}: {e}{}",
                    if retries > 0 { format!(" (after {retries} retries)") } else { String::new() }
                )))
            }
        };
        if retryable(status) && retries < retry.max_retries {
            let wait = retry_after.unwrap_or_else(|| backoff(retries + 1, retry.max_wait));
            if wait <= retry.max_wait {
                retries += 1;
                std::thread::sleep(wait);
                continue;
            }
        }
        if !(200..300).contains(&status) {
            let mut body: String = clean(&text).chars().take(2000).collect();
            if retries > 0 {
                body.push_str(&format!(" (after {retries} retries)"));
            }
            if let Some(wait) = retry_after.filter(|w| *w > retry.max_wait) {
                body.push_str(&format!(
                    " (Retry-After {}s exceeds the {}s limit)",
                    wait.as_secs(),
                    retry.max_wait.as_secs()
                ));
            }
            return Err(ModelError::Provider { status, body });
        }
        return serde_json::from_str(&text).map(|v| (v, retries)).map_err(|e| {
            ModelError::Protocol(format!("not JSON ({e}): {}", clean(&text).chars().take(500).collect::<String>()))
        });
    }
}

fn trim_url(url: &str) -> &str {
    url.trim_end_matches('/')
}

/// OpenAI-compatible chat completions: local servers and remote APIs alike.
pub struct OpenAiCompatibleModel {
    agent: Agent,
    url: String,
    model: String,
    key: Option<String>,
    max_tokens: u32,
    temperature: Option<f64>,
    retry: Retry,
}

impl OpenAiCompatibleModel {
    pub fn new(config: &ModelConfig) -> Result<Self, ModelError> {
        let base = config.base_url.clone().unwrap_or_else(|| "http://localhost:11434/v1".into());
        Ok(Self {
            agent: agent(config),
            url: format!("{}/chat/completions", trim_url(&base)),
            model: config
                .model
                .clone()
                .ok_or_else(|| ModelError::Config("openai-compatible needs a model name".into()))?,
            key: key(config, "OPENAI_API_KEY", false)?,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            retry: Retry::of(config),
        })
    }
}

impl Model for OpenAiCompatibleModel {
    fn describe(&self) -> String {
        format!("openai-compatible:{} via {}", self.model, self.url)
    }

    fn complete(&mut self, request: &Request) -> Result<Response, ModelError> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens.min(self.max_tokens),
            "messages": openai_messages(request)
        });
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }
        let headers: Vec<(&str, String)> = self.key.iter().map(|k| ("authorization", format!("Bearer {k}"))).collect();
        let secrets: Vec<String> = self.key.iter().cloned().collect();
        let (answer, retries) = post(&self.agent, &self.url, &headers, &body, self.retry, &secrets)?;
        let finish = answer.pointer("/choices/0/finish_reason").and_then(Value::as_str).unwrap_or_default();
        // A refusal is the provider declining, never a proposal and never an agent outcome.
        if let Some(refusal) =
            answer.pointer("/choices/0/message/refusal").and_then(Value::as_str).filter(|r| !r.is_empty())
        {
            return Err(ModelError::Protocol(format!(
                "the model declined (stop_reason refusal, category {:?})",
                refusal.chars().take(200).collect::<String>()
            )));
        }
        if finish == "content_filter" {
            return Err(ModelError::Protocol(
                "the model declined (stop_reason refusal, category \"content_filter\")".into(),
            ));
        }
        let text = answer
            .pointer("/choices/0/message/content")
            .and_then(Value::as_str)
            .ok_or_else(|| ModelError::Protocol("no choices[0].message.content".into()))?
            .to_string();
        if text.trim().is_empty() {
            return Err(ModelError::Protocol(format!("empty completion (finish_reason {finish:?})")));
        }
        let usage = match (answer.pointer("/usage/prompt_tokens"), answer.pointer("/usage/completion_tokens")) {
            (Some(i), Some(o)) => Usage {
                input_tokens: i.as_u64().unwrap_or_default(),
                output_tokens: o.as_u64().unwrap_or_default(),
                estimated: false,
            },
            _ => Usage::estimate(&format!("{}{}", request.system, request.prompt), &text),
        };
        let model = answer.get("model").and_then(Value::as_str).unwrap_or(&self.model).to_string();
        let stop_reason = answer.pointer("/choices/0/finish_reason").and_then(Value::as_str).map(str::to_string);
        let raw = None;
        Ok(Response { text, usage, model, stop_reason, raw, retries })
    }
}

/// The Anthropic Messages API.
pub struct AnthropicModel {
    agent: Agent,
    url: String,
    model: String,
    key: String,
    workspace: Option<String>,
    max_tokens: u32,
    temperature: Option<f64>,
    effort: Option<String>,
    retry: Retry,
}

impl AnthropicModel {
    pub fn new(config: &ModelConfig) -> Result<Self, ModelError> {
        let base = config.base_url.clone().unwrap_or_else(|| "https://api.anthropic.com".into());
        Ok(Self {
            agent: agent(config),
            url: format!("{}/v1/messages", trim_url(&base)),
            model: config.model.clone().ok_or_else(|| ModelError::Config("anthropic needs a model name".into()))?,
            key: key(config, "ANTHROPIC_API_KEY", true)?.unwrap_or_default(),
            workspace: std::env::var(config.workspace_id_env.as_deref().unwrap_or("ANTHROPIC_WORKSPACE_ID"))
                .ok()
                .filter(|w| !w.is_empty()),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            effort: config.effort.clone(),
            retry: Retry::of(config),
        })
    }
}

impl Model for AnthropicModel {
    fn describe(&self) -> String {
        format!("anthropic:{} via {}", self.model, self.url)
    }

    fn complete(&mut self, request: &Request) -> Result<Response, ModelError> {
        let mut body = json!({
            "model": self.model,
            "max_tokens": request.max_tokens.min(self.max_tokens),
            "system": request.system,
            "messages": anthropic_messages(request)
        });
        if let Some(t) = self.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(effort) = &self.effort {
            body["output_config"] = json!({ "effort": effort });
        }
        let mut headers = vec![("x-api-key", self.key.clone()), ("anthropic-version", "2023-06-01".to_string())];
        if let Some(workspace) = &self.workspace {
            headers.push(("anthropic-workspace-id", workspace.clone()));
        }
        let secrets: Vec<String> = [Some(self.key.clone()), self.workspace.clone()].into_iter().flatten().collect();
        let (answer, retries) = post(&self.agent, &self.url, &headers, &body, self.retry, &secrets)?;
        let stop_reason = answer.get("stop_reason").and_then(Value::as_str).map(str::to_string);
        if stop_reason.as_deref() == Some("refusal") {
            // Never retried on another model here: that would change what is being measured.
            return Err(ModelError::Protocol(format!(
                "the model declined (stop_reason refusal, category {})",
                answer.pointer("/stop_details/category").unwrap_or(&Value::Null)
            )));
        }
        let raw = answer.get("content").cloned();
        let text: String = answer
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| ModelError::Protocol("no content blocks".into()))?
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect();
        if text.trim().is_empty() {
            return Err(ModelError::Protocol(format!(
                "empty completion (stop_reason {:?})",
                stop_reason.unwrap_or_default()
            )));
        }
        let usage = Usage {
            input_tokens: answer.pointer("/usage/input_tokens").and_then(Value::as_u64).unwrap_or_default(),
            output_tokens: answer.pointer("/usage/output_tokens").and_then(Value::as_u64).unwrap_or_default(),
            estimated: answer.get("usage").is_none(),
        };
        let model = answer.get("model").and_then(Value::as_str).unwrap_or(&self.model).to_string();
        Ok(Response { text, usage, model, stop_reason, raw, retries })
    }
}

/// A local program: the system and user prompts on stdin, the completion on stdout.
pub struct CommandModel {
    command: Vec<String>,
    timeout: Duration,
}

impl CommandModel {
    pub fn new(config: &ModelConfig) -> Result<Self, ModelError> {
        if config.command.is_empty() {
            return Err(ModelError::Config("command needs a program to run".into()));
        }
        Ok(Self { command: config.command.clone(), timeout: Duration::from_secs(config.timeout_secs) })
    }
}

impl Model for CommandModel {
    fn describe(&self) -> String {
        format!("command:{}", self.command[0])
    }

    fn complete(&mut self, request: &Request) -> Result<Response, ModelError> {
        let mut child = Command::new(&self.command[0])
            .args(&self.command[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| ModelError::Transport(format!("{}: {e}", self.command[0])))?;
        let input = format!("{}\n\n{}", request.system, request.prompt);
        let mut stdin = child.stdin.take().expect("piped");
        let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
        let mut stdout = child.stdout.take().expect("piped");
        let reader = std::thread::spawn(move || {
            let mut text = String::new();
            stdout.read_to_string(&mut text).map(|_| text)
        });
        let deadline = std::time::Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| ModelError::Transport(e.to_string()))? {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = child.kill();
                return Err(ModelError::Transport(format!("{} timed out", self.command[0])));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let _ = writer.join();
        let text = reader
            .join()
            .map_err(|_| ModelError::Transport("reader panicked".into()))?
            .map_err(|e| ModelError::Transport(e.to_string()))?;
        if !status.success() {
            return Err(ModelError::Transport(format!("{} exited with {status}", self.command[0])));
        }
        let usage = Usage::estimate(&format!("{}{}", request.system, request.prompt), &text);
        Ok(Response { text, usage, model: self.command[0].clone(), stop_reason: None, raw: None, retries: 0 })
    }
}

/// Replays pre-written completions (`01.txt`, `02.txt`, …) in order.
///
/// Not a model: it makes the loop reproducible without one, so the deterministic
/// core can be demonstrated and tested, including how it rejects a bad change.
pub struct ScriptedModel {
    files: Vec<PathBuf>,
    next: usize,
    dir: PathBuf,
}

impl ScriptedModel {
    pub fn new(config: &ModelConfig) -> Result<Self, ModelError> {
        let dir = config
            .responses
            .clone()
            .ok_or_else(|| ModelError::Config("scripted needs a responses directory".into()))?;
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| ModelError::Config(format!("{}: {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "txt"))
            .collect();
        files.sort();
        if files.is_empty() {
            return Err(ModelError::Config(format!("{} holds no .txt completions", dir.display())));
        }
        Ok(Self { files, next: 0, dir })
    }
}

impl Model for ScriptedModel {
    fn describe(&self) -> String {
        format!("scripted:{}", self.dir.display())
    }

    fn complete(&mut self, request: &Request) -> Result<Response, ModelError> {
        let path = self.files.get(self.next).ok_or_else(|| {
            ModelError::Transport(format!("scripted completions exhausted after {}", self.files.len()))
        })?;
        self.next += 1;
        let text =
            std::fs::read_to_string(path).map_err(|e| ModelError::Transport(format!("{}: {e}", path.display())))?;
        let usage = Usage::estimate(&format!("{}{}", request.system, request.prompt), &text);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        Ok(Response { text, usage, model: format!("scripted/{name}"), stop_reason: None, raw: None, retries: 0 })
    }
}
