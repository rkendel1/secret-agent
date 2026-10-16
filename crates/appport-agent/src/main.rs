//! `appport-agent` command line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent_core::generic::CodingContext;
use agent_core::plan::ContextMode;
use agent_model::{ModelConfig, Provider};
use appport_agent::arms::{arm_eval, arm_markdown, Arm, ArmEvalOptions};
use appport_agent::coding::{coding_eval, coding_markdown, CodingEvalOptions};
use appport_agent::eval::{eval, markdown, EvalOptions};
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
  appport-agent arm-eval --tasks <dir> [model options] --out <dir> [--trials <n>] [--arms A,B,C]
                     [--max-attempts <n>] [--max-turns <n>] [--budget-usd <usd>] [--root <dir>]
                     (bare vs context-assisted vs AppPort-native: see docs/three-arm-eval.md)
  appport-agent code-eval --tasks <dir> [model options] --out <dir> [--trials <n>]
                     [--arms observed,minimal] [--min-success <0..1>] [--require-test-runs]
                     (ordinary repositories, no AppPort: see docs/coding-agent-eval.md)
  appport-agent eval --project <dir> --intent <text> [model options] --out <dir>
                     [--trials <n>] [--arms knowledge,bare] [--max-attempts <n>]

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
  --context <mode>         what the model is told: knowledge (default) or bare (the ablation)
  --quiet                  no progress on stderr

EVAL OPTIONS
  --out <dir>              where trials, eval.json and eval.md go
  --trials <n>             trials per arm (default 5); each starts from a fresh copy of --project
  --arms <list>            comma-separated context modes (default knowledge,bare)

