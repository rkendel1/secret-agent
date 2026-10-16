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

Keys are read from the environment variable the configuration names. They are
never written to a file, the report or the log, and never passed to the
application or a test command, which run code a model wrote (see
[Safety guarantees](#safety-guarantees)). Both HTTP providers are tested against
local stand-ins of their APIs (`crates/agent-model/tests/providers.rs`,
`tests/reliability.rs`).

**Provider failures.**
- **Retried, within one request:**
  - rate limits (429) and failing servers (500, 502, 503, 504, 529), honouring `Retry-After`;
  - failed connections.

  Retries use exponential backoff with jitter, up to `maxRetries` (default 2) and at most `maxRetryWaitSecs` (default 30) per wait. A `Retry-After` longer than that ends the call instead.
- **Never retried:**
  - a timeout, because the request may still be served;
  - 400, 401, 403 and 404.
- **One layer of retries only.** The loop itself never retries a failed model call, so retries do not multiply.
- **Refusals and empty answers** are provider failures, never proposals.

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

## Outcomes and exit status

`run` writes the report (`--report`) for every outcome it can account for:

| outcome | exit | meaning | the project afterwards |
|---|---|---|---|
| `accepted` | 0 | a change passed every check, and showed the goal was achieved (below) | changed: the accepted files |
| `rejected` | 1 | no attempt passed | unchanged: every attempt undone and checked |
| (usage or harness error) | 2 | bad options, or the agent could not run the unchanged application | unchanged |
| `limitation` | 3 | declined before changing anything; `reason` says why: no plan (`refused to plan: …`), no way to show the goal, uncommitted changes in a file it would rewrite | unchanged |
| `provider-error` | 4 | the model provider failed (after its retries) | unchanged |
| `rollback-failed` | 5 | a rejected change could not be undone and checked | **uncertain**: the report lists what was restored, what was not, and what was left alone because it changed meanwhile |
| `interrupted` | — | only ever seen in a report left by a run that died while a change was applied | see `inFlight` |

### Goal evidence

Passing every check shows a change preserves the contract and the application's behaviour. It does not show the change did what was asked. A behaviour-preserving rewrite that ignores the goal passes the same checks.

So `run` accepts a change only with evidence of the goal:

- **A goal check** (`--goal-check <file.json>`): a short scenario run through the application's own capabilities. The check must **fail** on the unchanged project, or the run is declined, because a check that already passes cannot show anything. A change is accepted only if the check then **passes**.

  ```json
  {
    "description": "A newly registered customer gets a billing contact.",
    "steps": [
      { "capability": "customers.register@1", "input": { "name": "Goal Check", "email": "goal.check@example.com", "tier": "gold" } },
      { "capability": "billing.contact@1", "input": { "customerId": { "$step": 0, "at": "/customerId" } },
        "expect": [{ "at": "/email", "equals": { "$step": 0, "input": "/email" } }] }
    ]
  }
  ```

  - Each step must answer `ok`.
  - `{"$step": n, "at": ptr}` refers to an earlier step's output, and `{"$step": n, "input": ptr}` to its input.
  - The application is granted only the permissions those capabilities declare.
  - A task file with a `goalCheck` field works too.
- **Declared verification:** a check the contract declares failed on the unchanged application and passes after the change. For example, `customers.register@1`'s `customer-readable` verification fails until the lookup reads the store.

If neither is available, `run` declines with `limitation` before any model call. `--accept-preserving` accepts a contract-preserving change without goal evidence, and the report records `goal.method: none`.

The evaluation harnesses (`eval`, `arm-eval`) don't need this, because they score the goal with their own oracle.

### Safety guarantees

Each is tested in `crates/appport-agent/tests/readiness.rs` and `src/edit.rs`:

- **Nothing outside the project is read or written.**
  - Paths are normalized lexically and checked component by component.
  - Symbolic links are never followed. A linked source file is reported (`source.symlink`), not observed.
- **Writes are checked first.** Every target is checked before anything is written. Each file is written through a temporary file and a rename.
- **A rollback is reported only when it is checked.**
  - Each file is restored byte for byte (or removed again) and re-read.
  - A file that changed after the agent wrote it is left alone and reported, never overwritten.
- **Uncommitted work is not taken over.** In a git work tree, `run` declines if a file it would rewrite has uncommitted or untracked changes. `--allow-dirty` overrides this.
- **One run per project.** A second concurrent run on the same project is refused. The lock lives in the temporary directory, and a dead run's lock is taken over.
- **Interruption is recorded.** Before the changed application runs, the report is written with `outcome: interrupted` and `inFlight`: each changed file's path, its SHA-256 before and as written, and its original contents. A run that dies there leaves exactly that record. Restoring from it is manual; runs do not resume.
- **No credentials reach model-written code.**
  - The application and test commands run without `AI_GATEWAY_API_KEY`, `ANTHROPIC_API_KEY`, `ANTHROPIC_WORKSPACE_ID`, `OPENAI_API_KEY`, `VERCEL_OIDC_TOKEN`, or any variable the model configuration names.
  - The log and the report are redacted of those values.
- **Test commands are contained on timeout.** A command that times out is killed with its whole process group.

## Tests

| Test | Needs | Covers |
|---|---|---|
| `crates/agent-core` unit + `tests/agent.rs` | nothing | conformance, synthesis, the scanner, observation of the sample project, the plan (snapshot), the model task, refusals, every judge rule including a changed contract |
| `crates/agent-model/tests/providers.rs` | nothing | each provider's wire format, errors, keys, configuration |
| `crates/agent-model/tests/reliability.rs`, `tests/gateway.rs` | nothing | retries, `Retry-After`, no retry of timeouts or auth failures, refusals and empty answers, secret redaction; the free-gateway preflight |
| `crates/appport-agent/tests/loop.rs` (`--ignored`) | Node.js, `npm ci` in the example | the whole loop on the real runtime: accept after a rejection; an edit outside the boundary is never written; a contradicted verification is rejected and rolled back; an implementation that does not load; an unusable response fed back; the offline manifest fixture still matches the runtime |
| `crates/appport-agent/tests/readiness.rs` (mostly `--ignored`) | Node.js, `npm ci` in the example | the production-readiness matrix: goal evidence, exit codes, symlinks, provider failure, retry without a duplicate write, interruption, secrets, uncommitted work, concurrent runs (see `docs/production-readiness.md`) |

CI runs the last group and the demo in the `agent-e2e` job.

## Context modes and the evaluation

`--context bare` sends only the goal and the editable file (the ablation);
`appport-agent eval` runs both modes over many trials. See
`docs/closed-loop-test.md`.

The planner refuses with `contract.unverifiable` when no regression probe and
no declared verification could observe the target's behaviour: a change there
would be accepted on structure alone.

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
- **Generated code runs on your machine, unsandboxed,** when the application or a test command starts. It has your user's filesystem and network access, though not the provider credentials. Run the agent where that is acceptable. See `docs/production-readiness.md`.
