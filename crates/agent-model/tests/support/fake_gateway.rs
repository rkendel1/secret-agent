//! A local stand-in for the Vercel AI Gateway: serves `GET /v1/models` and
//! `POST /v1/chat/completions` from a handler and records every request, so
//! tests can show what was (and was not) sent. No network, no inference.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

/// One request the stand-in received.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: Value,
}

/// What to answer: `(status, body text, delay)`.
pub type Handler = dyn Fn(&Seen) -> (u16, String, Duration) + Send + Sync;

/// What to answer, with extra response headers.
pub type HeaderHandler = dyn Fn(&Seen) -> (u16, String, Duration, Vec<(String, String)>) + Send + Sync;

pub struct FakeGateway {
    /// `http://127.0.0.1:<port>/v1`
    pub base_url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeGateway {
    pub fn start(handler: Box<Handler>) -> FakeGateway {
        FakeGateway::start_with_headers(Box::new(move |seen: &Seen| {
            let (status, text, delay) = handler(seen);
            (status, text, delay, Vec::new())
        }))
    }

    pub fn start_with_headers(handler: Box<HeaderHandler>) -> FakeGateway {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let handler: Arc<HeaderHandler> = Arc::from(handler);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                let handler = Arc::clone(&handler);
                thread::spawn(move || {
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        return;
                    }
                    let mut parts = request_line.split_whitespace();
                    let method = parts.next().unwrap_or_default().to_string();
                    let path = parts.next().unwrap_or_default().to_string();
                    let mut length = 0usize;
                    let mut authorization = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                            break;
                        }
                        let lower = line.to_lowercase();
                        if let Some(v) = lower.strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap_or(0);
                        }
                        if lower.starts_with("authorization:") {
                            authorization = Some(line["authorization:".len()..].trim().to_string());
                        }
                    }
                    let mut payload = vec![0; length];
                    let _ = reader.read_exact(&mut payload);
                    let seen = Seen {
                        method,
                        path,
                        authorization,
                        body: serde_json::from_slice(&payload).unwrap_or(Value::Null),
                    };
                    log.lock().unwrap().push(seen.clone());
                    let (status, text, delay, headers) = handler(&seen);
                    thread::sleep(delay);
                    let mut stream = stream;
                    let extra: String = headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n{extra}connection: close\r\n\r\n{text}",
                        text.len()
                    );
                });
            }
        });
        FakeGateway { base_url, seen }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub fn completions(&self) -> Vec<Seen> {
        self.seen().into_iter().filter(|s| s.path.ends_with("/chat/completions")).collect()
    }
}

/// A catalog listing `model` with the given pricing (or none).
pub fn catalog(model: &str, pricing: Option<Value>) -> String {
    let mut entry = json!({ "id": model, "object": "model", "owned_by": "test" });
    if let Some(p) = pricing {
        entry["pricing"] = p;
    }
    json!({ "object": "list", "data": [entry, { "id": "anthropic/claude-opus-5.5", "pricing": { "input": "0.000004", "output": "0.00002" } }] })
        .to_string()
}

/// A chat completion answering `text`.
pub fn completion(model: &str, text: &str) -> String {
    json!({
        "model": model,
        "choices": [{ "message": { "role": "assistant", "content": text }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2 }
    })
    .to_string()
}

/// A gateway serving `model` as free and answering every completion with `text`.
pub fn free_gateway(model: &'static str, text: &'static str) -> FakeGateway {
    FakeGateway::start(Box::new(move |seen: &Seen| {
        if seen.path.ends_with("/models") {
            (200, catalog(model, Some(json!({ "input": "0", "output": "0" }))), Duration::ZERO)
        } else {
            (200, completion(model, text), Duration::ZERO)
        }
    }))
}
