//! `agent-model`: the agent's only way to reach a model.
//!
//! The model is a pluggable component that writes code. It is never asked what a
//! project contains or whether a change is correct; the deterministic core in
//! `agent-core` decides both. Everything a provider needs is in a [`Request`],
//! and everything the agent takes back is the text and the token usage in a
//! [`Response`].
//!
//! Providers are chosen by [`ModelConfig`] alone, so moving between a local LLM
//! and a remote API changes configuration, not code:
//!
//! | provider | reaches | typical use |
//! |---|---|---|
//! | `openai-compatible` | `POST {base_url}/chat/completions` | local LLMs (Ollama, llama.cpp `llama-server`, vLLM, LM Studio) and OpenAI-compatible remote APIs |
//! | `anthropic` | `POST {base_url}/v1/messages` | the Anthropic API |
//! | `command` | a local program: prompt on stdin, completion on stdout | any CLI model runner |
//! | `scripted` | numbered files in a directory, in order | reproducible demos and CI; replays pre-written completions |

mod config;
pub mod gateway;
mod providers;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use config::{ModelConfig, Provider};
pub use providers::{connect, AnthropicModel, CommandModel, OpenAiCompatibleModel, ScriptedModel};

/// What the model is given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    pub system: String,
    /// Earlier turns of the conversation, oldest first; empty for a single call.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<Turn>,
    /// The new user message.
    pub prompt: String,
    pub max_tokens: u32,
}

impl Request {
    /// A single-turn request.
    pub fn new(system: impl Into<String>, prompt: impl Into<String>, max_tokens: u32) -> Request {
        Request { system: system.into(), history: Vec::new(), prompt: prompt.into(), max_tokens }
    }
}

/// Who spoke a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// One earlier turn. `raw` is the provider's own content for an assistant turn
/// (for the Anthropic API, its content blocks, thinking included), replayed
/// unchanged so the conversation is never edited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub role: Role,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
}

/// Token usage for one call.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// True when the provider reported no usage and it was estimated (≈ 4 bytes per token).
    pub estimated: bool,
}

impl Usage {
    /// The estimate used when a provider reports nothing.
    pub fn estimate(input: &str, output: &str) -> Usage {
        Usage {
            input_tokens: input.len().div_ceil(4) as u64,
            output_tokens: output.len().div_ceil(4) as u64,
            estimated: true,
        }
    }
}

/// What the model returned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    pub text: String,
    pub usage: Usage,
    /// The model that answered, as the provider reports it.
    pub model: String,
    /// Why generation stopped, when the provider says (`end_turn`, `max_tokens`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// The provider's own content for this turn, to replay in a later [`Turn`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<serde_json::Value>,
    /// Requests retried before this answer (rate limits, server errors).
    #[serde(default)]
    pub retries: u32,
}

/// Why a model call failed.
#[derive(Debug)]
pub enum ModelError {
    Config(String),
    Transport(String),
    Provider { status: u16, body: String },
    Protocol(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModelError::Config(m) => write!(f, "model configuration: {m}"),
            ModelError::Transport(m) => write!(f, "model transport: {m}"),
            ModelError::Provider { status, body } => write!(f, "model provider answered {status}: {body}"),
            ModelError::Protocol(m) => write!(f, "unexpected model response: {m}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// A model. Implementations hold their own connection state.
pub trait Model {
    /// `provider:model`, for logs. Never includes credentials.
    fn describe(&self) -> String;
    fn complete(&mut self, request: &Request) -> Result<Response, ModelError>;
}
