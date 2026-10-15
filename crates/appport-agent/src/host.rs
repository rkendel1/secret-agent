//! Runs an AppPort application from disk and talks to it in AppPort envelopes.
//!
//! The application is hosted by `host/appport-ipc-host.mjs` behind
//! `AppPortConnection` from `@appport/server`, resolved from the project's own
//! dependencies. One envelope per line on stdin/stdout: requests, responses,
//! subscriptions and events, exactly as the protocol defines them. Every request
//! goes through the runtime's pipeline, under a session holding only the
//! permissions the plan derived from the contract.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_core::appport::knowledge::{constants::PROTOCOL_ID, rules::sha256_hex, CapabilityId};
use agent_core::observe::ApplicationEntry;
use agent_core::verify::Reply;
use serde_json::{json, Value};

const HOST_SCRIPT: &str = include_str!("../host/appport-ipc-host.mjs");

/// How to run applications.
#[derive(Debug, Clone)]
pub struct HostConfig {
    /// The Node.js executable.
    pub node: PathBuf,
    /// Longest wait for one response.
    pub timeout: Duration,
}

impl Default for HostConfig {
    fn default() -> Self {
        HostConfig { node: PathBuf::from("node"), timeout: Duration::from_secs(20) }
    }
}

enum Line {
    Envelope(Value),
    Closed,
}

/// A running application.
pub struct Host {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Line>,
    stderr: Arc<Mutex<String>>,
    events: Vec<Value>,
    next: u64,
    timeout: Duration,
    /// Envelopes sent, for metrics.
    pub sent: usize,
}

fn script_path() -> std::io::Result<PathBuf> {
    let digest = sha256_hex(HOST_SCRIPT);
    let path = std::env::temp_dir().join(format!("appport-ipc-host-{}.mjs", &digest[..16]));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(HOST_SCRIPT) {
        std::fs::write(&path, HOST_SCRIPT)?;
    }
    Ok(path)
}

impl Host {
    /// Starts the application exported by `entry` in the project at `root`.
    pub fn start(
        config: &HostConfig,
        root: &Path,
        entry: &ApplicationEntry,
        permissions: &[String],
    ) -> Result<Host, String> {
        let script = script_path().map_err(|e| format!("cannot write the IPC host: {e}"))?;
        let mut child = Command::new(&config.node)
            .arg(script)
            .arg(root)
            .arg(&entry.module)
            .arg(&entry.export)
            .arg(serde_json::to_string(permissions).expect("strings serialize"))
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", config.node.display()))?;

        let (tx, lines) = channel();
        let stdout = child.stdout.take().expect("piped");
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if tx.send(Line::Envelope(value)).is_err() {
                        return;
                    }
                }
            }
            let _ = tx.send(Line::Closed);
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&stderr);
        let mut pipe = child.stderr.take().expect("piped");
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                let mut text = sink.lock().expect("not poisoned");
                text.push_str(&String::from_utf8_lossy(&buffer[..n]));
                if text.len() > 16 * 1024 {
                    let cut = text.len() - 8 * 1024;
                    let cut = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(cut);
                    text.drain(..cut);
                }
            }
        });
        let stdin = child.stdin.take();
        Ok(Host { child, stdin, lines, stderr, events: Vec::new(), next: 0, timeout: config.timeout, sent: 0 })
    }

    fn stderr_tail(&self) -> String {
        let text = self.stderr.lock().expect("not poisoned").clone();
        let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        lines[lines.len().saturating_sub(6)..].join(" | ")
    }

    fn send(&mut self, envelope: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("the application's input is closed")?;
        let mut line = serde_json::to_string(envelope).expect("JSON serializes");
        line.push('\n');
        self.sent += 1;
        stdin
            .write_all(line.as_bytes())
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("the application stopped reading ({e}): {}", self.stderr_tail()))
    }

    fn await_response(&mut self, request_id: &str) -> Result<Value, String> {
        let deadline = std::time::Instant::now() + self.timeout;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match self.lines.recv_timeout(remaining) {
                Ok(Line::Envelope(envelope)) => match envelope.get("type").and_then(Value::as_str) {
                    Some("response") if envelope.get("requestId").and_then(Value::as_str) == Some(request_id) => {
                        return Ok(envelope)
                    }
                    Some("event") => self.events.push(envelope),
                    _ => {}
                },
                Ok(Line::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    let _ = self.child.wait();
                    return Err(format!("the application exited: {}", self.stderr_tail()));
                }
                Err(RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return Err(format!("no response to {request_id} within {}s", self.timeout.as_secs()));
                }
            }
        }
    }

    fn request_id(&mut self) -> String {
        self.next += 1;
        format!("agent_{}", self.next)
    }

    /// Invokes a request capability. Returns the reply and the request id.
    pub fn request(&mut self, capability: &CapabilityId, input: &Value) -> Result<(Reply, String), String> {
        let request_id = self.request_id();
        self.send(&json!({
            "protocol": PROTOCOL_ID,
            "type": "request",
            "requestId": request_id,
            "capability": { "name": capability.name, "version": capability.version },
            "input": input
        }))?;
        let response = self.await_response(&request_id)?;
        let reply = if response.get("ok") == Some(&Value::Bool(true)) {
            Reply::Ok { output: response.get("output").cloned().unwrap_or(Value::Null) }
        } else {
            Reply::Err {
                code: response.pointer("/error/code").and_then(Value::as_str).unwrap_or("INVALID_PROTOCOL").to_string(),
                message: response.pointer("/error/message").and_then(Value::as_str).unwrap_or_default().to_string(),
            }
        };
        Ok((reply, request_id))
    }

    /// Subscribes to an event. Delivered events are kept until [`Host::events_named`].
    pub fn subscribe(&mut self, event: &str) -> Result<(), String> {
        let request_id = self.request_id();
        self.send(&json!({
            "protocol": PROTOCOL_ID,
            "type": "subscribe",
            "requestId": request_id,
            "subscriptionId": format!("sub_{event}"),
            "event": { "name": event }
        }))?;
        let response = self.await_response(&request_id)?;
        match response.get("ok") {
            Some(Value::Bool(true)) => Ok(()),
            _ => Err(format!(
                "subscription to {event} refused: {}",
                response.pointer("/error/code").unwrap_or(&Value::Null)
            )),
        }
    }

    /// Payloads of delivered events named `name`, after a round trip that flushes
    /// anything published before it.
    pub fn events_named(&mut self, name: &str) -> Result<Vec<Value>, String> {
        let ping = CapabilityId { name: "appport.ping".into(), version: 1 };
        self.request(&ping, &json!({}))?;
        Ok(self
            .events
            .iter()
            .filter(|e| e.pointer("/event/name").and_then(Value::as_str) == Some(name))
            .filter_map(|e| e.get("payload").cloned())
            .collect())
    }

    /// Closes the channel and waits for the application to exit.
    pub fn close(mut self) {
        drop(self.stdin.take());
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