EXIT STATUS
  run: 0 accepted, 1 rejected, 2 error
  eval: 0 when the first arm accepted at least one trial, 1 otherwise, 2 error
  arm-eval: 0 unless a trial hit a harness error (the comparison itself never fails the run)
  code-eval: 0 when the first arm's success rate is at least --min-success (default 0)
             and, with --require-test-runs, every trial ran the test command; 1 otherwise";

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
        let value = if matches!(name.as_str(), "json" | "quiet" | "require-test-runs") {
            None
        } else {
            Some(args.next().ok_or_else(|| format!("--{name} needs a value"))?)
        };
        flags.push((name, value));
    }
    let get = |name: &str| flags.iter().rev().find(|(n, _)| n == name).and_then(|(_, v)| v.clone());
    let has = |name: &str| flags.iter().any(|(n, _)| n == name);
    for (name, _) in &flags {
        const KNOWN: [&str; 28] = [
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
            "context",
            "out",
            "trials",
            "arms",
            "tasks",
            "min-success",
            "test-timeout",
            "require-test-runs",
            "root",
            "max-turns",
            "budget-usd",
        ];
        if !KNOWN.contains(&name.as_str()) {
            return Err(format!("unknown option --{name}\n\n{USAGE}"));
        }
    }

    if command == "arm-eval" {
        return arm_eval_command(&get, has("quiet"));
    }
    if command == "code-eval" {
        return code_eval_command(&get, has("quiet"));
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
                context: ContextMode::Knowledge,
            };
            let mut log = Log::new(options.log.as_deref(), options.quiet).map_err(|e| e.to_string())?;
            let prepared = prepare(&options, &mut log)?;
            if has("json") {
                println!("{}", serde_json::to_string_pretty(&prepared.plan).expect("plan serializes"));
            }
            Ok(ExitCode::SUCCESS)
        }
        "run" | "eval" => {
            let model = model_config(&get)?;
            let max_attempts = get("max-attempts")
                .map(|n| n.parse().map_err(|_| "--max-attempts must be a number"))
                .transpose()?
                .unwrap_or(3);
            if command == "eval" {
                let arms = get("arms")
                    .unwrap_or_else(|| "knowledge,bare".into())
                    .split(',')
                    .map(|a| a.trim().parse::<ContextMode>())
                    .collect::<Result<Vec<_>, _>>()?;
                let first = *arms.first().ok_or("--arms is empty")?;
                let report = eval(&EvalOptions {
                    project,
                    intent,
                    model,
                    arms,
                    trials: get("trials")
                        .map(|n| n.parse().map_err(|_| "--trials must be a number"))
                        .transpose()?
                        .unwrap_or(5),
                    max_attempts,
                    host,
                    out: PathBuf::from(get("out").ok_or("--out is required")?),
                    quiet: has("quiet"),
                })?;
                println!("{}", markdown(&report));
                return Ok(if report.arm(first).is_some_and(|a| a.accepted > 0) {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                });
            }
            let options = RunOptions {
                project,
                intent,
                model,
                max_attempts,
                host,
                report: get("report").map(PathBuf::from),
                log: get("log").map(PathBuf::from),
                quiet: has("quiet"),
                context: get("context").map(|c| c.parse::<ContextMode>()).transpose()?.unwrap_or_default(),
            };
            let report = run(&options)?;
            Ok(if report.outcome == "accepted" { ExitCode::SUCCESS } else { ExitCode::from(1) })
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

/// The model configuration from `--config` and the model flags (flags win).
fn model_config(get: &dyn Fn(&str) -> Option<String>) -> Result<ModelConfig, String> {
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
    Ok(model)
}

/// `code-eval`: the coding-agent evaluation on ordinary repositories.
fn code_eval_command(get: &dyn Fn(&str) -> Option<String>, quiet: bool) -> Result<ExitCode, String> {
    let root = PathBuf::from(get("tasks").ok_or("--tasks is required")?);
    let mut tasks: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("task.json").is_file())
        .collect();
    if root.join("task.json").is_file() {
        tasks = vec![root.clone()];
    }
    tasks.sort();
    if tasks.is_empty() {
        return Err(format!("{} holds no task.json", root.display()));
    }
    let arms = get("arms")
        .unwrap_or_else(|| "observed,minimal".into())
        .split(',')
        .map(|a| a.trim().parse::<CodingContext>())
        .collect::<Result<Vec<_>, _>>()?;
    let first = *arms.first().ok_or("--arms is empty")?;
    let number = |name: &str, default: f64| -> Result<f64, String> {
        get(name)
            .map(|v| v.parse::<f64>().map_err(|_| format!("--{name} must be a number")))
            .transpose()
            .map(|v| v.unwrap_or(default))
    };
    let report = coding_eval(&CodingEvalOptions {
        tasks,
        model: model_config(get)?,
        arms,
        trials: number("trials", 5.0)? as u32,
        max_attempts: number("max-attempts", 3.0)? as u32,
        test_timeout: Duration::from_secs(number("test-timeout", 120.0)? as u64),
        out: PathBuf::from(get("out").ok_or("--out is required")?),
        quiet,
    })?;
    println!("{}", coding_markdown(&report));
    let min = number("min-success", 0.0)?;
    let arm = report.arm(first).ok_or("no trials ran")?;
    let mut ok = arm.success_rate >= min;
    if !ok {
        eprintln!("FAILED: {first} success rate {:.0}% is below {:.0}%", arm.success_rate * 100.0, min * 100.0);
    }
    if get("require-test-runs").is_some() || std::env::args().any(|a| a == "--require-test-runs") {
        let silent: Vec<String> = report
            .trials
            .iter()
            .filter(|t| t.test_runs == 0)
            .map(|t| format!("{} {} #{}", t.task, t.arm, t.trial))
            .collect();
        if !silent.is_empty() {
            eprintln!("FAILED: the test command never ran in {}", silent.join(", "));
            ok = false;
        }
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// `arm-eval`: bare vs context-assisted vs AppPort-native on the same tasks.
fn arm_eval_command(get: &dyn Fn(&str) -> Option<String>, quiet: bool) -> Result<ExitCode, String> {
    let root = PathBuf::from(get("tasks").ok_or("--tasks is required")?);
    let mut tasks: Vec<PathBuf> = std::fs::read_dir(&root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("task.json").is_file())
        .collect();
    if root.join("task.json").is_file() {
        tasks = vec![root.clone()];
    }
    tasks.sort();
    if tasks.is_empty() {
        return Err(format!("{} holds no task.json", root.display()));
    }
    let arms = get("arms")
        .unwrap_or_else(|| "A,B,C".into())
        .split(',')
        .map(|a| a.trim().parse::<Arm>())
        .collect::<Result<Vec<_>, _>>()?;
    let number = |name: &str, default: f64| -> Result<f64, String> {
        get(name)
            .map(|v| v.parse::<f64>().map_err(|_| format!("--{name} must be a number")))
            .transpose()
            .map(|v| v.unwrap_or(default))
    };
    let report = arm_eval(&ArmEvalOptions {
        root: get("root").map_or_else(|| PathBuf::from("."), PathBuf::from),
        tasks,
        model: model_config(get)?,
        arms,
        trials: number("trials", 5.0)? as u32,
        max_attempts: number("max-attempts", 3.0)? as u32,
        max_turns: number("max-turns", 12.0)? as u32,
        host: HostConfig {
            node: get("node").map_or_else(|| PathBuf::from("node"), PathBuf::from),
            timeout: Duration::from_secs(number("timeout", 20.0)? as u64),
        },
        out: PathBuf::from(get("out").ok_or("--out is required")?),
        quiet,
        budget_usd: get("budget-usd")
            .map(|v| v.parse::<f64>().map_err(|_| "--budget-usd must be a number"))
            .transpose()?,
    })?;
    println!("{}", arm_markdown(&report));
    let broken = report.trials.iter().filter(|t| t.category == "harness-error").count();
    if broken > 0 {
        eprintln!("FAILED: {broken} trial(s) hit a harness error");
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}
