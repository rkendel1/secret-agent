use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ModelError;

/// Which provider protocol to speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
    Anthropic,
    Command,
    Scripted,
}

impl std::str::FromStr for Provider {
    type Err = ModelError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).map_err(|_| {
            ModelError::Config(format!("unknown provider {s:?}: use openai-compatible, anthropic, command or scripted"))
        })
    }
}

/// Model configuration, from a JSON file and/or command-line flags. Secrets are
/// never stored here: `api_key_env` names the environment variable holding the key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelConfig {
    pub provider: Provider,
    /// Model name, e.g. `qwen2.5-coder:14b` or `claude-sonnet-5-5`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Defaults: `http://localhost:11434/v1` (openai-compatible), `https://api.anthropic.com` (anthropic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Environment variable with the API key. Defaults: `OPENAI_API_KEY` (optional
    /// for local servers), `ANTHROPIC_API_KEY`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    /// For `command`: the program and its arguments.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
    /// For `scripted`: the directory of completions (`01.txt`, `02.txt`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub responses: Option<PathBuf>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Sampling temperature, for providers and models that accept it (current
    /// Claude models reject it; control them with `effort`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    /// For `anthropic`: `output_config.effort` (`low` … `max`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// For `anthropic`: environment variable holding the workspace id, for keys
    /// not scoped to one workspace (sent as `anthropic-workspace-id`). Default
    /// `ANTHROPIC_WORKSPACE_ID`; nothing is sent when it is unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id_env: Option<String>,
}

fn default_max_tokens() -> u32 {
    8192
}

fn default_timeout() -> u64 {
    300
}

impl ModelConfig {
    pub fn new(provider: Provider) -> Self {
        ModelConfig {
            provider,
            model: None,
            base_url: None,
            api_key_env: None,
            command: Vec::new(),
            responses: None,
            max_tokens: default_max_tokens(),
            timeout_secs: default_timeout(),
            temperature: None,
            effort: None,
            workspace_id_env: None,
        }
    }

    /// Reads a JSON configuration file.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ModelError> {
        let text = std::fs::read_to_string(path).map_err(|e| ModelError::Config(format!("{}: {e}", path.display())))?;
        serde_json::from_str(&text).map_err(|e| ModelError::Config(format!("{}: {e}", path.display())))
    }
}
