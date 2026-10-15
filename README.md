# appport-agent

A Rust coding agent built around one separation: the model (a local LLM or any
remote provider) is a pluggable reasoning component; structure, knowledge and
authority live in the agent. The agent observes reality instead of asking the
model to invent it.

## Layout

| Path | What |
|---|---|
| `crates/agent-core` | Deterministic core. `appport::knowledge` is AppPort's contracts, capabilities, integrations, graphs, verification rules and replacement constraints, derived from the published `@appport/*` packages. No model calls, no provider code, no Node at runtime. |
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

## Develop

```sh
cargo test                               # everything, offline
scripts/check-appport-drift.sh           # + re-inspect the pinned packages (needs Node 20+)
scripts/check-appport-drift.sh --latest  # against the newest published @appport/*
```
