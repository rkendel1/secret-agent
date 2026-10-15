# appport-agent

A Rust coding agent built around one separation: the model (a local LLM or any
remote provider) is a pluggable reasoning component; structure, knowledge and
authority live in the agent. The agent observes reality instead of asking the
model to invent it.

## Layout

| Path | What |
|---|---|
| `crates/agent-core` | Deterministic core. `appport::knowledge` is AppPort's contracts, capabilities, integrations, graphs, verification rules and replacement constraints, derived from the published `@appport/*` packages. `observe`, `plan`, `conformance` and `verify` turn it into decisions about a real project. No model calls, no provider code, no Node at runtime. |
| `crates/agent-model` | The only way to a model: one `Model` trait; local LLMs (OpenAI-compatible), Anthropic, a command, or scripted completions, chosen by configuration. |
| `crates/appport-agent` | The `appport-agent` CLI: observe → plan → model → apply → run the application → judge; metrics and a step-by-step run log. |
| `examples/customers-app` | A small application built with `@appport/core`, used by the demo and the end-to-end tests. |
| `scripts/demo.sh` | Reproducible demo of the loop; results committed in `docs/demo/`. |
| `docs/agent-loop.md` | How the loop decides, what it checks, how to choose a model, and the demo's metrics. |
| `tools/appport-inspect` | Build-time inspection of `@appport/*`: writes the surface snapshot and fixtures the Rust model is checked against. |
| `scripts/check-appport-drift.sh` | Drift check (`--pinned` / `--latest`). |
| `docs/appport-knowledge.md` | How the knowledge is derived, how drift is caught, and the TypeScript → Rust mapping. |

## Use

```rust
use agent_core::appport::knowledge::{model_json, Contract};

let contract = Contract::from_json(&manifest_json)?;
let issues = contract.validate();                         // deterministic, model-free
let view = contract.inspect("orders.place", 1).unwrap();  // inputs, outputs, evidence, unknowns
let boundary = contract.replacement_boundary("orders.place", 1);
let slice_for_model = model_json(&view);                  // small, precise context
```

## Run the agent

```sh
npm ci --prefix examples/customers-app
scripts/demo.sh                                        # scripted completions: reproducible, no key
scripts/demo.sh --config examples/models/ollama.json   # a local LLM
scripts/demo.sh --config examples/models/anthropic.json  # a remote provider (ANTHROPIC_API_KEY)

appport-agent plan --project <dir> --intent "replace <name>@<version>: <goal>" --json   # no model involved
appport-agent run  --project <dir> --intent "..." --config <model.json> --report report.json --log run.jsonl
```

## Develop

```sh
cargo test                               # everything that runs offline
cargo test -p appport-agent -- --ignored # the loop on the real runtime (needs Node 20+ and npm ci in examples/customers-app)
scripts/check-appport-drift.sh           # + re-inspect the pinned packages (needs Node 20+)
scripts/check-appport-drift.sh --latest  # against the newest published @appport/*
```
