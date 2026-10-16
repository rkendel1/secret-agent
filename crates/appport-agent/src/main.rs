//! `appport-agent` command line.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use agent_core::plan::ContextMode;
use agent_model::gateway::{preflight, preflight_text, GatewayPolicy};
use agent_model::{ModelConfig, Provider};
use appport_agent::arms::{arm_eval, arm_markdown, Arm, ArmEvalOptions, SMOKE_TASKS};
use appport_agent::coding::{coding_eval, coding_markdown, CodingArm, CodingEvalOptions, Limits, Strategies};
use appport_agent::eval::{eval, markdown, EvalOptions};
use appport_agent::experiment::{read_spec, resolve};
use appport_agent::goal::read_goal_check;
use appport_agent::host::HostConfig;
use appport_agent::report::Log;
use appport_agent::run::{prepare, run, RunOptions};
use appport_agent::strategy::{ContextStrategyId, RecoveryPolicyId, DEFAULT_CONTEXT_BUDGET};

const USAGE: &str = "\
appport-agent: observe an AppPort project, plan a bounded change, ask a model only
for the implementation, and accept the result only if the running application
satisfies its contract.

USAGE
  appport-agent run  --project <dir> --intent <text> [model options] [run options]
  appport-agent plan --project <dir> --intent <text> [--json]
  appport-agent arm-eval --tasks <dir> [model options] --out <dir> [--trials <n>] [--arms A,B,C]
                     [--mode comparison|smoke|preflight] [--free-gateway]
                     [--max-attempts <n>] [--max-turns <n>] [--budget-usd <usd>]
                     [--max-model-calls <n>] [--max-minutes <m>] [--root <dir>]
                     (bare vs context-assisted vs AppPort-native: see docs/three-arm-eval.md)
  appport-agent preflight [model options] [--out <dir>]
                     (checks a free Vercel AI Gateway model before any live run)
  appport-agent code-eval --tasks <dir> [model options] --out <dir> [--trials <n>]
                     [--arms observed,minimal] [--recovery <policy>] [--context-budget <bytes>]
                     [--min-success <0..1>] [--require-test-runs] [--free-gateway]
                     [--max-model-calls <n>] [--max-minutes <m>] [--budget-usd <usd>]
                     (ordinary repositories, no AppPort: see docs/coding-agent-eval.md)
  appport-agent experiment --spec <experiment.json> [--validate] [--out <dir>] [--trials <n>] [--model <id>]
  appport-agent experiment --show <out dir>
                     (strategy comparisons and ablations: see docs/agent-research/experiments.md)
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
  --goal-check <file.json> a check of the goal itself (see docs/agent-loop.md): a change is
                           accepted only if it fails before the change and passes after it
  --accept-preserving      accept a verified, contract-preserving change even when nothing
                           shows the goal was achieved (by default such a run is declined)
  --allow-dirty            rewrite files that have uncommitted changes (refused by default)
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

FREE GATEWAY (arm-eval --free-gateway, preflight)
  The configuration must be the openai-compatible provider at https://ai-gateway.vercel.sh/v1,
  the model must be on the free allowlist and priced at zero by the gateway catalog, and one
  minimal completion must succeed, before any trial. Otherwise no trial runs and the exit
  status is 1. The report goes to preflight.json and preflight.txt in --out.
  --mode smoke             replace-lookup and contract-change, every arm, one trial
  --mode preflight         the preflight only (implies --free-gateway)
  --mode comparison        every task in --tasks, --trials trials (the default)

STRATEGIES (code-eval arms, experiment specs)
  context    minimal | observed (baseline) | dependency-aware | appport-aware
  recovery   feedback (baseline) | expand-context | stop-on-repeat
  The planner's edit boundary and the project's test command are the same for every
  strategy: a strategy chooses what the model sees and what follows a rejection, nothing else.

