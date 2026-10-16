# Three-arm evaluation: bare vs context-assisted vs AppPort-native

**Research question.** Does an agent built around AppPort's capability contracts and verification workflow achieve higher verified task completion, fewer retries, and lower total inference and execution cost than an agent that merely receives the same application knowledge as context?

**Status of the evidence.**
- **Historical:** the only measured comparison is the Opus 5.5 run below, made locally through Anthropic's API on 2026-10-16, on the task suite as it was then (with `billing-from-store`, now retired). Its records in [`docs/three-arm/`](three-arm/) are kept unchanged.
- **Current:** live runs now go only through the Vercel AI Gateway with a model verified as free (see [Live runs](#live-runs-free-vercel-ai-gateway-model-only)). No free-model results are reported here yet. Nothing in this document says a free model matches or beats Opus; a free-model run is a separate measurement, to be read on its own.

## Historical results (Opus 5.5, 2026-10-16)

`claude-opus-5-5` (Anthropic API, `effort: high`). 6 tasks × 3 arms × 5 trials = 90 trials, rotated order, $10 budget cap.

- **Spend:** $3.55, no trial skipped for budget.
- **Failures outside the agents:** no provider or harness failures.
- **Raw evidence:** every trial's record, prompts, responses, tool results, verdicts and oracle output are in [`docs/three-arm/`](three-arm/), with the generated tables in [`arm-eval.md`](three-arm/arm-eval.md).

### Headline (5 valid tasks, 75 trials)

`billing-from-store` is excluded; see [A flawed task](#a-flawed-task-billing-from-store).

| arm | verified changes (2 positive tasks) | safe outcomes (3 negative tasks) | of which an explicit limitation | false successes | model calls | cost |
|---|---|---|---|---|---|---|
| A — bare | **0/10** | 15/15 | 10 | 0 | 30 | $0.67 |
| B — context-assisted | **5/10** | 10/15 | 10 | **5** | 10 | $0.24 |
| C — AppPort-native | **10/10** | 15/15 | 15 | 0 | 112 | $1.76 |

### Per task

| task | A — bare | B — context | C — native |
|---|---|---|---|
| `replace-lookup` (named target) | unverified 5/5 | **verified 5/5**: 1 call, $0.019, 5.1 s median | **verified 5/5**: 7 calls, $0.125, 24.8 s median |
| `discover-readback` (no target) | stops: needs a target | stops: needs a target | **verified 5/5**: 8.4 calls, $0.155, 27.0 s |
| `contract-change` (needs an output change) | fail-closed 5/5 | **false success 5/5** | **correct limitation 5/5** |
| `missing-capability` | refused (no target) | refused (no target) | correct limitation 5/5 |
| `unverifiable-contract` | refused by the planner, no model call | refused by the planner, no model call | correct limitation 5/5 (3 calls) |

What happened, from the run logs:

- **`replace-lookup`**:
  - **B and C** both read the store, and every proposal passed on the first try.
  - **A** never sees `store.mjs` and guessed a module that does not exist, as in PR 4.
  - **The cost of C's approach:** it reached the same result as B with 7× the calls, ~7× the cost and ~5× the time. Before proposing, C chose to list capabilities, inspect the contract and read the store and the legacy code itself. B was handed exactly those files by the deterministic planner.
- **`discover-readback`** (only C can attempt it). C:
  1. listed the capabilities;
  2. inspected `customers.lookup@1` and the legacy export;
  3. inspected `customers.register@1` to find where customers are written;
  4. read `src/store.mjs`;
  5. proposed a store-backed lookup, which was verified first time in every trial.
- **`contract-change`**:
  - **C** inspected the contract and reported that the output schema has no phone field and admits no extra properties.
  - **B has no way to say "this needs a contract change."** In all 5 trials its model made the change it *could* make: a store-backed lookup that preserves the contract, with a code comment saying the phone number was not done. The judge checks that the contract and behaviour are preserved, not that the goal was met, so it accepted. That is a false success.
  - **A** guessed a nonexistent module three times and failed closed.
- **Negatives without a target:** A and B stop before any model call because their loop requires `replace <capability>@<version>`. That counts as safe, but it is not a diagnosis. C reached the same outcome through the contract and said why: no delete capability exists; the contract declares no verification.

### A flawed task: `billing-from-store`

> **Retired.** This task is invalid as a measurement and is no longer in the active suite; it is kept in [`fixtures/appport-tasks-retired/`](../fixtures/appport-tasks-retired/) so these records stay reproducible. Its replacement is [`billing-migration`](#the-oracle), which has an observable goal check.

| arm | oracle outcome | what the proposals actually did |
|---|---|---|
| A — bare | verified 3/5 | kept calling the legacy `lookupCustomer` |
| B — context | verified 5/5 | 4 read the legacy CRM export directly, 1 kept the lookup; none used the store (the graph gave B no path to `store.mjs`, because billing declares no resource access) |
| C — native | wrong limitation 5/5 | declined: `billing.contact@1` declares `requires: customers.lookup@1` (`required: true`), so bypassing the lookup makes a declared dependency false, which is a contract change; and the capability declares no verification |

**No arm achieved the stated goal, and the oracle counted 8 trials as successes anyway.**
- **Why the oracle passed them:** it verifies preservation (same answers, same contract, declared verification), not goal achievement. For `replace-lookup` the goal is observable, because `customers.register@1`'s declared verification only passes when the lookup reads the store. For `billing-from-store` nothing observable distinguishes "reads the store" from "unchanged".
- **C's reading is defensible:** the task asks for something the contract's declared dependency contradicts.

This task is excluded from the headline and reported here as the raw outcome. It was fixed by doing both: `billing-migration` uses a contract whose declarations allow the change, and the oracle runs a goal check the unchanged fixture fails.

### Answer to the research question, as far as these tasks go

- **Verified task completion: yes, C is higher.**
  - **Positive tasks:** C completed 10/10, against B's 5/10 and A's 0/10. Half of B's shortfall is structural: B needs a named target, and C can discover one.
  - **Named-target task:** where both B and C can run, they tie at 5/5.
- **Trustworthiness: C is higher.** C produced no false successes and 15/15 correct limitations. B claimed success 5 times on a request it could not meet. Its acceptance criteria test preservation, and it has no channel to decline.
- **Retries: no difference.** Neither B nor C needed one on any task both completed.
- **Cost: C is much more expensive.**
  - **Valid tasks:** $1.76 for C vs $0.24 for B (7.3×), with 112 vs 10 model calls.
  - **Named-target task:** where both succeed, C costs ~7× as much and takes ~5× as long.
  - **Where the cost goes:** C pays for discovery and inspection that B's deterministic planner does for free. C's conversation also grows with each tool result; C sent 171 KB of AppPort results over the full run.

**So:** making AppPort the model's operating model buys capabilities the context arm lacks:
- **finding** the capability to change;
- **declining** what the contract forbids;
- **explaining** why it declined.

It costs about 7× more when the context arm could have done the job. On a task with a named target and a sufficient contract, the context-assisted arm is the cheaper way to the same verified result.

### What remains unproven

- **Sample size:** one model, five trials per cell, small tasks in one application.
- **The oracle (at the time):** it measured preservation and declared verification only; `billing-from-store` showed why that is not enough. The oracle now also runs a goal check on every positive task (see [The oracle](#the-oracle)).
- **A hybrid:** B's deterministic planning with C's limitation channel and discovery fallback is untested. The per-task numbers suggest it would keep most of C's coverage at close to B's cost.
- **Smaller or local models:** not tested.
- **The GitHub `agent-live` run of 2026-10-16:** it produced no measurements. Every model call was refused by the Vercel AI Gateway (`HTTP 403: Free tier users do not have access to this model`), because the requested model was a paid one. The workflow now runs a free model only, after a preflight that stops the run before any trial (see [Live runs](#live-runs-free-vercel-ai-gateway-model-only)).


## The arms

| arm | what runs | how the model sees AppPort |
|---|---|---|
| **A — bare** | the existing loop (`appport-agent run --context bare`), unchanged | not at all: the goal and the editable file |
| **B — context-assisted** | the existing loop (`--context knowledge`), unchanged | as context: the deterministic core discovers, plans and selects files; the model receives the capability's contract slice, constraints, checks and the selected files in one prompt, and gets the violations back on retry |
| **C — AppPort-native** | `appport_agent::native`, enabled only by `arm-eval` | as an operating model: the model works through tools, one per reply, and decides what to discover, inspect, read, probe and propose |

### What C's tools are made of

No new AppPort API and no second contract authority. Every tool is an existing interface:

| tool | built on |
|---|---|
| `list_capabilities` | the running application's own `appport.manifest@1`, over the AppPort IPC binding |
| `inspect` | `Contract::inspect` and `Contract::replacement_boundary` (PR 1) on that manifest, plus where the project declares and implements the capability (`observe`) |
| `read` | a project source file, read-only |
| `invoke` | an observation capability on the running application, through the runtime's pipeline under exactly that capability's declared permissions. Consequential capabilities are refused; they are exercised only by declared verification. |
| `propose` | the same deterministic planner, edit boundary, evidence collection and judge that B uses; the verdict is returned to the model |
| `complete` | accepted only when a proposal has been verified, otherwise refused (fail-closed) |
| `limitation` | ends the trial; any applied change is rolled back |

## Fairness

- **Same everything that can be the same:**
  - the same task text (`goal`, and `target` when the task names one) for every arm;
  - the same fixture, copied fresh for every trial;
  - the same model configuration;
  - the same proposal budget (3);
  - the same judge for in-loop acceptance.
- **Arm order** rotates per trial and task, so each arm runs first, second and third equally often.
- **The oracle is independent of every arm.** See [The oracle](#the-oracle). No arm's own claim is credited.
- **Every trial is recorded** (`trial.json`, the arm's report and run log, `oracle.json`), including failures and budget-skipped trials.

### Unavoidable differences, recorded

- **A and B need a named target.** Their loop takes `replace <capability>@<version>`. On the two tasks without a target, they stop before any model call. This is what those workflows are, not a handicap added for the test.
- **Only C can report a limitation.** A and B have no channel for it. On a negative task they are safe only by failing closed: no proposal passes, so nothing is kept.
- **C chooses what to look at,** so it makes more model calls. Each reply is one tool call, so C's model calls include discovery and inspection. Those calls, their tokens and the bytes of AppPort results sent to the model are all counted.
- **The budgets differ in shape.** C's conversation is capped at 12 model calls. A and B make at most 3 calls, one per proposal.

## The oracle

After each trial, independently of the arm:

1. **Preservation.** It re-plans the task's `oracleTarget` on a pristine copy of the fixture, collects a fresh baseline, runs the final project, and judges it with the deterministic judge. The changed files come from diffing against the fixture. This checks same answers, same contract and declared verification.
2. **The goal (positive tasks).** It runs the task's `goalCheck` against the final project, in a fresh process, granted only the permissions the steps' capabilities declare in the fixture's own contract. Each step invokes a capability and must answer `ok`, with the expected values. For example, `billing-migration` registers a customer through `customers.register@1`, then asks `billing.contact@1` for that customer and expects the registered name and email.
3. **The goal check must fail on the unchanged fixture.** The oracle runs it there first. If it passes, the check measures nothing, and the oracle reports an error instead of a verdict.

**Verified** means preservation and the goal both hold. For a negative task, the project must be byte-identical to the fixture.

This is what the retired `billing-from-store` lacked. Its legacy implementation passes every behaviour check, so preservation alone could not tell "reads the store" from "unchanged". `crates/appport-agent/tests/arms.rs` pins this down:
- on the unchanged `billing-migration` fixture, the only failures are "no change was made" and `goal.not-achieved`;
- a scripted arm that rewrites billing but still goes through the legacy lookup is accepted by the in-loop judge, and the oracle scores it `preserved-only`.

## Task suite (`fixtures/appport-tasks/`)

| task | expect | category | target given | project |
|---|---|---|---|---|
| `replace-lookup` | verified change | contract-conforming change (the PR 4 task) | `customers.lookup@1` | `examples/customers-app` |
| `discover-readback` | verified change | capability discovery and selection | none | `examples/customers-app` |
| `billing-migration` | verified change | incremental legacy replacement with an observable goal | `billing.contact@1` | `fixtures/appport-projects/customers-billing` (billing declares that it reads customer records and depends on no other capability, but is still implemented through the legacy lookup) |
| `contract-change` | limitation | unsupported: the goal needs an output-contract change | `customers.lookup@1` | `examples/customers-app` |
| `missing-capability` | limitation | missing capability (delete a customer) | none | `examples/customers-app` |
| `unverifiable-contract` | limitation | verification cannot establish the outcome | `customers.lookup@1` | `fixtures/appport-projects/customers-undeclared` (a contract with no effects or verification) |

Every positive task has a goal check. `billing-from-store` was retired (see above).

No task needs GnuCOBOL, a network download or an external runtime beyond Node.js and the npm packages already used.

### Outcome categories

| category | meaning |
|---|---|
| `verified-success` | positive task; the arm claimed success, and the oracle verified both preservation and the goal |
| `preserved-only` | positive task; the arm claimed success and preserved behaviour, but the goal check failed. Counted as a false success. |
| `false-success` | the arm claimed success, but the oracle did not verify it; or, on a negative task, the arm claimed success or left a change |
| `wrong-limitation` | positive task; the arm stopped with a limitation |
| `unverified` | positive task; no proposal passed |
| `correct-limitation` | negative task; the arm reported a limitation and the project is unchanged |
| `fail-closed` | negative task; no proposal passed and the project is unchanged, but nothing was reported |
| `provider-refusal`, `provider-error`, `harness-error` | not agent failures; counted separately |
| `not-run` | skipped because a budget, the model-call cap or the time limit was reached; listed, never dropped |

## Live runs: free Vercel AI Gateway model only

Live runs use one path: the Vercel AI Gateway's OpenAI-compatible endpoint (`https://ai-gateway.vercel.sh/v1`), with a model that has been shown to be free.
- No Anthropic API calls.
- No paid model through the gateway.
- No fallback to another model or provider.

The default model is `nvidia/nemotron-3.5-lightning-free` ([`examples/models/vercel-gateway.json`](../examples/models/vercel-gateway.json)).

### What "free" means here

A successful request does not show that a model is free. Before any trial, the preflight (`crates/agent-model/src/gateway.rs`) checks:

1. **Configuration.**
   - The provider is `openai-compatible` and the endpoint is the gateway. Anthropic's provider and any other URL are refused.
   - The key's environment variable (`AI_GATEWAY_API_KEY`) is set. Its value is never printed.
2. **Allowlist.** The model is on `FREE_GATEWAY_MODELS`. A `-free` suffix alone is not enough.
3. **Catalog pricing.** The gateway's `GET /v1/models` must list the model and publish its input and output prices, and both must be zero.
   - No published pricing is `pricing-unverified`.
   - Any price above zero is `not-free`.
   - In both cases nothing is sent to the model.
4. **One minimal completion.** It is non-streaming and limited to 256 tokens. It must return non-empty content.

The result is written to `preflight.json` and `preflight.txt`.

**When the preflight fails:**
- no trial runs;
- the command exits 1;
- the report's status is `preflight-failed`.

Each failure has its own category, so a provider problem is never scored as an agent outcome or as a refusal:

| category | meaning |
|---|---|
| `config` | the configuration is not a free-gateway one: wrong provider or endpoint, no model, or no key |
| `not-allowlisted` | the model is not on the free allowlist |
| `pricing-unverified` | the catalog does not publish both prices for the model |
| `not-free` | the catalog prices the model above zero |
| `unavailable` | the model is not in the catalog, or the gateway says it does not exist |
| `auth` | the gateway rejected the key (401) |
| `forbidden` | the key may not use the model (403), such as a free-tier restriction |
| `rate-limited`, `timeout`, `unreachable`, `provider-error` | the provider or network failed |
| `malformed` | the catalog or the completion was not usable |

**Adding a model:** confirm in the gateway catalog that its input and output prices are zero, then add it to `FREE_GATEWAY_MODELS`. The live check still runs on every run.

### Modes

| mode | what runs |
|---|---|
| `preflight` | the preflight only |
| `smoke` | `replace-lookup` and `contract-change`, every arm, one trial: a positive task and a negative one, to show the path works end to end |
| `comparison` | every task, `--trials` trials per arm |

**Run limits:**
- Before starting, the run prints the most model calls it could make: A and B make up to `--max-attempts` per trial, and C up to `--max-turns`.
- `--max-model-calls` stops any trial from starting if it could take the run past the cap.
- `--max-minutes` stops starting trials after that time.
- Trials that do not start are recorded as `not-run` with the reason.

### What a run records

- **`arm-eval.json` / `arm-eval.md`:**
  - the status: `complete`, `preflight-failed`, or `partial` while running;
  - the counts: planned, attempted, completed, provider-failed and not-run trials; model calls made against the most planned;
  - the preflight;
  - provenance: the git commit, the binary's SHA-256, the model configuration and its SHA-256 (it names the key's variable, never the key), and a SHA-256 per task over the task and its project's files.
- **Per trial:** the provider and model it ran with, plus everything listed under [Fairness](#fairness).
- **`arm-eval.partial.json`:** rewritten after every trial, so an interrupted run still leaves its records and counts.
- **Secrets:** reasons, logs and reports are scrubbed of the key before they are written.

Provider errors are counted separately from agent outcomes. The run fails (exit 1) when:
- the preflight failed;
- a trial hit a harness error;
- no model call succeeded;
- more than 10% of trials hit provider errors.

### Running it

The **agent-live** workflow is manual (`workflow_dispatch`) only:
- `mode` defaults to `smoke`;
- `comparison` also needs the `confirm_comparison` box;
- `gateway_model` defaults to the free model;
- the `AI_GATEWAY_API_KEY` secret is passed only to the steps that call the gateway.

Every run starts with a `preflight` job, and each leg runs the preflight again before its trials. Artifacts are uploaded even when a step fails.

Locally:

```sh
export AI_GATEWAY_API_KEY=...   # a Vercel AI Gateway key
appport-agent preflight --config examples/models/vercel-gateway.json --out target/preflight
appport-agent arm-eval --tasks fixtures/appport-tasks --free-gateway --mode smoke \
  --config examples/models/vercel-gateway.json --out target/arms-smoke
```

### Limitations

- **A small free model is not Opus.** Its results measure that model. They neither confirm nor refute the Opus results above.
- **The gateway's terms can change.** A free model can become priced, rate-limited or unavailable to an account. The preflight then stops the run, and nothing is measured.
- **Price is checked once per run.** It is checked when the run starts, not before every call.

## Reproduce

```sh
npm ci --prefix examples/customers-app
# deterministic, no paid inference (what CI runs): scripted arms, the oracle's goal
# checks, and the preflight against a local stand-in for the gateway
cargo test -p appport-agent -- --include-ignored
cargo test -p agent-model --test gateway
# live, free model only
appport-agent arm-eval --tasks fixtures/appport-tasks --free-gateway --mode comparison \
  --config examples/models/vercel-gateway.json --trials 5 --max-model-calls 600 --out target/arms-live
```

The historical Opus run used `--config examples/models/anthropic.json --trials 5 --budget-usd 10` on the suite of that time, which included `billing-from-store` (now in `fixtures/appport-tasks-retired/`).
