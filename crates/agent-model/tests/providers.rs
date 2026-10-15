//! Each provider against a local stand-in for its API: the same `Model` trait,
//! different configuration, no code change in the caller.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::thread;

use agent_model::{connect, ModelConfig, Provider, Request};
use serde_json::{json, Value};

/// Serves one HTTP request with `body`; returns what the client sent (headers, JSON body).
fn serve_once(status: u16, body: Value) -> (String, thread::JoinHandle<(Vec<String>, Value)>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut headers = Vec::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
            headers.push(line.trim().to_lowercase());
        }
        let length: usize = headers
            .iter()
            .find_map(|h| h.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
            .unwrap_or(0);
        let mut payload = vec![0; length];
        reader.read_exact(&mut payload).unwrap();
        let text = body.to_string();
        let mut stream = stream;
        write!(stream, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len()).unwrap();
        (headers, serde_json::from_slice(&payload).unwrap())
    });
    (url, handle)
}

fn request() -> Request {
    Request { system: "system text".into(), prompt: "prompt text".into(), max_tokens: 100 }
}

#[test]
fn openai_compatible_local_server() {
    let (url, server) = serve_once(
        200,
        json!({ "model": "qwen2.5-coder", "choices": [{ "message": { "content": "hello" } }], "usage": { "prompt_tokens": 12, "completion_tokens": 3 } }),
    );
    let mut config = ModelConfig::new(Provider::OpenAiCompatible);
    config.model = Some("qwen2.5-coder".into());
    config.base_url = Some(format!("{url}/v1/"));
    config.api_key_env = Some("AGENT_MODEL_TEST_NO_SUCH_KEY".into());
    let mut model = connect(&config).unwrap();
    let response = model.complete(&request()).unwrap();
    assert_eq!((response.text.as_str(), response.usage.input_tokens, response.usage.output_tokens), ("hello", 12, 3));
    assert!(!response.usage.estimated);
    let (headers, body) = server.join().unwrap();
    assert!(headers[0].starts_with("post /v1/chat/completions "));
    assert!(!headers.iter().any(|h| h.starts_with("authorization:")), "no key is sent to a keyless local server");
    assert_eq!(body["messages"][0], json!({ "role": "system", "content": "system text" }));
    assert_eq!(body["messages"][1], json!({ "role": "user", "content": "prompt text" }));
    assert_eq!(body["max_tokens"], 100);
}

#[test]
fn anthropic_messages_api() {
    std::env::set_var("AGENT_MODEL_TEST_ANTHROPIC_KEY", "test-key");
    let (url, server) = serve_once(
        200,
        json!({ "model": "claude-x", "content": [{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }], "usage": { "input_tokens": 20, "output_tokens": 2 } }),
    );
    let mut config = ModelConfig::new(Provider::Anthropic);
    config.model = Some("claude-x".into());
    config.base_url = Some(url);
    config.api_key_env = Some("AGENT_MODEL_TEST_ANTHROPIC_KEY".into());
    let response = connect(&config).unwrap().complete(&request()).unwrap();
    assert_eq!((response.text.as_str(), response.usage.input_tokens, response.model.as_str()), ("ab", 20, "claude-x"));
    let (headers, body) = server.join().unwrap();
    assert!(headers[0].starts_with("post /v1/messages "));
    assert!(headers.contains(&"x-api-key: test-key".to_string()));
    assert!(headers.contains(&"anthropic-version: 2023-06-01".to_string()));
    assert_eq!(body["system"], "system text");
    assert_eq!(body["messages"], json!([{ "role": "user", "content": "prompt text" }]));
}

#[test]
fn provider_errors_are_reported_not_swallowed() {
    let (url, server) = serve_once(429, json!({ "error": "slow down" }));
    let mut config = ModelConfig::new(Provider::OpenAiCompatible);
    config.model = Some("m".into());
    config.base_url = Some(url);
    let error = connect(&config).unwrap().complete(&request()).unwrap_err().to_string();
    assert!(error.contains("429") && error.contains("slow down"), "{error}");
    server.join().unwrap();
}

#[test]
fn anthropic_requires_a_key() {
    let mut config = ModelConfig::new(Provider::Anthropic);
    config.model = Some("m".into());
    config.api_key_env = Some("AGENT_MODEL_TEST_UNSET_KEY".into());
    assert!(connect(&config).err().unwrap().to_string().contains("AGENT_MODEL_TEST_UNSET_KEY"));
}

#[test]
fn command_provider_pipes_the_prompt() {
    let mut config = ModelConfig::new(Provider::Command);
    config.command = vec!["sh".into(), "-c".into(), "wc -c".into()];
    let response = connect(&config).unwrap().complete(&request()).unwrap();
    let expected = "system text\n\nprompt text".len();
    assert_eq!(response.text.trim().parse::<usize>().unwrap(), expected);
    assert!(response.usage.estimated);
}

#[test]
fn scripted_provider_replays_in_order_then_stops() {
    let dir = std::env::temp_dir().join(format!("agent-model-scripted-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("02.txt"), "second").unwrap();
    std::fs::write(dir.join("01.txt"), "first").unwrap();
    let mut config = ModelConfig::new(Provider::Scripted);
    config.responses = Some(dir.clone());
    let mut model = connect(&config).unwrap();
    assert_eq!(model.complete(&request()).unwrap().text, "first");
    assert_eq!(model.complete(&request()).unwrap().text, "second");
    assert!(model.complete(&request()).is_err());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn configuration_files_select_the_provider() {
    let local: ModelConfig = serde_json::from_value(json!({ "provider": "openai-compatible", "model": "qwen2.5-coder:14b", "baseUrl": "http://localhost:11434/v1" })).unwrap();
    let remote: ModelConfig = serde_json::from_value(
        json!({ "provider": "anthropic", "model": "claude-sonnet-5-5", "apiKeyEnv": "ANTHROPIC_API_KEY" }),
    )
    .unwrap();
    assert_eq!((local.provider, remote.provider), (Provider::OpenAiCompatible, Provider::Anthropic));
    assert!(
        serde_json::from_value::<ModelConfig>(json!({ "provider": "anthropic", "apiKey": "never in a file" })).is_err()
    );
    for name in ["openai-compatible", "anthropic", "command", "scripted"] {
        assert_eq!(serde_json::to_value(name.parse::<Provider>().unwrap()).unwrap(), name);
    }
}

#[test]
fn shipped_example_configurations_parse() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/models");
    let mut providers = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        providers.push(ModelConfig::from_file(&entry.unwrap().path()).unwrap().provider);
    }
    assert!(providers.contains(&Provider::OpenAiCompatible) && providers.contains(&Provider::Anthropic));
}
