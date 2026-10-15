//! `appport-agent` command line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent_model::{ModelConfig, Provider};
use appport_agent::host::HostConfig;
use appport_agent::report::Log;
use appport_agent::run::{prepare, run, RunOptions};

const USAGE: &str = "\
appport-agent: observe an AppPort project, plan a bounded change, ask a model only
for the implementation, and accept the result only if the running application
satisfies its contract.

USAGE
  appport-agent run  --project <dir> --intent <text> [model options] [run options]
  appport-agent plan --project <dir> --intent <text> [--json]

INTENT
  \"replace <capability>@<version>[: <goal>]\"

MODEL OPTIONS (a --config file and flags combine; flags win)
  --config <file.json>     {\"provider\": ..., \"model\": ..., \"baseUrl\": ..., ...}
  --provider <p>           openai-compatible | anthropic | command | scripted
  --model <name>           model name
  --base-url <url>         provider endpoint (openai-compatible default: http://localhost:11434/v1)
  --api-key-env <NAME>     environment variable holding the API key
  --command <cmd>          for `command`: program and arguments, space-separated
  --responses <dir>        for `scripted`: directory of 01.txt, 02.txt, …
  --max-tokens <n>         completion budget per call (default 8192)

RUN OPTIONS
  --max-attempts <n>       model attempts before giving up (default 3)
  --report <file.json>     write the run report
  --log <file.jsonl>       write the step-by-step run log
  --node <path>            Node.js executable (default: node)
  --timeout <secs>         longest wait for one application response (default 20)
  --quiet                  no progress on stderr

EXIT STATUS
  0 accepted, 1 rejected, 2 error";

fn main() -> ExitCode {
    match main_inner() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("appport-agent: {message}");
            ExitCode::from(2)
        }
    }
}

fn main_inner() -> Result<ExitCode, String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    if matches!(command.as_str(), "" | "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(ExitCode::SUCCESS);
    }
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    while let Some(flag) = args.next() {
        let name = flag.strip_prefix("--").ok_or_else(|| format!("unexpected argument {flag:?}"))?.to_string();
        let value = if matches!(name.as_str(), "json" | "quiet") {
            None
        } else {
            Some(args.next().ok_or_else(|| format!("--{name} needs a value"))?)
        };
        flags.push((name, value));
    }
    let get = |name: &str| flags.iter().rev().find(|(n, _)| n == name).and_then(|(_, v)| v.clone());
    let has = |name: &str| flags.iter().any(|(n, _)| n == name);
    for (name, _) in &flags {
        const KNOWN: [&str; 17] = [
            "project",
            "intent",
            "config",
            "provider",
            "model",
            "base-url",
            "api-key-env",
            "command",
            "responses",
            "max-tokens",
            "max-attempts",
            "report",
            "log",
            "node",
            "timeout",
            "quiet",
            "json",
        ];
        if !KNOWN.contains(&name.as_str()) {
            return Err(format!("unknown option --{name}\n\n{USAGE}"));
        }
    }

    let project = PathBuf::from(get("project").ok_or("--project is required")?);
    let intent = get("intent").ok_or("--intent is required")?;
    let host = HostConfig {
        node: get("node").map_or_else(|| PathBuf::from("node"), PathBuf::from),
        timeout: Duration::from_secs(
            get("timeout").map(|t| t.parse().map_err(|_| "--timeout must be seconds")).transpose()?.unwrap_or(20),
        ),
    };

    match command.as_str() {
        "plan" => {
            let options = RunOptions {
                project,
                intent,
                model: ModelConfig::new(Provider::Scripted),
                max_attempts: 0,
                host,
                report: None,
                log: get("log").map(PathBuf::from),
                quiet: has("json") || has("quiet"),
            };
            let mut log = Log::new(options.log.as_deref(), options.quiet).map_err(|e| e.to_string())?;
            let prepared = prepare(&options, &mut log)?;
            if has("json") {
                println!("{}", serde_json::to_string_pretty(&prepared.plan).expect("plan serializes"));
            }
            Ok(ExitCode::SUCCESS)
        }
        "run" => {
            let mut model = match get("config") {
                Some(path) => ModelConfig::from_file(path.as_ref()).map_err(|e| e.to_string())?,
                None => ModelConfig::new(
                    get("provider")
                        .ok_or("--provider (or --config) is required")?
                        .parse()
                        .map_err(|e: agent_model::ModelError| e.to_string())?,
                ),
            };
            if let Some(p) = get("provider") {
                model.provider = p.parse().map_err(|e: agent_model::ModelError| e.to_string())?;
            }
            if let Some(v) = get("model") {
                model.model = Some(v);
            }
            if let Some(v) = get("base-url") {
                model.base_url = Some(v);
            }
            if let Some(v) = get("api-key-env") {
                model.api_key_env = Some(v);
            }
            if let Some(v) = get("command") {
                model.command = v.split_whitespace().map(str::to_string).collect();
            }
            if let Some(v) = get("responses") {
                model.responses = Some(PathBuf::from(v));
            }
            if let Some(v) = get("max-tokens") {
                model.max_tokens = v.parse().map_err(|_| "--max-tokens must be a number")?;
            }
            let options = RunOptions {
                project,
                intent,
                model,
                max_attempts: get("max-attempts")
                    .map(|n| n.parse().map_err(|_| "--max-attempts must be a number"))
                    .transpose()?
                    .unwrap_or(3),
                host,
                report: get("report").map(PathBuf::from),
                log: get("log").map(PathBuf::from),
                quiet: has("quiet"),
            };
            let report = run(&options)?;
            Ok(if report.outcome == "accepted" { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}
