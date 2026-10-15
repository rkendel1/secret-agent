# The autonomous loop

PR 1 gave the agent a deterministic understanding of AppPort
(`agent_core::appport::knowledge`). This PR uses it to run a closed loop on a
real project:

```
observe ──► read the contract ──► plan ──► model (implementation only) ──► apply ──► run the app ──► judge
   ▲          (from the app's        │                                                    │            │
   │           own runtime)          │◄──────────── violations, as tightened constraints ─┴── reject ──┤
   │                                                                                                   ▼
 filesystem                                                                               accept (keep) / give up (restore)
```

The model writes code. It is never asked what the project contains, what depends
on what, what must be preserved, or whether the result is right. Those are
answered by `agent-core`, from the contract the application's runtime publishes
and from what the running application answers.

## Pieces

| Crate / path | Role | Calls a model? |
|---|---|---|
| `agent-core::observe` | Walks the project: `@appport/*` dependencies, the module that calls `createApplication`, every `defineCapability({ name, version, handler })` and the module implementing it, the import graph. A small JS scanner, no execution; what it cannot resolve becomes a `Finding`. | no |
| `agent-core::plan` | Intent → `Plan`: target, `CapabilityView` and `ReplacementBoundary` (PR 1), the editable file, read-only context, least-privilege permissions, the probe plan, the acceptance checks, and every `Decision` with the fact it rests on. Refuses rather than guesses. | no |
| `agent-core::conformance` | Checks values against AppPort schemas; synthesizes conforming inputs deterministically. | no |
| `agent-core::verify` | Evaluates declared verification methods into `VerificationEvidence` (the published type) and judges a candidate against the baseline. | no |
| `agent-model` | `Model` trait; `openai-compatible`, `anthropic`, `command`, `scripted` providers. Selected by configuration only. | **yes — this is the only place** |
| `appport-agent` | The CLI and the loop: runs the application, collects evidence, applies and rolls back changes, writes metrics and the run log. | through `agent-model`, once per attempt |
| `appport-agent/host/appport-ipc-host.mjs` | Hosts the project's own application behind `AppPortConnection` (`@appport/server`) and carries one AppPort envelope per line over stdio. | no |
| `examples/customers-app` | A small application built with `@appport/core` 1.2.0, used by the demo and the end-to-end tests. | — |

The knowledge module from PR 1 is reused unchanged.

## How the agent sees the application

Nothing about the contract is read from source. The agent starts the project's
application (the export observation found) and asks it, in AppPort, for
`appport.manifest@1`. That manifest goes through `Contract::from_value`,
`validate`, `graph`, `inspect` and `replacement_boundary` from PR 1.

Every later request also goes through the runtime's own pipeline (validate,
authorize, execute, validate output) over the IPC binding. The session holds
only the permissions the plan derived from the contract (`customers.read`,
`customers.write` in the demo); discovery runs with none.

## What the planner decides, and from what

For `replace customers.lookup@1` on the sample app (`docs/demo/replace-customer-lookup.plan.json`):

| Decision | Grounded in |
|---|---|
| The change may touch only `src/handlers/customers.lookup.mjs` | `defineCapability({ handler: lookupCustomer })` + `import { lookupCustomer } from "./handlers/customers.lookup.mjs"` |
| `billing.contact@1` must keep answering as before | graph: it `observes` the target; code: its module imports the target's module |
| `customers.register@1`'s declared verification must confirm afterwards | graph: its state method `customer-readable` is `verified-by` the target |
| Existing state is discovered through `customers.list@1` | it is an observation with no required input whose output carries the `…/crm/Customer` concept the probed inputs need |
| The model sees the register and list handlers and `store.mjs` | graph: they `change` / `read` `urn:example:customers:customer`, which the target reads |
| The model does not see the contract module or `billing.contact.mjs` | the contract is sent as knowledge (`CapabilityView`), not as an editable file; the dependent is verified, not edited |
| Failures must be `NOT_FOUND` | `integration.failures` |

## Acceptance checks

All must pass; there is no partial acceptance.

| Code | Passes when | Source |
|---|---|---|
| `edit.boundary` | only the editable file changed (others are never written) | plan |
| `runtime.load` | the application starts and answers `appport.manifest@1` | runtime |
| `contract.identity` | `contractDigest` and the target's `contract_pin` are unchanged | PR 1 `rules::contract_digest`, `ReplacementBoundary` |
| `contract.valid` | the published contract has no validation errors | `Contract::validate` |
| `output.conformance` | every output observed conforms to its declared schema, checked by the agent | `outputSchema` |
| `failures.declared` | the target fails only with declared codes (or, if none are declared, codes it already used) | `integration.failures` |
| `behavior.regression` | the target and its observation dependents answer existing-state probes exactly as the baseline did (errors by code) | baseline run |
| `verification` | per flow: no method contradicted, every state method confirmed | `integration.verify` |

### Probes

Run in a fresh application process for the baseline and for every attempt, in
this order, so a consequential call never changes what a regression probe sees:

1. **Enumerate**: call each enumerator; collect values by `x-appport-concept`.
2. **Regression**: for each probed observation, one input per known value of each
   concept-annotated required input (up to 5), plus one value nothing has
   (`absent-…`). The candidate replays exactly the baseline's inputs.
