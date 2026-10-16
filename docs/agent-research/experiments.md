# Experiments

How to ask a question about coding-agent effectiveness, and measure the answer against the baseline.

## The baseline

`observed` context with `feedback` recovery, the planner's 48 KB read-only budget, and up to 3 attempts. It is what `code-eval` has always run, now behind the strategy boundary. It renders byte-identical prompts (see [`decisions.md`](decisions.md#d4-the-baseline-is-the-old-behaviour-byte-for-byte)).

| component | values | what changes |
|---|---|---|
| `context` | `minimal` | the intent and the editable files; nothing observed |
| | **`observed`** (baseline) | the editable files, the failing tests, what the code imports, the test output before the change |
| | `dependency-aware` | observed, plus who imports each editable file and where its exported names are used (lexical, with unknowns stated) |
| | `appport-aware` | dependency-aware, plus the published AppPort contract, dependents and declared verification of capabilities implemented in the editable files (from the running application) |
| `recovery` | **`feedback`** (baseline) | retry with the test failure or the reason the reply could not be applied |
| | `expand-context` | as feedback, plus read-only files the failure output names (or, failing that, importers of the editable files) |
| | `stop-on-repeat` | as feedback, but stop when an attempt fails exactly as the previous one did |
| `contextBudget` | bytes (default 49152) | bound on read-only context; editable files always included |

**Every arm has the same:**
- task text, fixture and fresh workspace;
- editable files (the planner's) and acceptance command (the project's);
- hidden acceptance tests;
- model and inference settings, and attempt budget.

## Hypotheses (open)

| id | hypothesis | arms | primary metric | guard metrics |
|---|---|---|---|---|
| H1 | Call-site facts help when the change must keep callers working | `observed` vs `dependency-aware` | verified rate | tokens per trial; context bytes |
| H2 | Contract knowledge helps on AppPort repositories and costs nothing elsewhere | `dependency-aware` vs `appport-aware` | verified rate on `appport-readback`; equality elsewhere | tokens; `accepted-unverified` count |
| H3 | Less context is enough on small tasks | `minimal` vs `observed` | verified rate | tokens (the Opus result in `docs/coding-agent-eval.md` suggests yes on two tasks) |
| H4 | Stopping on a repeated failure saves calls without losing successes | `feedback` vs `stop-on-repeat` | model calls per trial | verified rate must not drop; false-stop rate (stopped trials the feedback arm solved) |
| H5 | Context named by the failure helps the retry | `feedback` vs `expand-context` | verified rate at attempt ≥ 2 | context bytes added; boundary violations (must stay 0 applied) |

None of these has been measured with a live model yet. The scripted runs below exercise the machinery, not the hypotheses: scripted replies do not depend on the context.

## Method

- **One specification per question.** Use `secret-agent.experiment/1` (below). Vary one component per ablation, so a difference can be traced to it.
- **Isolation:**
  - Every trial gets a fresh copy of the task's repository.
  - Fixture digests are taken before and after the run, and a changed fixture fails the run.
  - Arm order rotates per trial and task.
- **Scoring is execution evidence only:**
  - an applied change passed the project's own test command;
  - when the task has hidden acceptance tests, those passed too.

  The model's claims count for nothing. For the categories, see [`decisions.md` D8](decisions.md#d8-failures-are-classified-never-scored-against-a-strategy).
- **Provider and infrastructure failures** are excluded from the scored denominator. They are listed, never dropped.
- **Usage:**
  - Tokens are the provider's figures where it reports them. Otherwise they are estimated and marked *estimated*.
  - Cost is computed only from known pricing (allowlisted free models are $0), and is otherwise *unknown*.
- **Comparisons** against the baseline are descriptive: deltas, with the scored count per arm. There is no significance claim.
- **Live runs** use a free Vercel AI Gateway model only, after the preflight (`agent_model::gateway`). Locally, an OpenAI-compatible server works too. No Anthropic provider is accepted in a specification.

## Running

```sh
# check a specification and see what it would run (no trials)
appport-agent experiment --spec examples/experiments/context-ablation.json --validate

# the baseline, then an ablation, offline with scripted completions
appport-agent experiment --spec examples/experiments/baseline.json
appport-agent experiment --spec examples/experiments/context-ablation.json
appport-agent experiment --spec examples/experiments/recovery-comparison.json

# inspect a finished run
appport-agent experiment --show target/experiments/context-ablation

# live, free gateway model only (needs AI_GATEWAY_API_KEY; preflight runs first)
appport-agent experiment --spec examples/experiments/gateway-free-context.json

# the same arms from code-eval flags
appport-agent code-eval --tasks fixtures/coding --provider scripted --trials 1 \
  --arms observed,minimal,dependency-aware,appport-aware --recovery stop-on-repeat --out target/ctx
```

Or run the manual **agent-live** workflow with `legs: experiment` (the spec defaults to `gateway-free-context.json`).