EXIT STATUS
  run: 0 accepted (verified, with goal evidence unless --accept-preserving)
       1 rejected (no attempt passed; every attempt was undone and checked)
       2 usage or harness error
       3 limitation (declined before changing anything: no plan, no goal evidence possible,
         uncommitted changes, ...)
       4 the model provider failed (nothing is left applied)
       5 a rejected change could not be undone and checked: inspect the project (see the report)
  eval: 0 when the first arm accepted at least one trial, 1 otherwise, 2 error
  arm-eval: 1 when the preflight failed, when a trial hit a harness error, when no model call
            succeeded, or when more than 10% of trials hit provider errors; otherwise 0 (which
            arm wins never fails the run)
  preflight: 0 when the model may be used, 1 otherwise
  code-eval: 0 when the first arm's success rate is at least --min-success (default 0)
             and, with --require-test-runs, every trial ran the test command; 1 otherwise,
             and 1 when the preflight failed, a fixture changed, or every trial hit a
             provider or infrastructure failure
  experiment: 0 when the run is a valid measurement (whichever arm wins); 1 when the
             preflight failed, a fixture changed, or nothing could be measured";

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
        let value = if matches!(
            name.as_str(),
            "json" | "quiet" | "require-test-runs" | "free-gateway" | "accept-preserving" | "allow-dirty" | "validate"
        ) {
            None
        } else {
            Some(args.next().ok_or_else(|| format!("--{name} needs a value"))?)
        };
        flags.push((name, value));
    }
    let get = |name: &str| flags.iter().rev().find(|(n, _)| n == name).and_then(|(_, v)| v.clone());
    let has = |name: &str| flags.iter().any(|(n, _)| n == name);
    for (name, _) in &flags {
        const KNOWN: [&str; 41] = [
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
            "mode",
            "free-gateway",
            "max-model-calls",
            "max-minutes",
            "goal-check",
            "accept-preserving",
            "allow-dirty",
            "spec",
            "validate",
            "show",
            "recovery",
            "context-budget",
            "experiment-id",
        ];
        if !KNOWN.contains(&name.as_str()) {
            return Err(format!("unknown option --{name}\n\n{USAGE}"));
        }
    }

    if command == "arm-eval" {
        return arm_eval_command(&get, has("quiet"), has("free-gateway"));
    }
    if command == "preflight" {
        return preflight_command(&get);
    }
    if command == "code-eval" {
        return code_eval_command(&get, has("quiet"));
    }
    if command == "experiment" {
        return experiment_command(&get, has("quiet"), has("validate"));
    }
    let project = PathBuf::from(get("project").ok_or("--project is required")?);
    let intent = get("intent").ok_or("--intent is required")?;
    let host = HostConfig {
        node: get("node").map_or_else(|| PathBuf::from("node"), PathBuf::from),
        timeout: Duration::from_secs(
            get("timeout").map(|t| t.parse().map_err(|_| "--timeout must be seconds")).transpose()?.unwrap_or(20),
        ),
        ..HostConfig::default()
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
                goal_check: None,
                require_goal_evidence: false,
                allow_dirty: true,
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
                goal_check: get("goal-check").map(|p| read_goal_check(p.as_ref())).transpose()?,
                require_goal_evidence: !has("accept-preserving"),
                allow_dirty: has("allow-dirty"),
            };
            let report = run(&options)?;
            if report.outcome != "accepted" {
                eprintln!(
                    "appport-agent: {}{}",
                    report.outcome,
                    report.reason.as_ref().map(|r| format!(": {r}")).unwrap_or_default()
                );
            }
            Ok(ExitCode::from(run_exit_code(&report.outcome)))
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