3. **Flows**: subscribe to the events the methods name, call the capability with
   a synthesized input (fresh per seed, enum values rotated), then run its
   declared methods.

### Evaluating declared verification

The packages publish the evidence shape (`VerificationEvidence`) but no
evaluator, so the agent defines one, following the contract's own rules:

- **state** (establishes): call `via` with `$input`/`$output` resolved; each
  `expect` is `equals` (JS equality) or `exists`. Disagreement → `contradicted`,
  with `mismatches`; a failing probe → `unavailable` with its `errorCode`; an
  unresolvable reference → `not-run`.
- **event** (corroborates): `confirmed` if a delivered event matches; otherwise
  `unavailable`. An absent event never contradicts.
- outcome: `unverified` if anything is contradicted, `verified` if a state
  method confirmed, else `unverifiable`.

## Choosing a model

Configuration only; the loop, the observation and the verification are the same:

```sh
# local LLM through Ollama's OpenAI-compatible endpoint
appport-agent run --project <dir> --intent "<intent>" --config examples/models/ollama.json
# llama.cpp llama-server
appport-agent run ... --config examples/models/llama-cpp.json
# remote: Anthropic (ANTHROPIC_API_KEY) or any OpenAI-compatible API
appport-agent run ... --config examples/models/anthropic.json
# any CLI runner: prompt on stdin, completion on stdout
appport-agent run ... --provider command --command "my-model-runner --flag"
# pre-written completions, for reproducible runs and CI
appport-agent run ... --provider scripted --responses examples/scripted/replace-customer-lookup
```

Keys are read from the environment variable the configuration names and are
never written to a file, the report or the log. Both HTTP providers are tested
against local stand-ins of their APIs (`crates/agent-model/tests/providers.rs`).

## Demo and metrics

```sh
scripts/demo.sh                                    # scripted, reproducible
scripts/demo.sh --config examples/models/ollama.json
```

Outputs `target/demo/{plan.json,report.json,run.jsonl}`. The committed run is in
`docs/demo/` (`*.log.txt` is the human-readable progress, `*.run.jsonl` every
step including the exact prompts, `*.report.json` the metrics).

Committed run (`--provider scripted`):

| Metric | Value |
|---|---|
| Outcome | accepted on attempt 2 |
| Attempt 1 | rejected: `failures.declared`, `behavior.regression` — a missing customer raised a plain `Error`, so callers got `INTERNAL_ERROR` instead of the declared `NOT_FOUND` (also through `billing.contact@1`). The runtime accepted it; the agent's checks did not. |
| Model calls | 2 (1 rejected by verification), 3,800 in / 293 out tokens (estimated: the scripted provider reports no usage) |
| Model calls for discovery / for verification | 0 / 0 |
| Established without the model | 6 capability versions, graph of 9 nodes / 13 edges, 2 dependents, 9 replacement constraints, 8 acceptance checks, 15 recorded decisions |
| Application runs / AppPort envelopes | 4 (discovery, baseline, 2 candidates) / 43 |
| Verification methods evaluated | 6 (2 per run) |
| Context sent to the model | 5 of 8 source files, 2,678 of 8,750 source bytes, plus 1,339 bytes of contract |
| Baseline (why the change was needed) | `customers.register@1#customer-readable` unavailable (`NOT_FOUND`): a customer just registered cannot be read back through the legacy lookup |

These numbers show where the work happened. They are not yet a comparison
against a model working without the knowledge layer: the scripted completions
stand in for a model, and the rejected one is hand-written to show the rejection
path. A comparison needs runs with a real local and remote model, with and
without the plan's context, on the same intent; the report format already
records what that comparison needs (attempts, rejected checks, tokens, context
size).

## Tests

| Test | Needs | Covers |
|---|---|---|
| `crates/agent-core` unit + `tests/agent.rs` | nothing | conformance, synthesis, the scanner, observation of the sample project, the plan (snapshot), the model task, refusals, every judge rule including a changed contract |
| `crates/agent-model/tests/providers.rs` | nothing | each provider's wire format, errors, keys, configuration |
| `crates/appport-agent/tests/loop.rs` (`--ignored`) | Node.js, `npm ci` in the example | the whole loop on the real runtime: accept after a rejection; an edit outside the boundary is never written; a contradicted verification is rejected and rolled back; an implementation that does not load; an unusable response fed back; the offline manifest fixture still matches the runtime |

CI runs the last group and the demo in the `agent-e2e` job.

## Limits

- One intent: `replace <capability>@<version>` for a request capability, with an
  implementation-only change (the contract must not change).
- Observation reads the shapes `@appport/*` defines in JavaScript/TypeScript; a
  name or version that is not a literal, or a handler built dynamically, is
  reported as a finding and the agent refuses to plan.
- Preconditions are not executed (recorded as a decision when they probe the
  target). Stream capabilities are not probed.
- Inputs whose schema needs a `pattern` cannot always be synthesized; such a flow
  is recorded as not run and its check fails.
- Generated code runs on your machine when the application is started. Run the
  agent where that is acceptable.