## The specification

```json
{
  "schema": "secret-agent.experiment/1",
  "id": "context-ablation",
  "suite": "fixtures/coding",
  "tasks": ["bugfix-invoice-discount"],
  "model": { "config": "examples/models/vercel-gateway.json", "overrides": { "maxTokens": 4096 } },
  "arms": [{ "id": "baseline", "context": "observed", "recovery": "feedback", "contextBudget": 49152 }],
  "ablation": { "from": "baseline", "vary": "context", "values": ["minimal", "dependency-aware", "appport-aware"] },
  "baseline": "baseline",
  "trials": 3,
  "maxAttempts": 3,
  "testTimeoutSecs": 120,
  "limits": { "maxModelCalls": 150, "maxMinutes": 60, "budgetUsd": 0 },
  "balancedOrder": true,
  "freeGateway": true,
  "out": "target/experiments/context-ablation"
}
```

| field | default | notes |
|---|---|---|
| `tasks` | all in `suite` | unknown ids are an error |
| `model` | — | an existing configuration file and/or `overrides` of its fields; unknown fields are an error |
| `arms` | — | at least one; ids unique |
| `ablation` | none | adds one arm per value, named `<vary>=<value>`; a value equal to the base is skipped |
| `baseline` | the first arm | |
| `trials`, `maxAttempts`, `testTimeoutSecs` | 1, 3, 120 | |
| `limits.maxModelCalls` | none | a trial that could cross it is `not-run` |
| `limits.maxMinutes` | none | stop starting trials |
| `limits.budgetUsd` | none | needs known pricing |
| `freeGateway` | false | the preflight runs anyway whenever the model is at the gateway |
| `out` | `target/experiments/<id>` | |

## What a run writes

| file | contents |
|---|---|
| `experiment.json` | the resolved specification: arms expanded, model configuration filled in (variable names only, never keys) |
| `preflight.json`, `preflight.txt` | the free-gateway preflight, when it ran |
| `coding-eval.json` | schema `secret-agent.coding-eval/2`: status, provenance (commit, binary SHA-256, configuration SHA-256, per-task fixture digests), whether fixtures were unchanged, arm configurations, per-cell and per-arm aggregates, comparisons, every trial |
| `coding-eval.md` | the same, for people |
| `coding-eval.partial.json` | rewritten after every trial while running, so an interrupted run keeps its records |
| `<task>/<arm>-NN/trial.json` | the trial: strategies, category and reason, patch applied, tests and hidden results, attempts, model ids reported by the provider, tokens (estimated or not), provider retries, recovery decisions, context bytes, timestamps |
| `<task>/<arm>-NN/report.json` | the loop's report: the full context selection (every item with its reason, bytes, inclusion; unknowns), every attempt (context files, test exit, recovery decision) |
| `<task>/<arm>-NN/run.jsonl` | every step, including the exact prompts and responses (redacted of credentials) |
| `<task>/<arm>-NN/final.diff`, `accepted.diff`, `hidden.txt` | the final state against the fixture, the accepted change, the hidden test output |

### Scripted example output

This is the ablation above on the three fixtures with scripted completions (`target/experiments/context-ablation/coding-eval.md`). It shows the context cost of each strategy, not its effect, because scripted replies ignore the prompt:

| arm | differs in | Δ verified rate | scored | Δ tokens per trial | Δ context bytes |
|---|---|---|---|---|---|
| `context=minimal` | context | +0 pp | 3 / 3 | −1350 | −3670 |
| `context=dependency-aware` | context | +0 pp | 3 / 3 | +337 | +938 |
| `context=appport-aware` | context | +0 pp | 3 / 3 | +661 | +2236 |

## Adding a strategy

1. **Implement the trait** in `crates/appport-agent/src/strategy.rs`: a `ContextStrategy` or a `RecoveryPolicy`.
   - Give it an id (a new variant of `ContextStrategyId` or `RecoveryPolicyId`) and a version.
   - Bump the version whenever its behaviour changes.
2. **Report what it uses:**
   - A context strategy reports every item it considers through the builder (`file` / `fact`), with a reason, and records what it cannot establish as an unknown.
   - A recovery policy returns one of the existing `RecoveryDecision` variants. If it seems to need a new one that changes the editable files, permissions or acceptance, the answer is no (see D3).
3. **Test it:** add a unit test beside it, and a scripted end-to-end test in `crates/appport-agent/tests/experiments.rs` if it changes the loop's behaviour.
4. **Measure it:** add an example specification under `examples/experiments/` that ablates it against the baseline.

## Adding a task

Add a directory under `fixtures/coding/` (see `fixtures/coding/README.md`):
- `task.json`;
- `repo/`;
- `hidden/`;
- `golden/`;
- `scripted/`.

`scripts/check-coding-fixtures.sh` must pass: the visible tests fail before the change, and the golden solution passes the visible and hidden tests.