/// The exit status of `run` for an outcome (see USAGE).
fn run_exit_code(outcome: &str) -> u8 {
    match outcome {
        "accepted" => 0,
        "rejected" => 1,
        "limitation" => 3,
        "provider-error" => 4,
        "rollback-failed" | "interrupted" => 5,
        _ => 2,
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
    let number = |name: &str, default: f64| -> Result<f64, String> {
        get(name)
            .map(|v| v.parse::<f64>().map_err(|_| format!("--{name} must be a number")))
            .transpose()
            .map(|v| v.unwrap_or(default))
    };
    // Each arm is a context strategy; --recovery and --context-budget apply to every arm.
    let recovery: RecoveryPolicyId = get("recovery").map(|r| r.parse()).transpose()?.unwrap_or_default();
    let budget = number("context-budget", DEFAULT_CONTEXT_BUDGET as f64)? as usize;
    let arms = get("arms")
        .unwrap_or_else(|| "observed,minimal".into())
        .split(',')
        .map(|a| {
            let context: ContextStrategyId = a.trim().parse()?;
            Ok(CodingArm {
                id: context.to_string(),
                strategies: Strategies { context, recovery, context_budget: budget },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let first = arms.first().ok_or("--arms is empty")?.id.clone();
    let mut options =
        CodingEvalOptions::new(tasks, model_config(get)?, arms, PathBuf::from(get("out").ok_or("--out is required")?));
    options.trials = number("trials", 5.0)? as u32;
    options.max_attempts = number("max-attempts", 3.0)? as u32;
    options.test_timeout = Duration::from_secs(number("test-timeout", 120.0)? as u64);
    options.quiet = quiet;
    options.limits = Limits {
        max_model_calls: get("max-model-calls")
            .map(|v| v.parse().map_err(|_| "--max-model-calls must be a whole number"))
            .transpose()?,
        max_minutes: get("max-minutes").map(|v| v.parse().map_err(|_| "--max-minutes must be a number")).transpose()?,
        budget_usd: get("budget-usd").map(|v| v.parse().map_err(|_| "--budget-usd must be a number")).transpose()?,
    };
    if std::env::args().any(|a| a == "--free-gateway") {
        options.free_gateway = Some(GatewayPolicy::default());
    }
    let report = coding_eval(&options)?;
    println!("{}", coding_markdown(&report));
    if let Some(problem) = eval_problem(&report) {
        eprintln!("FAILED: {problem}");
        return Ok(ExitCode::from(1));
    }
    let min = number("min-success", 0.0)?;
    let arm = report.arm_named(&first).ok_or("no trials ran")?;
    let mut ok = arm.success_rate >= min;
    if !ok {
        eprintln!("FAILED: {first} success rate {:.0}% is below {:.0}%", arm.success_rate * 100.0, min * 100.0);
    }
    if std::env::args().any(|a| a == "--require-test-runs") {
        let silent: Vec<String> = report
            .trials
            .iter()
            .filter(|t| t.test_runs == 0 && t.category != "not-run")
            .map(|t| format!("{} {} #{}", t.task, t.arm, t.trial))
            .collect();
        if !silent.is_empty() {
            eprintln!("FAILED: the test command never ran in {}", silent.join(", "));
            ok = false;
        }
    }
    Ok(if ok { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// Why a coding evaluation cannot be trusted as a measurement, if it cannot.
fn eval_problem(report: &appport_agent::coding::CodingEvalReport) -> Option<String> {
    if report.status == "preflight-failed" {
        return Some("the free-gateway preflight failed; no trial ran".into());
    }
    if !report.fixtures_unchanged {
        return Some("a task fixture changed during the run; the trials did not see identical inputs".into());
    }
    let ran: Vec<_> = report.trials.iter().filter(|t| t.category != "not-run").collect();
    let failed =
        ran.iter().filter(|t| matches!(t.category.as_str(), "provider-error" | "infrastructure-error")).count();
    if !ran.is_empty() && failed == ran.len() {
        return Some(format!("every trial ({failed}) hit a provider or infrastructure failure; nothing was measured"));
    }
    None
}

/// `experiment`: runs, checks or shows an experiment specification.
fn experiment_command(get: &dyn Fn(&str) -> Option<String>, quiet: bool, validate: bool) -> Result<ExitCode, String> {
    if let Some(dir) = get("show") {
        let path = PathBuf::from(dir).join("coding-eval.md");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        print!("{text}");
        return Ok(ExitCode::SUCCESS);
    }
    let spec_path = PathBuf::from(get("spec").ok_or("--spec <file.json> (or --show <dir>) is required")?);
    let spec = read_spec(&spec_path)?;
    let root =
        get("root").map_or_else(|| PathBuf::from("."), PathBuf::from).canonicalize().map_err(|e| e.to_string())?;
    let mut resolved = resolve(&spec, &root)?;
    if let Some(out) = get("out") {
        resolved.options.out = PathBuf::from(out);
    }
    // The model id may be overridden (e.g. by the workflow input); the preflight still decides.
    if let Some(model) = get("model") {
        resolved.options.model.model = Some(model);
        resolved.record["resolvedModel"] = serde_json::to_value(&resolved.options.model).expect("serializes");
        resolved.options.spec = Some(resolved.record.clone());
    }
    if let Some(trials) = get("trials") {
        resolved.options.trials = trials.parse().map_err(|_| "--trials must be a number")?;
        resolved.record["trials"] = resolved.options.trials.into();
        resolved.options.spec = Some(resolved.record.clone());
    }
    resolved.options.quiet = quiet;
    let o = &resolved.options;
    let planned = o.tasks.len() * o.arms.len() * o.trials as usize;
    eprintln!(
        "experiment {}: {} task(s) × {} arm(s) × {} trial(s) = {planned} trials, at most {} model calls; baseline `{}`; out {}",
        o.experiment,
        o.tasks.len(),
        o.arms.len(),
        o.trials,
        planned * o.max_attempts as usize,
        o.baseline.as_deref().unwrap_or_default(),
        o.out.display()
    );
    if validate {
        println!("{}", serde_json::to_string_pretty(&resolved.record).expect("serializes"));
        return Ok(ExitCode::SUCCESS);
    }
    std::fs::create_dir_all(&o.out).map_err(|e| format!("{}: {e}", o.out.display()))?;
    std::fs::write(
        o.out.join("experiment.json"),
        serde_json::to_string_pretty(&resolved.record).expect("serializes") + "\n",
    )
    .map_err(|e| e.to_string())?;
    let report = coding_eval(o)?;
    println!("{}", coding_markdown(&report));
    match eval_problem(&report) {
        Some(problem) => {
            eprintln!("FAILED: {problem}");
            Ok(ExitCode::from(1))
        }
        None => Ok(ExitCode::SUCCESS),
    }
}

/// `preflight`: may this configuration be used for a free live run?
fn preflight_command(get: &dyn Fn(&str) -> Option<String>) -> Result<ExitCode, String> {
    let model = model_config(get)?;
    let report = preflight(&model, &GatewayPolicy::default());
    let text = preflight_text(&report);
    print!("{text}");
    if let Some(out) = get("out").map(PathBuf::from) {
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        std::fs::write(out.join("preflight.json"), serde_json::to_string_pretty(&report).expect("serializes") + "\n")
            .map_err(|e| e.to_string())?;
        std::fs::write(out.join("preflight.txt"), &text).map_err(|e| e.to_string())?;
    }
    Ok(if report.proceed { ExitCode::SUCCESS } else { ExitCode::from(1) })
}

/// `arm-eval`: bare vs context-assisted vs AppPort-native on the same tasks.
fn arm_eval_command(get: &dyn Fn(&str) -> Option<String>, quiet: bool, free_gateway: bool) -> Result<ExitCode, String> {
    let mode = get("mode").unwrap_or_else(|| "comparison".into());
    if !matches!(mode.as_str(), "comparison" | "smoke" | "preflight") {
        return Err(format!("unknown --mode {mode:?}: use comparison, smoke or preflight"));
    }
    if mode == "preflight" {
        return preflight_command(get);
    }
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
    if mode == "smoke" {
        tasks.retain(|t| t.file_name().is_some_and(|n| SMOKE_TASKS.contains(&n.to_string_lossy().as_ref())));
    }
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
        trials: if mode == "smoke" { 1 } else { number("trials", 5.0)? as u32 },
        max_attempts: number("max-attempts", 3.0)? as u32,
        max_turns: number("max-turns", 12.0)? as u32,
        host: HostConfig {
            node: get("node").map_or_else(|| PathBuf::from("node"), PathBuf::from),
            timeout: Duration::from_secs(number("timeout", 20.0)? as u64),
            ..HostConfig::default()
        },
        out: PathBuf::from(get("out").ok_or("--out is required")?),
        quiet,
        budget_usd: get("budget-usd")
            .map(|v| v.parse::<f64>().map_err(|_| "--budget-usd must be a number"))
            .transpose()?,
        max_model_calls: get("max-model-calls")
            .map(|v| v.parse::<usize>().map_err(|_| "--max-model-calls must be a whole number"))
            .transpose()?,
        max_minutes: get("max-minutes")
            .map(|v| v.parse::<f64>().map_err(|_| "--max-minutes must be a number"))
            .transpose()?,
        free_gateway: free_gateway.then(GatewayPolicy::default),
    })?;
    println!("{}", arm_markdown(&report));
    match appport_agent::arms::run_problem(&report) {
        Some(problem) => {
            eprintln!("FAILED: {problem}");
            Ok(ExitCode::from(1))
        }
        None => Ok(ExitCode::SUCCESS),
    }
}
